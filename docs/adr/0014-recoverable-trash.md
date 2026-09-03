# ADR 0014: Recoverable trash is durable content state

- Status: Accepted
- Date: 2026-09-03

## Context

A writer needs to remove a piece from the site, and sometimes needs it back. A hard delete makes the second need impossible and turns one mis-click into data loss, which contradicts the promise that nothing written is lost. Unpublishing is not the same thing: an unpublished draft still occupies the dashboard and the writer's attention.

## Decision

Content carries an optional `deleted_at` timestamp. Trashed content leaves every public query and the publication clock in the same transaction, keeps its slug reserved and its media referenced, and refuses edits until it is restored. Restoring returns it to exactly the publication state it had before. Permanent deletion applies only to trashed content and cascades to revisions, tags, redirects, engagement, and the search row.

Trash is durable state and therefore part of the `.simple-blog` migration unit. The field is optional and omitted when empty, so an archive without trashed content is byte-identical to one written before this decision. An archive that does contain trashed content is rejected by a binary that predates the field, which is the fail-closed behaviour ADR 0011 already prescribes for unknown fields; the format version stays at 1 because no existing field changed meaning.

## Consequences

- Deleting is reversible until the writer explicitly empties the trash.
- Media referenced only by trashed content survives garbage collection.
- A trashed scheduled entry does not hold the publication clock; restoring it re-arms the boundary.
- Host adapters that consume `.simple-blog` archives must accept the optional field before they can import sites with a non-empty trash.
