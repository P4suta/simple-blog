# ADR 0018: A declared public surface

- Status: Accepted
- Date: 2026-09-06

## Context

The crate is published as both a binary and a library. Twelve modules were public and nothing distinguished an interface a caller may build on from one that is public only because `src/main.rs` and the integration tests are separate crates. The vision says the project is not a framework and not a plugin platform, so an undeclared public surface is a promise the project never made and does not intend to keep, offered to anyone who reads the documentation. Two modules exist purely for the binary: `cli` returns `anyhow::Result` and exposes clap types, and `observability` installs a global tracing subscriber and replaces the process panic hook.

## Decision

Every module in `src/lib.rs` carries exactly one tier in the crate documentation, which is the normative list. `supported` is an interface this project maintains for callers. `reachable` is public because a supported signature names it, and carries no promise. `internal` is public only because the binary is a separate crate; it carries `#[doc(hidden)]` and is absent from the published documentation.

Nothing is narrowed to `pub(crate)`. `web::AppState::new` names `Config` and `SqliteRepository` by value, so narrowing `config` or `infrastructure` would make those signatures private-in-public. Enforcement is therefore documentary: the repository policy proves that the tier table lists exactly the declared modules, that the hidden set is exactly the internal tier, and that no third caller of `init_tracing` or `install_panic_hook` appears.

The supported embedding is SQLite and the filesystem. Substituting storage under the HTTP layer is not supported; the capability ports exist for the host-adapter seam. `examples/embed.rs` is the executable definition of that embedding and is included verbatim as the crate's documentation example, because `cargo test --all-targets` compiles examples and excludes doctests.

Before 1.0 the crate version promises nothing about the Rust API. The durable contracts — the `.simple-blog` archive, the release manifest, the diagnostic JSON schema, the stable error codes, and the fixtures in `contracts/` — are versioned independently and remain the compatibility surface.

## Consequences

- The published documentation presents an application with a small embedding surface, not a framework with twelve namespaces.
- A new public module fails the repository policy until it is given a tier, so the surface cannot grow silently.
- A caller who ignores the tiers still compiles. What they lose is the promise and the documentation, and that is the whole of the guarantee.
- A host-adapter author has a named starting point: `application::ports`, `release`, `portable`, and `domain`, proven against the fixtures in `contracts/`.
- `examples/embed.rs` is load-bearing for compiling the library, so a change that breaks the embedding breaks the build on every target.
- Three properties make this crate a poor dependency, and they stay: `panic = "abort"` is a compile error, `build.rs` shells out to Bun through relative paths, and `env!("CARGO_PKG_VERSION")` is written into release manifests and `.simple-blog` archives, so an embedder's releases carry simple-blog's version rather than their own.
- Promoting a module out of `internal` or `reachable` is an architectural change and needs a superseding record.
