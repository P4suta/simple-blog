# ADR 0007: Reproducible verification is a release contract

- Status: Accepted
- Date: 2026-08-30

## Context

Fast delivery has less value than a failure that can be reproduced, localized, and repaired safely. A single passing test suite cannot expose compiler drift, platform differences, partial I/O, dependency risk, or stale generated assets.

## Decision

Verification is part of the product boundary. Time is injected, durable multi-step work exposes failure seams, and panics unwind to a sanitized request failure. CI independently gates the declared MSRV, current Rust on Linux and macOS, property tests, line coverage, dependency policy, reproducible frontend assets, and a Node-free release-binary smoke test. Coverage floors may only stay constant or increase unless a superseding ADR explains the loss.

## Consequences

- A flaky or non-diagnostic check is a defect, not tolerated noise.
- Failure-path tests precede fixes and include compensation assertions for partial writes.
- Stable diagnostic codes and correlation IDs are tested as compatibility contracts.
- New tools and invariants are added when they shorten reproduction or fault isolation, even when they slow delivery.
