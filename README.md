# simple-blog

A writing-focused, single-owner dynamic CMS in one Rust binary.

> v0.1 is under active development and is not yet a production release.

## Properties

- Axum and bundled SQLite; no external database or Node.js runtime
- Server-rendered public and admin pages
- Markdown as the canonical content source, rendered to sanitized HTML
- Passkey-only owner authentication
- Scheduled publishing, revisions, redirects, media variants, feeds, and backups
- Structured tracing, stable diagnostic codes, deep `doctor` checks, and failure-path tests

## Run locally

Rust 1.88 or newer is required. Node.js 24 is needed only when rebuilding the embedded admin asset.

```sh
cargo run --locked -- init
cargo run --locked -- serve
```

`init` prints a short-lived setup URL for registering the first owner passkey. Data is stored in `./data` by default.

## Verify

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
PROPTEST_CASES=1024 cargo test --locked --all-targets --all-features
npm ci
npm run check:admin
```

Architectural decisions live only in [`docs/adr`](docs/adr/README.md). Changes follow Red–Green–Refactor and may not use unscoped lint suppression.

## License

Licensed under either [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
