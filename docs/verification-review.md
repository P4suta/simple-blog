# Critical review and verification record

Scope: ADR 0017, Rust Core, native and Cloudflare diagnostics, replacement
recovery, browser workflows, evidence, adversarial checks and release symbols.
Review began 2026-09-05 and continued 2026-09-06, starting from `7cd494a`.
Run manifests fingerprint dirty and untracked inputs; the starting commit alone
does not reproduce the working changes. Pending checks are not success claims.

## Design phase

| Pass | Evidence, correction and verification |
| --- | --- |
| 1: users/contracts | Doctor opened a migrating connection and implicitly probed writes. Split diagnosis and explicit `--probe-writes`. CLI tests compare complete file membership and bytes for old schema, corrupt DB and write probes. |
| 2: faults/recovery | Restore replaced individual files and cleanup could remove rollback material. Complete sibling staging, durable intent and retained previous installation now bridge replacement. Actual child processes are killed at all three activation boundaries; recovery and repetition are checked. |
| 3: assumptions | Immutable SQLite hid committed WAL rows in the first diagnostic design. Stable source/copy/source fingerprints and private SHM replace that assumption. Live-WAL regression confirms visible rows and untouched source DB/WAL/SHM; busy copies and hot journals are inconclusive. |
| 4: compatibility | Windows spelling variants and Unix symbolic aliases could bypass a lease. Resolve existing aliases and normalize Windows lock keys; dedicated regressions added. Newest alias change awaits final platform verification. |

Final review A: pending platform verification. Final review B: pending.

## Implementation phase

| Pass | Evidence, correction and verification |
| --- | --- |
| 1: user failures | Expired fetch authentication followed a redirect to HTML and looked like a network failure. Return JSON 401 for JSON requests; retain HTML login redirects. The editor retains writing and shows the server inquiry ID, including saved-but-publication-pending state. |
| 2: privacy | Native and Worker traces contained preview paths and unknown exception messages. Use route templates, bounded methods, generated IDs and typed codes. Synthetic path/cookie/body/method/exception tests verify their values are absent. |
| 3: tracing/side effects | Background retries lost the request parent; probe sync/removal failures were not independently observable. Retain publication parent until success and test a real filesystem obstruction. Probe tests inject creation, sync and cleanup errors and assert injection fired. |
| 4: basic operation | The published editor lacked a visible no-JavaScript save control. Add localized noscript save; real browser tests cover reading, recovery login and HTML editing. |

Final review A: pending newest regression run. Final review B: pending.

## Verification phase

| Pass | Evidence, correction and verification |
| --- | --- |
| 1: real browsers | WebKit assertions could inspect the old document before navigation; Saved could be stale. Wait for actual POST responses and committed main-frame navigation. Repaired WebKit author flow passed twice. Chromium uses real virtual WebAuthn/IME; every engine authenticates through real recovery codes. |
| 2: evidence failure | PID report failure could orphan a child; child exit zero could mask failed log writes. Add bounded tree termination and distinct failure states. Unit tests inject pre-spawn, PID-report and mid-write errors, child exit 7 and timeout. |
| 3: blind spots | Pretty JSON bypassed linewise redaction and snapshot keys bypassed value redaction. Raw Playwright failures can quote credentials. Parse complete JSON, sanitize encoded keys, export allowlisted summaries/traces and test secret canaries. Browser evidence is isolated per run. |
| 4: measurements | SQL events from worker threads initially produced false zero counts. Timezone checks only asserted a hint; storage injection was not counted. Global SQL observer rejects inactive measurement; fixed time and three samples support comparison. Browser now saves in Tokyo, reloads in New York, and requires positive storage-fault count. |
| 5: adversarial controls | A build failure/timeout is not a detected mutation; empty shards must not pretend tests ran. Parse actual cargo-mutants outcomes, report not-applicable scope and test that shard union covers every selected target exactly once. A real search smoke run caught 2/2 mutants after a successful baseline. |

Final review A: pending full browser/deep verification. Final review B: pending.

## Release preparation phase

