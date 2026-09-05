# Development contract

Prioritize users' data, clear failure states, reproducibility, investigation and
recovery over delivery speed. Make ordinary implementation decisions without
asking the user to choose. Read CONTRIBUTING.md and relevant accepted ADRs.

For design, implementation, testing and release preparation, perform at least
three critical review passes: (1) users and requirements, (2) faults, concurrency,
privacy and compatibility, (3) weaknesses in the verification and evidence.
For each actionable finding record evidence, a failing test or reproduction,
the fix and its verification in docs/verification-review.md. Reconsider your
own assumptions. Continue until two consecutive final reviews find no new
actionable issues and all required checks pass. Never relabel an unrun,
inconclusive, skipped or timed-out check as successful.

Use `node scripts/verify.mjs` as the common local/CI entry. Preserve existing
lint and coverage floors. Replay parser counterexamples in ordinary tests;
run `node scripts/deep-checks.mjs pr` for parser or important decision changes.
Timeouts in mutation testing do not count as detections. Use only disposable
installations and synthetic data; export only vetted evidence with
`node scripts/collect-evidence.mjs`. Never commit credentials or private traces.

Doctor must not migrate, recover, checkpoint or alter the investigated files.
Write probes require `--probe-writes`. HTTP traces use route templates and
allowlisted fields; raw paths, queries, cookies, bodies and unclassified
exceptions do not belong in logs. Keep prior data and public releases available
across failed replacements. Document architectural changes in a new ADR.

Monitor free storage during development. Remove obsolete reproducible build
outputs, package download caches and disposable fixtures when no process uses
them. Verify every resolved cleanup target stays within this workspace. Preserve
source changes, recovery directories, review evidence and matching release
symbols. Record environment or space failures as failures, then rerun after repair.
