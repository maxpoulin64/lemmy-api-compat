use std::{collections::HashMap, convert::Infallible, error::Error, net::SocketAddr, sync::Arc};

use hyper::{
    body::{to_bytes, Bytes},
    client::HttpConnector,
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    service::{make_service_fn, service_fn},
    Body, Client, HeaderMap, Request, Response, Server, Uri,
};

struct ProxyContext {
    client: Client<HttpConnector>,
    upstream: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let server_addr = SocketAddr::from(([127, 0, 0, 1], 8536));

    let upstream = match std::env::var("LEMMY_UPSTREAM") {
        Ok(value) => Ok(value),
        Err(_) => Err("Missing LEMMY_UPSTREAM value"),
    }?;

    let context = Arc::new(ProxyContext {
        client: Client::new(),
        upstream,
    });

    let make_service = make_service_fn(|_conn| {
        let context = context.clone();
        let service = service_fn(move |req| proxy_request(context.clone(), req));

        async move { Ok::<_, Infallible>(service) }
    });

    let server = Server::bind(&server_addr).serve(make_service);

    if let Err(e) = server.await {
        eprintln!("Server error: {}", e);
    }

    Ok(())
}

/// Proxies an incoming request to the Lemmy backend, rewriting any legacy auth
/// parameter to an Authorization header
async fn proxy_request<'a>(
    context: Arc<ProxyContext>,
    incoming_request: Request<Body>,
) -> Result<Response<Body>, Infallible> {
    let (incoming_parts, incoming_body) = incoming_request.into_parts();
    let (incoming_headers, incoming_uri) = (incoming_parts.headers, incoming_parts.uri);

    let (proxy_headers, proxy_body) =
        match try_inject_auth_header(&incoming_uri, &incoming_headers, incoming_body).await {
            Ok(result) => result,
            Err(err_resp) => return Ok(err_resp),
        };

    let mut proxy_request = Request::builder()
        .uri(
            Uri::builder()
                .scheme("http")
                .authority(context.upstream.clone())
                .path_and_query(incoming_uri.path_and_query().unwrap().as_str())
                .build()
                .unwrap(),
        )
        .method(incoming_parts.method)
        .body(proxy_body)
        .unwrap();

    *proxy_request.headers_mut() = proxy_headers;

    let proxy_response = context.client.request(proxy_request).await;

    Ok(match proxy_response {
        Ok(response) => response,
        Err(e) => Response::builder()
            .status(502)
            .body(Body::from(format!("Upstream failed to respond: {}", e)))
            .unwrap(),
    })
}

/// Attempts to convert a GET ?auth= query parameter or a JSON body "auth"
/// property to an Authorization header.
///
/// May consume the request body and return a new one, but will return the same
/// unprocessed body if possible
async fn try_inject_auth_header(
    uri: &Uri,
    headers: &HeaderMap,
    body: Body,
) -> Result<(HeaderMap, Body), Response<Body>> {
    let mut proxy_headers = headers.clone();

    // Do nothing in presence of existing authorization header
    if headers.contains_key(AUTHORIZATION) {
        Ok((proxy_headers, body))
    }
    // If we can find auth in the query string, use that
    else if let Some(auth) = extract_auth_from_query(uri.query()) {
        // We got a ?auth= parameter, no need to parse body
        proxy_headers.append(AUTHORIZATION, auth_token_to_bearer(&auth));
        Ok((proxy_headers, body))
    }
    // Otherwise, attempt to match an auth param in the body
    else {
        let (body, auth) = try_extract_auth_from_body(headers, body).await?;

        if let Some(auth) = auth {
            proxy_headers.append(AUTHORIZATION, auth_token_to_bearer(&auth));
        }

        Ok((proxy_headers, body))
    }
}

/// Attempts to extract a ?auth= query paramter
fn extract_auth_from_query(query: Option<&str>) -> Option<String> {
    if let Some(query_string) = query {
        let query_map: HashMap<String, String> =
            url::form_urlencoded::parse(query_string.as_bytes())
                .into_owned()
                .collect();

        if let Some(auth) = query_map.get("auth") {
            Option::Some(auth.to_owned())
        } else {
            Option::None
        }
    } else {
        Option::None
    }
}

/// Attempts to extract an "auth" property from a JSON body
///
/// Will consume the body if the content-type is application/json. It may fail
/// and return an error response if it does so. If the body merely doesn't parse
/// as JSON, then the body is reconstructed and no authorization header is
/// returned.
async fn try_extract_auth_from_body(
    headers: &HeaderMap,
    body: Body,
) -> Result<(Body, Option<String>), Response<Body>> {
    // If not application/json, don't waste our time
    if !headers.get(CONTENT_TYPE).map_or(false, |h| {
        h.to_str().unwrap_or("").contains("application/json")
    }) {
        return Ok((body, Option::None));
    }

    let data = body_to_bytes(body).await?;

    let auth = match String::from_utf8(data.to_vec()) {
        Ok(data) => match json::parse(&data) {
            Ok(data) => data["auth"].as_str().map(|v| v.to_owned()),
            _ => Option::None, // No auth if we can't parse as JSON
        },
        _ => Option::None, // No auth if we can't parse as UTF-8
    };

    Ok((Body::from(data), auth))
}

/// Converts a plain auth token to a Bearer token header value
fn auth_token_to_bearer(auth: &str) -> HeaderValue {
    let h = format!("Bearer {}", auth);
    HeaderValue::from_str(&h).unwrap()
}

/// Converts a body to a Bytes
///
/// Returns an error response in case of error reading the body
async fn body_to_bytes(body: Body) -> Result<Bytes, Response<Body>> {
    if let Ok(body) = to_bytes(body).await {
        Ok(body)
    } else {
        Err(Response::builder()
            .status(400)
            .body(Body::from("Failed to receive request body"))
            .unwrap())
    }
}
