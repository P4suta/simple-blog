# ADR 0008: No unscoped lint suppression

- Status: Accepted
- Date: 2026-08-30

## Context

Broad lint suppression hides both current design debt and new findings introduced by compiler upgrades. Enabling documentation-style lints globally would instead create boilerplate for binary-internal APIs.

## Decision

`allow` attributes are forbidden by the lint configuration. An `expect` attribute is permitted only at the narrowest scope, with a reason, when the reported pattern is an intentional invariant that cannot be expressed more directly. The project enables `all`, `nursery`, and an explicit failure-oriented lint set; it does not enable lint groups merely to suppress their non-applicable members.

## Consequences

- Warnings are repaired or the lint policy is changed explicitly; they are not hidden locally.
- Unfulfilled or unexplained lint expectations fail CI.
- Lint policy remains reviewable in `Cargo.toml` without generating documentation solely to satisfy a style check.
