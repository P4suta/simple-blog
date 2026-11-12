---
date: 2026-11-12
status: proposed
---

# Select a patched database dependency

## Context

SQLx 0.9.0 excludes libsqlite3-sys 0.39, which bundles the SQLite fix for [RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459).
The mandatory dependency gate fails.
Upstream [PR 4502](https://github.com/launchbadge/sqlx/pull/4502) widens the range and passed its platform checks.

## Decision

Pin SQLx to merge commit `10462fa4414c6c7f1ed1d39bb31678e3b726878c` and select libsqlite3-sys 0.39.1 in Cargo.lock.
Use Cargo's [registry patch](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html) for independent registry version and Git digest lookups.
Keep the features and Rust 1.96.

## Alternatives

Waiting for a release leaves the mandatory gate failing.
System SQLite makes the fix depend on each installation's library.
A local driver fork adds maintenance.

## Consequences

The pin includes upstream changes after SQLx 0.9.0 and requires the project's compatibility checks.
Return to a registry release when its dependency range includes the fix.

## Confirmation

Require formatting, Clippy, database and recovery tests, and dependency checks.
The common macOS checks pass, including database, recovery, and dependency checks.
Owner acceptance remains pending.
