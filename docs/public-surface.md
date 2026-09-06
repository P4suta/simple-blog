# The public surface

What this project maintains for people outside it, and what it does not. Three
readers are meant here: someone who ran `cargo add simple-blog`, someone
writing a host adapter, and a contributor looking for the boundary before
moving something across it.

`src/lib.rs` is the normative statement. This document explains it, and adds
the part that is not Rust: the fixtures a non-Rust adapter conforms to.

## Three tiers

Every module carries exactly one tier, and the table in `src/lib.rs` is the
list `tests/repository_policy.sh` checks.

**Supported.** An interface this project maintains for callers.
`application` (its `ports` are the capability traits a host implements),
`config`, `domain`, `infrastructure`, `portable`, `release`, and `web`.

**Reachable.** Public only because a supported signature names it, and
carrying no promise: `i18n` (`AppBuildError::Translation` names its error),
`materialize` (generic over the supported release seam), and `operations`
(`AppState::create_backup` returns its error).

Hiding a module whose types appear in a supported signature would make the
documentation worse, not the surface smaller: the linked type would render as
plain text with no variants. So reachability is stated rather than concealed.

**Internal.** Public only because `src/main.rs` is a separate crate that has
to call into the library: `cli`, `cli_report`, and `observability`. Each
carries `#[doc(hidden)]` and is absent from the published documentation.

Enforcement is documentary, not compiler-enforced. `web::AppState::new` names
`Config` and `SqliteRepository` by value, so narrowing `config` or
`infrastructure` to `pub(crate)` would make those signatures
private-in-public. A caller who ignores the tiers still compiles; what they do
not get is a compatibility promise or documentation. That is the whole of the
guarantee, and ADR 0018 says so.

## Stability

Before 1.0 the crate version promises nothing about the Rust API: a minor
release may change any part of it.

The durable contracts are versioned separately and outlive the crate version.
They are the real compatibility surface, and changing one needs a decision
record:

- the `.simple-blog` archive, format version 1 (ADR 0011);
- the release manifest and its content-addressed objects (ADR 0010);
- the diagnostic JSON schema and the stable error codes (ADR 0006);
- the fixtures in [`contracts/`](../contracts).

## Embedding

The supported embedding is SQLite and the filesystem: build a
`simple_blog::config::Config`, open a
`simple_blog::infrastructure::sqlite::SqliteRepository`, hand both to
`simple_blog::web::AppState::new`, and serve `simple_blog::web::router`.
`examples/embed.rs` is that program, and it is compiled by every CI target, so
it cannot drift from the code it demonstrates.

Substituting storage underneath the HTTP layer is **not** supported.
`AppState` takes a concrete repository on purpose. The capability ports exist
for the host-adapter seam described below, not for swapping a database out
from under the admin.

The library installs no process-global state. `observability::init_tracing`
and `install_panic_hook` replace a process-wide subscriber and the panic hook;
only `src/main.rs` calls them, the repository policy proves there is no third
caller, and an embedder keeps its own.

## Writing a conforming host adapter

The vision names *a second conforming host adapter* as still owed. Conforming
means passing every fixture in `contracts/` and a round-trip migration, not
resembling the existing one.

There are two shapes, and the one that exists today is the second.

**In process, in Rust.** Implement `simple_blog::release::ReleaseStore` and
`simple_blog::release::ReleaseReader` — `ReleaseBackend` is both in one bound,
because Rust has no `dyn A + B` — and, for a complete host, the traits in
`simple_blog::application::ports`. `FixtureStore` in
`tests/cross_adapter_spec.rs` is a working template.

**Out of process, in any language.** `adapters/cloudflare` is TypeScript and
implements none of the Rust traits: it speaks HTTP to a Core service binding,
and conformance is behavioural. The Rust code is then the reference
implementation, not a dependency. Its README states the routes such a Core
must answer.

Four obligations, whichever shape:

1. **Route resolution** identical to `release::ReleaseResolver`, proven
   against `contracts/release-resolution-v1.json`. Both languages read that
   file today, and the repository policy fails if either stops.
2. **Domain lifecycle** identical to `domain::hosting`, proven against
   `contracts/domain-registration-v1.json`.
3. **A `.simple-blog` round trip** that drops no durable state. The archive
   carries content and revisions, redirects, settings, navigation, media,
   engagement, publication state, trash, owner passkeys and recovery
   capabilities. Sessions and in-progress ceremonies are ephemeral and are
   never exported. ADR 0011 is the record; `simple_blog::portable` is the
   schema.
4. **Diagnostics**: the stable error codes, the request identifier on every
   trace, and the redaction rules. ADR 0006 is the record. A code is read from
   the typed error — `web::WebError::diagnostic_code` and
   `application::publication::PublicationServiceError::code` — never
   reconstructed from a message.

## Not extension points

There is one theme, restyled with a stylesheet, and no marketplace. The
templates, the default stylesheet, the locale catalogues in
`simple_blog::i18n`, the Markdown pipeline, and the admin routes are not
interfaces. A change to any of them is a product change, decided in
[`docs/vision.md`](vision.md), not a compatibility event.

The `cli` and `cli_report` modules are the binary's own; `simple_blog::cli`
returns `anyhow::Result`, which is a signature for a program, not for a
library.

See [ADR 0018](adr/0018-declared-public-surface.md) for the decision and its
consequences.
