# ADR 0002: Canonical Markdown and request-time SSR

- Status: Accepted
- Date: 2026-08-30

## Context

Writers need portable source content, immediate publication, safe rendering, and predictable URLs without a static build pipeline.

## Decision

Markdown is canonical. A save derives sanitized HTML with Comrak and Ammonia and stores both in the same transaction. Public HTML is rendered on request. Visibility is a query predicate over status and `publish_at`; no publication job exists. Canonical permalinks have a trailing slash, and slug changes create a transactional redirect.

## Consequences

- Export does not reverse HTML into Markdown.
- Scheduled content becomes visible at request time.
- Rendering changes apply on the next content save unless a migration explicitly re-renders content.
- Public responses can use content version ETags without a page cache.
