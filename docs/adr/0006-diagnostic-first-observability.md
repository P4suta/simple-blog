# ADR 0006: Diagnostic-first observability

- Status: Accepted
- Date: 2026-08-30

## Context

Slow delivery is acceptable; an undiagnosable failure is not. Authentication capabilities also make indiscriminate logging unsafe.

## Decision

Every phase treats tests and failure evidence as acceptance criteria. HTTP requests receive a server-generated correlation ID and structured spans containing only method, path, status, and latency. Query strings, cookies, credentials, CSRF values, and request bodies are never logged. Operational checks expose actionable failures through `doctor`, while user-facing errors remain non-sensitive.

## Consequences

- Failures can be correlated without disclosing bearer capabilities.
- New behavior includes failure-path and diagnostic assertions before implementation.
- Diagnostic seams are maintained at adapter boundaries rather than added ad hoc inside domain rules.
- Additional verification work takes precedence over delivery speed.
