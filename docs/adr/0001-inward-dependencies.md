# ADR 0001: Inward dependencies through capability ports

- Status: Accepted
- Date: 2026-08-30

## Context

HTTP, SQLite, WebAuthn, image codecs, and the filesystem change for different reasons. Business rules must remain testable without any of them.

## Decision

Dependencies point inward: `domain` contains pure rules, `application` owns use cases and capability-oriented ports, and `infrastructure` plus `web` are adapters. Transactions belong to repository implementations when one use case has multiple durable effects. Public templates receive only a typed `ThemeContext`.

## Consequences

- Domain tests have no I/O.
- Adapters can be replaced without changing use cases.
- A port is added only for a concrete capability; no generic service locator or repository bag is allowed.
- Some adapter composition remains in the binary/application state.
