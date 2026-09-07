# ADR 0017: Verifiable diagnostics and repeated critical review

- Status: Accepted
- Date: 2026-09-05

## Context

Read-only diagnosis previously opened a migrating SQLite connection. Capability
paths reached request logs, browser checks lacked real engines, and restore
could delete its rollback data after a partial failure. Passing unit tests did
not establish that these operational paths were safe or that evidence survived.

## Decision

Every delivery phase includes at least three evidence-driven review passes:
user behavior and requirements; faults, privacy and compatibility; and the
ability of the verification itself to detect mistakes. Each finding receives
a reproduction, a correction and verification. Two final review passes without
new actionable findings and all required checks passing close a phase. Record
not-run, inconclusive and timeout outcomes separately from success.

Diagnostics use allowlisted fields. HTTP paths identify route templates, never
capabilities or arbitrary user paths. Unclassified exception messages do not
belong in operational traces. Generated correlation identifiers connect requests,
operations and publication builds without trusting client-provided identifiers.

The default doctor makes a stable private copy of the DB and WAL, verifying the
whole source/copy/source file sets. SQLite opens only that copy through a
read-only, non-migrating connection. Private SHM coordination is permitted;
investigated DB/WAL/SHM bytes and membership remain untouched.
Independent filesystem and release checks run even when database inspection
fails. SQLite WAL coordination is explicitly distinguished from durable data:
the connection must not migrate, checkpoint, recover or change persisted data.
An unsafe or unavailable read is reported as inconclusive, not healthy. Active
installations must never be opened with SQLite's immutable assertion.
Neither are copied WAL databases: immutable mode can hide uncheckpointed data.
Filesystem write checks require `doctor --probe-writes`; creation, synchronization
and cleanup failures are observable. Existing diagnostic JSON fields and exit
codes remain compatible; check codes, hints and inspection scope are additive.

Verification records revision, tool versions, seed, commands, duration and exit
status. Failure and timeout preserve available evidence. Artifact collection and
fault injection are themselves tested. Only disposable synthetic installations
are used by browser and crash tests. Shared evidence must pass secret inspection.
CI evidence expires after 30 days; release symbol identity is tied to its binary.

Restore and portable import prepare a complete sibling directory, synchronize
it, persist an activation intent, retain the entire previous installation, then
install the replacement. Ordinary CLI commands hold an OS installation lease
and recover any interrupted activation before loading configuration. Doctor only
reports a pending intent. Recovery is idempotent, validates confined sibling
names, refuses ambiguous state and never deletes previous data. Library callers
must hold the installation lease while accessing or replacing the installation.
Windows directory synchronization remains an explicitly reported best-effort
boundary; process-crash tests are not claims about hardware power-loss behavior.

## Consequences

- This supersedes ADR 0006's use of raw request paths and clarifies the doctor
  boundary in ADR 0007. Normal application startup still performs safe migrations.
- Fuzz and mutation checks supplement contracts and recovery tests. A timed-out
  mutant is inconclusive and never counted as proof of detection.
- Diagnostics and verification remain development priorities even when delivery
  takes longer. Documentation records evidence rather than asserting unrun checks.
