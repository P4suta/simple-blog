# ADR 0004: Content-addressed local media

- Status: Accepted
- Date: 2026-08-30

## Context

Media must work without object storage while resisting disguised files, path traversal, partial writes, and filename collisions.

## Decision

Only JPEG, PNG, WebP, and GIF are accepted after both signature sniffing and successful decode. Original bytes use a BLAKE3 filename and are retained. EXIF orientation is applied to generated responsive WebP variants. Files are written with create-new, sync, and atomic rename. HTTP serves only filenames referenced by media records.

## Consequences

- Duplicate bytes share one identity.
- Public media URLs are immutable and cacheable for one year.
- Metadata updates do not rename files.
- Local filesystem capacity and backup remain operator responsibilities.
