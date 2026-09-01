# ADR 0011: Portable Core and conforming host adapters

- Status: Accepted
- Date: 2026-09-02

## Context

A hosted CMS becomes another lock-in mechanism if its database model, authentication identity, generated output, or private storage cannot leave the provider intact. Git-backed static-site workflows are not an acceptable migration interface for ordinary writers.

## Decision

Domain rules, publication, compilation, and portable schemas belong to the Core. Filesystem, SQLite, Cloudflare, and future providers implement narrow capability ports. Conformance is behavioral: adapters consume the same domain-state and release-resolution fixtures and may not reinterpret lifecycle states or routes.

`.simple-blog` format version 1 is the complete migration unit. It carries canonical content and revisions, redirects, settings, navigation, media bytes and metadata, engagement totals, publication state, owner passkeys, and recovery capabilities. Sessions, setup tokens, and in-progress ceremonies are ephemeral and never exported. Archives use deterministic ordering, checksums, bounded parsing, strict schemas, and safe paths.

Import first validates the whole package, rebuilds derived HTML and a public release with the destination Core, runs integrity diagnostics, and only then atomically replaces the destination. A forced native import preserves the replaced installation in a recoverable sibling directory. Unknown versions or fields fail closed; adapters never silently discard state.

The canonical origin is retained in version 1. Moving the same custom domain between conforming hosts therefore preserves public URLs and the WebAuthn relying-party identity as well as content.

## Consequences

- Host migration is a CMS operation, not a source-control exercise.
- A new adapter is incomplete until it passes the shared fixtures and round-trip migration contract.
- Adapter-specific caches and sessions may be rebuilt or invalidated; durable user state may not be omitted.
- Changing the site's domain is a separate future migration protocol, not an accidental side effect of host migration.
