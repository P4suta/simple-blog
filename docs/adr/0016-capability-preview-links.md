# ADR 0016: Preview links are short-lived bearer capabilities

- Status: Accepted
- Date: 2026-09-03

## Context

A writer wants a second pair of eyes on a piece before it is public, and wants to see the future page in the real theme rather than an admin-styled imitation. The public site is an immutable release that drafts never enter, and the owner session is the only credential the system has.

## Decision

The owner previews any piece through the public templates at a session-gated admin route that renders the current database state with the current stylesheet; the response allows same-origin framing so the editor can show it beside the text. A shareable link is a 256-bit bearer capability, stored only as a hash, valid for seven days, revocable per piece, and deleted with the piece. The shared page carries `noindex`, omits reader interactions, and is served without caching. Links are ephemeral like sessions: they are not part of the `.simple-blog` migration unit, and an import discards them.

## Consequences

- Unpublished stylesheet edits are visible to link holders, since the preview uses the current settings.
- A leaked link exposes one piece for at most seven days and can be revoked at once.
- The framed preview is the only admin response that relaxes `frame-ancestors`; every other page keeps forbidding framing.
