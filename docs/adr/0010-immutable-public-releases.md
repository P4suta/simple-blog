# ADR 0010: Immutable public releases supersede request-time SSR

- Status: Accepted
- Date: 2026-09-02
- Supersedes: ADR 0002's request-time public rendering and publication timing

## Context

The canonical Markdown decision remains useful, but request-time public rendering couples every reader to the database, template engine, and application process. It also makes a multi-host implementation harder to migrate and harder to activate without a partially updated site. Scheduled entries still need to become visible at exact durable boundaries.

## Decision

Markdown remains canonical and safe derived HTML remains transactional editor state. The public site is compiled into a host-neutral release manifest and immutable content-addressed objects. A host adapter persists the complete graph, verifies it, and changes one active-release pointer with compare-and-swap semantics. Readers resolve only that pointer and graph; an incomplete build is never visible.

The publication clock is durable. Saving public state increments a public revision, scheduled boundaries advance it exactly once, and publisher failures retain the last complete release while scheduling a retry. Admin, passkey ceremonies, likes, views, and publication coordination remain dynamic.

The release format and resolver behavior are versioned contracts shared by native and hosted adapters. A native deployment may materialize a release to ordinary files, but Git is never an authoring or deployment requirement.

## Consequences

- Public reads do not require request-time SQL or templates.
- Activation is one visible pointer write after full graph verification.
- Builds can reuse unchanged objects without introducing a product-level content quota.
- A failed compiler, upload, or scheduler leaves the previous site intact and diagnosable.
- Template changes require a release build; content changes trigger publication through the Core.