| Pass | Evidence, correction and verification |
| --- | --- |
| 1: diagnostics | Vendored OpenSSL objects referenced a removed compiler PDB. Restore its exact dependency-specific path; reject the initial shared-PDB approach that caused collisions. Windows release, PE/PDB GUID+age matching and standalone init/doctor passed. |
| 2: repeatability | Linux symbol re-export could overwrite the only remaining debug information. Export from an untouched original and test repeated/failed objcopy calls. Smoke runs the exported binary; artifact export verifies symbol hashes. Actual Linux objcopy remains CI scope. |
| 3: environment | Debug copies exhausted disk and left corrupt PDBs; missing generated cache metadata prevented cleanup. Keep logs/symbols, remove obsolete builds, use line-table verification and bound concurrent links. After cleanup/cache repair, every unchanged coverage floor passed. Failed runs remain failed. |

Final review A: pending latest release/CI. Final review B: pending.

## Evidence checkpoints

Paths are beneath `target/verification/`. Export via
`node scripts/collect-evidence.mjs`; the safe integrity index is
`target/shareable/evidence.json`. Raw contexts, authentication state and opaque
trace resources are excluded. Sanitized replay may lack styles/images; action
timing, DOM metadata and network structure remain available.

| Run | Result and limits |
| --- | --- |
| `2026-09-05T13-29-43-423Z-a782ad48` | Windows release build (744.5 s), symbols and smoke passed before later lock/log refinements. |
| `2026-09-05T13-30-05-860Z-639c3909` | 10 CLI process-recovery scenarios passed. Browser 27/28: WebKit race failed, then was repaired and separately passed twice. This run remains failed. |
| `mutation-review2/mutants.out/outcomes.json` | Search escape: baseline passed, 2 mutants caught, no timeout. Uses previously built vendored OpenSSL locally; not proof of the full critical set. |
| `2026-09-05T14-33-31-305Z-e3e0d486` | Cargo dependency policy passed. |
| `2026-09-05T14-36-51-541Z-057731a8` | Frontend/Worker types and tests, tool tests, asset reproduction, audit, actionlint, source/history secret scans and policy passed. |
| `2026-09-05T14-49-41-312Z-d2cef960` | All-target/all-feature tests and coverage passed: lines 87.23%, functions 82.07%; region/file floors unchanged. Later alias/method refinements require a final run. |
| `2026-09-05T14-52-40-570Z-77c3813e` | Tool tests, sharded workflow lint and repository policy passed. |
| `2026-09-05T14-49-41-137Z-74c777bf` | Lint, current browser build, all 29 browser scenarios, 10 process-recovery scenarios and three fixed-time performance samples passed, including real Tokyo/New York conversion and counted storage injection. |
| `2026-09-05T15-28-05-948Z-09c2fe1f` | Latest native format, lint and complete coverage passed after method/alias refinements. |
| `2026-09-05T15-28-09-964Z-6e6cee92` | Latest Windows release, matching symbols and standalone smoke passed. |
| `2026-09-05T15-33-41-245Z-764bcd68` | Source/history secret scan, workflows, policy, both frontend type/test suites, tooling, asset and audit passed before the final metadata-only refinement. |

Additional evidence review: include build parallelism/toolchain/shard conditions
in run manifests and toolchain identity in performance comparisons. The common
performance profile builds the fixture first. This was found by comparing the
recorded rerun conditions with the actual Windows resource-limited invocation.

Further evidence review found that the runner's `performance.json` status report
overwrote the child's same-named metrics artifact (the metrics survived only in
stdout). Runner status now lives in reserved `_steps/`; a subprocess regression
proves that a same-named measurement artifact survives completion. Performance
is rerun before accepting its standalone baseline file.

Windows cargo-fuzz failed linking instrumented `slug` as a cdylib (`main`
unresolved); this is not an executed fuzz success. Docker's daemon was unavailable.
Linux fuzz, the full critical mutation set, MSRV, macOS and Linux symbol extraction
remain CI scope. Versioned ruleset edits have not changed live GitHub settings.
