---
date: 2026-11-23
status: accepted
---

# Preserve stored and rendered timestamps

## Context

Chrono supplies the UTC values in SQLite, portable archives, and public feeds.
Its maintenance ended in [RUSTSEC-2026-0543](https://rustsec.org/advisories/RUSTSEC-2026-0543.html).
[Site-local time](0015-site-local-time.md) requires identical output on every host.

## Decision

Represent archive leap seconds with Unix seconds and nanoseconds.
Use the `time` crate for calendar arithmetic and formatting.
Preserve the UTC suffix and fractional precision of each persisted format.
Use the [bundled time zone database](https://docs.rs/tzdb/) to resolve site zones consistently on every host.
Keep the schema and archive versions.

## Alternatives

The SQLx encoder for `time::OffsetDateTime` changes the SQLite UTC suffix.
Mixing suffixes changes comparisons for equal instants stored before and after the update.
[SQLx documents the SQLite encoding](https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html).
Jiff converts a stored leap second to the preceding second during parsing.
[Jiff documents the conversion](https://docs.rs/jiff/latest/jiff/struct.Timestamp.html#leap-seconds).

## Consequences

The storage adapters encode and decode timestamps.
Reconsider the adapter when SQLx supports the existing formats.

## Confirmation

Read SQLite rows from the previous release and compare writes, archive bytes, and publication ordering across the update.
Check fractional seconds, explicit offsets, UTC midnight, zone aliases, and both sides of a daylight saving transition.
