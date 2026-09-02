# ADR 0007: Reproducible verification is a release contract

- Status: Accepted
- Date: 2026-08-30

## Context

Fast delivery has less value than a failure that can be reproduced, localized, and repaired safely. A single passing test suite cannot expose compiler drift, platform differences, partial I/O, dependency risk, or stale generated assets.

## Decision

Verification is part of the product boundary. Time is injected, durable multi-step work exposes failure seams, and panics unwind to a sanitized request failure. CI independently gates the declared MSRV, current Rust on Linux, macOS, and Windows, property tests, line coverage, dependency policy, reproducible frontend assets, and a Node-free release-binary smoke test. Coverage floors may only stay constant or increase unless a superseding ADR explains the loss.

Atomic file replacement flushes file contents before installation and synchronizes the containing directory on platforms with a documented directory-flush contract. Windows documents `FlushFileBuffers` for writable file handles but not directory handles, so the Windows boundary validates the directory and emits a debug trace instead of promising or invoking an unsupported flush operation. Cross-platform CI exercises this deliberately weaker, explicit guarantee.

The Windows build vendors the OpenSSL implementation required by the WebAuthn dependency. This deliberately trades build time for a reproducible binary that does not depend on a developer machine or CI runner having a matching global OpenSSL/vcpkg installation.

Backup manifests and archive diagnostics use `/`-separated portable entry identities on every operating system. Restore validates those identities before joining them to disk paths and reports exact missing or unexpected entries. Human-facing recovery errors render filesystem paths with their native display form so Windows paths remain directly usable instead of appearing debug-escaped.

## Consequences

- A flaky or non-diagnostic check is a defect, not tolerated noise.
- Failure-path tests precede fixes and include compensation assertions for partial writes.
- Stable diagnostic codes and correlation IDs are tested as compatibility contracts.
- New tools and invariants are added when they shorten reproduction or fault isolation, even when they slow delivery.
