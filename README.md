# simple-blog

[![CI](https://github.com/P4suta/simple-blog/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/P4suta/simple-blog/actions/workflows/ci.yml)

A personal blog server with Markdown content, passkey login, and SQLite storage.
Version 0.1 remains under development.

## Run locally

Use the Rust toolchain pinned in [rust-toolchain.toml](rust-toolchain.toml).
See the [build prerequisites](CONTRIBUTING.md#build-prerequisites) for native dependencies.

```sh
git clone https://github.com/P4suta/simple-blog.git
cd simple-blog
cargo run --locked -- init
cargo run --locked -- serve
```

The CLI prints a temporary setup URL for the first owner passkey.
Data lives in `./data` by default.
The native server provides scheduling, revisions, feeds, search, and backups.

## Documentation

- [Operations](docs/operations.md): publishing, imports, migration, backups, recovery, and diagnostics.
- [Development and verification](CONTRIBUTING.md).
- [Product vision](docs/vision.md), [public interfaces](docs/public-surface.md), and [architecture decisions](docs/adr/README.md).
- [Cloudflare adapter](adapters/cloudflare/README.md): deployment also needs the compatible internal Core service.

See the [security policy](SECURITY.md) for private vulnerability reports and the [code of conduct](CODE_OF_CONDUCT.md) for participation.

## License

Licensed under either [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
