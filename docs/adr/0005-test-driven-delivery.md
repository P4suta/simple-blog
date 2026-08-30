# ADR 0005: Test-driven delivery

- Status: Accepted
- Date: 2026-08-30

## Context

The CMS combines persistence, authentication, HTTP, image processing, and operational workflows. Architectural boundaries erode if only end-state behavior is tested.

## Decision

Every behavior change follows Red, Green, Refactor. Pure invariants use unit specifications; port implementations use integration specifications; security and user workflows use HTTP or browser-level specifications. A phase is complete only when its new tests and the full existing suite pass.

## Consequences

- Tests define observable contracts, not private implementation details.
- Refactoring cannot weaken previously accepted behavior.
- Defects first receive a reproducing test.
- Build, lint, security, and release smoke checks remain final gates rather than substitutes for TDD.
