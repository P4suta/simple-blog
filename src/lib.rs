//! A writing-focused, single-owner CMS, normally run as the `simple-blog`
//! binary: one binary, one database file, one data directory.
//!
//! This crate is published so that the Core can be embedded in another Rust
//! program, and so that anyone writing a conforming host adapter can read the
//! interfaces the Core already speaks. It is deliberately **not a framework**:
//! there is no plugin API, no theme engine, and no marketplace. What the
//! project is for, and the promises it keeps, are written down in
//! [`docs/vision.md`](https://github.com/P4suta/simple-blog/blob/main/docs/vision.md).
//!
//! # The supported surface
//!
//! Every module below carries one tier, and this table is the normative list.
//! `supported` is an interface this project maintains for callers.
//! `reachable` is public only because a supported signature names it, and
//! carries no promise. `internal` is public only because `src/main.rs` is a
//! separate crate; it is absent from this documentation.
//!
//! | Module | Tier | What it is |
//! | --- | --- | --- |
//! | `application` | supported | Use-cases, and in `ports` the capability traits a host implements. |
//! | `cli` | internal | The binary's argument parsing and command bodies. |
//! | `cli_report` | internal | The human account of a failed command. |
//! | `config` | supported | The resolved configuration an embedder builds. |
//! | `domain` | supported | The vocabulary every port and every archive field is written in. |
//! | `i18n` | reachable | The embedded locale catalogues. |
//! | `infrastructure` | supported | The reference adapters: SQLite, WebAuthn, Markdown, clock, entropy. |
//! | `materialize` | reachable | Writing an active release out as plain files. |
//! | `observability` | internal | Process-global tracing setup for the binary. |
//! | `operations` | reachable | The operational services behind the subcommands. |
//! | `portable` | supported | The `.simple-blog` archive schema, format version 1. |
//! | `release` | supported | Immutable releases, and the store a host adapter implements. |
//! | `web` | supported | The HTTP application: [`web::AppState`] and [`web::router`]. |
//!
//! Dependencies point inward: `web` and `infrastructure` adapt external
//! systems, `application` coordinates use-cases, and `domain` owns business
//! rules and knows nothing of HTTP, SQLite or files.
//!
//! A host adapter starts at [`application::ports`], [`release::ReleaseStore`]
//! and [`portable::PortableSiteV1`]; what conformance means, and how it is
//! proven, is in
//! [`docs/public-surface.md`](https://github.com/P4suta/simple-blog/blob/main/docs/public-surface.md).
//!
//! # Stability
//!
//! Before 1.0 the crate version promises nothing about the Rust API: a minor
//! release may change any of it. The durable contracts are versioned
//! separately and outlive it — the `.simple-blog` archive, the release
//! manifest, the diagnostic JSON schema, and the stable error codes. Those are
//! the real compatibility surface, and changing one requires a decision
//! record.
//!
//! The library installs no process-global state. Setting up a tracing
//! subscriber and replacing the panic hook are the binary's business, done
//! once in `main`; an embedder keeps its own.
//!
//! # Embedding
//!
#![doc = "```no_run"]
#![doc = include_str!("../examples/embed.rs")]
#![doc = "```"]

#[cfg(panic = "abort")]
compile_error!("simple-blog requires panic=unwind so one failed request cannot terminate the CMS");

pub mod application;
#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod cli_report;
pub mod config;
pub mod domain;
mod durable_fs;
pub mod i18n;
pub mod infrastructure;
pub mod materialize;
#[doc(hidden)]
pub mod observability;
pub mod operations;
pub mod portable;
pub mod release;
pub mod web;
