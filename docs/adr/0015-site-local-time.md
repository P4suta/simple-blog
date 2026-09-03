# ADR 0015: The site has a local time

- Status: Accepted
- Date: 2026-09-03

## Context

Every public date was rendered from `DateTime<Utc>` with `%Y-%m-%d`, so a piece published at 08:00 in Tokyo appeared under the previous day, and a Japanese site showed `2026-09-03` where a reader expects 2026年9月3日. The site is single-owner and single-locale, so it has one place; the writer's browser knows that place at setup time, and nobody should have to configure it.

## Decision

`site_settings.timezone` holds an IANA zone name, defaulting to `UTC`. The name is validated against the time zone database embedded in the binary, so every host adapter renders identical bytes for the same snapshot. Public dates use locale-specific patterns from the translation catalogs, rendered in the site zone; machine timestamps stay RFC 3339 with the site's offset (feeds may keep UTC). The setup ceremony offers the browser's zone and the Core adopts it exactly once, while the stored zone is still `UTC`. An author name and a one-slot backup of the stylesheet ride the same migration.

The three settings are optional in the `.simple-blog` format and omitted at their defaults, so an archive written before this decision is byte-identical to one written after it; the format version stays 1. An older binary rejects an archive that carries them, which is the fail-closed behaviour ADR 0011 prescribes.

## Consequences

- Changing the zone re-dates every page and produces a new release identity.
- Time zone rule updates arrive with dependency updates, never from the host's own database.
- Admin timestamps remain in the writer's browser zone; the editor names the site zone next to the scheduled instant so the two are never confused.
