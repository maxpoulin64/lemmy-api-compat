This is a quick and dirty API compatibility proxy for Lemmy's backend. It translates 0.18.5 API calls into 0.19.x calls so that apps still expecting (currently stable) 0.18.x APIs.

The only reason this exists is that I updated my instance to 0.19 accidentally and broke everything and I just want it to work.

# Installing

- Clone repository
- Make sure you have a recent Rust toolchain
- Run `cargo build --release`

## Usage

```
env LEMMY_UPSTREAM=lemmy-api:8536 target/release/lemmy-api-compat
```
