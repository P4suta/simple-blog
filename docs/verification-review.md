# Critical review and verification record

Scope: ADR 0017, Rust Core, native and Cloudflare diagnostics, replacement
recovery, browser workflows, evidence, adversarial checks and release symbols.
Review began 2026-09-05 and continued 2026-09-06, starting from `7cd494a`.
Run manifests fingerprint dirty and untracked inputs; the starting commit alone
does not reproduce the working changes. Pending checks are not success claims.
The implementation is on `feat/diagnostic-review-and-recovery`, with its
first checkpoint at `e1e89a1`. No phase is globally closed while its mandatory
Linux/macOS and adversarial CI gates remain unexecuted.

## Design phase

| Pass | Evidence, correction and verification |
| --- | --- |
| 1: users/contracts | Doctor opened a migrating connection and implicitly probed writes. Split diagnosis and explicit `--probe-writes`. CLI tests compare complete file membership and bytes for old schema, corrupt DB and write probes. |
| 2: faults/recovery | Restore replaced individual files and cleanup could remove rollback material. Complete sibling staging, durable intent and retained previous installation now bridge replacement. Actual child processes are killed at all three activation boundaries; recovery and repetition are checked. |
| 3: assumptions | Immutable SQLite hid committed WAL rows in the first diagnostic design. Stable source/copy/source fingerprints and private SHM replace that assumption. Live-WAL regression confirms visible rows and untouched source DB/WAL/SHM; busy copies and hot journals are inconclusive. |
| 4: compatibility | Windows spelling variants and Unix symbolic aliases could bypass a lease. Resolve existing aliases and normalize Windows lock keys; Windows regression passed on stable and MSRV. Unix execution remains CI scope. |

Final local review A: compare replacement/recovery branches with the three real
process-death boundaries and retained original data; no new finding after the
special-file guard. Final local review B: inspect CLI lease ordering and compare
doctor's complete source-file snapshots, including live WAL; no new finding.
Unix-specific execution remains required before closing this phase.

## Implementation phase

| Pass | Evidence, correction and verification |
| --- | --- |
| 1: user failures | Expired fetch authentication followed a redirect to HTML and looked like a network failure. Return JSON 401 for JSON requests; retain HTML login redirects. The editor retains writing and shows the server inquiry ID, including saved-but-publication-pending state. |
| 2: privacy | Native and Worker traces contained preview paths and unknown exception messages. Use route templates, bounded methods, generated IDs and typed codes. Synthetic path/cookie/body/method/exception tests verify their values are absent. |
| 3: tracing/side effects | Background retries lost the request parent; probe sync/removal failures were not independently observable. Retain publication parent until success and test a real filesystem obstruction. Probe tests inject creation, sync and cleanup errors and assert injection fired. |
| 4: basic operation | The published editor lacked a visible no-JavaScript save control. Add localized noscript save; real browser tests cover reading, recovery login and HTML editing. |

Final local review A: recheck failure response contracts against actual three-engine
HTTP responses, saved content and inquiry IDs; no new finding. Final local review
B: trace nested and background operation identities, cancellation and independent
diagnostic checks against native/MSRV tests; no new finding. HTTP trace privacy
tests do not certify arbitrary local operator CLI error output for sharing.

## Verification phase

| Pass | Evidence, correction and verification |
| --- | --- |
| 1: real browsers | WebKit assertions could inspect the old document before navigation; Saved could be stale. Wait for actual POST responses and committed main-frame navigation. Repaired WebKit author flow passed twice. Chromium uses real virtual WebAuthn/IME; every engine authenticates through real recovery codes. |
| 2: evidence failure | PID report failure could orphan a child; child exit zero could mask failed log writes. Add bounded tree termination and distinct failure states. Unit tests inject pre-spawn, PID-report and mid-write errors, child exit 7 and timeout. |
| 3: blind spots | Pretty JSON bypassed linewise redaction and snapshot keys bypassed value redaction. Raw Playwright failures can quote credentials. Parse complete JSON, sanitize encoded keys, export allowlisted summaries/traces and test secret canaries. Browser evidence is isolated per run. |
| 4: measurements | SQL events from worker threads initially produced false zero counts. Timezone checks only asserted a hint; storage injection was not counted. Global SQL observer rejects inactive measurement; fixed time and three samples support comparison. Browser now saves in Tokyo, reloads in New York, and requires positive storage-fault count. |
| 5: adversarial controls | A build failure/timeout is not a detected mutation; empty shards must not pretend tests ran. Parse actual cargo-mutants outcomes, report not-applicable scope and test that shard union covers every selected target exactly once. A real search smoke run caught 2/2 mutants after a successful baseline. |

Final local review A: compare the corrected mutation parser with recorded tool
output and inspect the exercised runner/CI deadline failure paths; no new finding.
Final local review B: export twice, rehash every exported file and match all 29
latest browser cases to 29 server logs; private snapshots remain excluded. No
new finding. Full adversarial execution remains required before closing this phase.

## Release preparation phase

| Pass | Evidence, correction and verification |
| --- | --- |
| 1: diagnostics | Vendored OpenSSL objects referenced a removed compiler PDB. Restore its exact dependency-specific path; reject the initial shared-PDB approach that caused collisions. Windows release, PE/PDB GUID+age matching and standalone init/doctor passed. |
| 2: repeatability | Linux symbol re-export could overwrite the only remaining debug information. Export from an untouched original and test repeated/failed objcopy calls. Smoke runs the exported binary; artifact export verifies symbol hashes. Actual Linux objcopy remains CI scope. |
| 3: environment | Debug copies exhausted disk and left corrupt PDBs; missing generated cache metadata prevented cleanup. Keep logs/symbols, remove obsolete builds, use line-table verification and bound concurrent links. After cleanup/cache repair, every unchanged coverage floor passed. Failed runs remain failed. |

Final local review A: rebuilt Windows binary passes standalone init/doctor and
PE/PDB GUID+age pairing; no new finding. Final local review B: compare the exported
pair against MSVC dumpbin's RSDS record and the complete integrity index after
repeat export; no new finding. Actual Linux extraction and hosted CI remain required.

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
| `2026-09-05T15-44-09-272Z-0b931958` | Rust 1.96.0 format, lint and all-target/all-feature tests (1,024 property cases) passed before the final special-file guard. |
| `2026-09-05T16-01-04-517Z-a6ab2564` | Node 24 frontend/Worker checks, build, 29 browser scenarios and 10 process-recovery scenarios passed. |
| `2026-09-05T16-11-43-870Z-9f25ee55` | Three fixed-data performance samples passed with positive SQL counters and measured memory; the metrics artifact survived and is saved as `docs/performance-baseline.json`. |
| `2026-09-05T16-16-14-146Z-b52853b2` | Tools, workflows, policy, secrets, fresh build, 29 browser scenarios and 10 recovery scenarios passed after file-based server-log attachments. |
| `2026-09-05T16-35-10-345Z-b6bd8f9d` | Tools, workflows, policy and secrets passed with a private source snapshot and confirmed unchanged inputs. |
| `2026-09-05T17-00-25-665Z-44a72f27` | Final Rust 1.96 format, lint and all-target/all-feature tests passed after the special-file guard; inputs unchanged. |
| `2026-09-05T17-00-02-809Z-ef4d4442` | Final native format/lint/coverage (87.23% lines, 82.04% functions), frontend/Worker types and tests, assets, audit, policy, workflow and secret checks, Windows release/symbols/smoke, all 29 browser and 10 recovery cases and performance comparison passed; inputs unchanged. This precedes the final tool-only mutation/deadline refinements, which receive a separate run. |
| `2026-09-05T17-49-35-167Z-a0643ae2` | Final tools/CI refinements passed: format, all 35 tool regressions, actionlint, repository policy and source/history secret scans; inputs unchanged. |
| Final export review | 549 files (415,534,161 bytes) were exported twice and fully rehashed, including all 29 latest browser server logs and zero private source snapshots. `review-corrections/release-pe-headers.log` records the independent MSVC header inspection. Later metadata-only reruns add further manifests to the index. |

Additional evidence review: include build parallelism/toolchain/shard conditions
in run manifests and toolchain identity in performance comparisons. The common
performance profile builds the fixture first. This was found by comparing the
recorded rerun conditions with the actual Windows resource-limited invocation.

Further evidence review found that the runner's `performance.json` status report
overwrote the child's same-named metrics artifact (the metrics survived only in
stdout). Runner status now lives in reserved `_steps/`; a subprocess regression
proves that a same-named measurement artifact survives completion. Performance
is rerun before accepting its standalone baseline file.

Artifact review also found that in-memory Playwright server-log attachments had
no path and were therefore omitted by the exporter. Save the sanitized log as a
fixture attachment file, so exported browser evidence includes operation/request
correlation as well as test status. Node 24 browser revalidation covers this path.
Measurement validation now requires all three DB counters and dataset identity;
an empty counter object cannot pass through an empty iterator. Missing memory is
explicitly unavailable and makes the measurement step fail after preserving its
partial evidence. Regression tests cover each of these false-success cases.
The final reproducibility review found that hashes alone could not reconstruct
an intermediate dirty input, and edits during a run could invalidate its initial
fingerprint. New runs retain a private source snapshot and fail if any source
changes before completion. A regression preserves original bytes and detects
both capture-time and execution-time drift. Raw snapshots are not shared; CI
reproduction uses the recorded commit and input hashes.
Export review now excludes the entire private snapshot tree before discovering
browser reports and exports only manifest-listed symbol files. Canary regressions
cover both exclusions. Publication failure restores the previous validated export;
cleanup failure retains the newly published evidence and the old directory. Both
filesystem failures have injected, firing-asserted regressions.

The next independent fault review found that activation checked for symbolic
links but accepted FIFOs, so opening a malformed intent could wait indefinitely.
Reject special files before opening any activation input; a Unix regression
creates a real FIFO and first asserts rejection without opening it. Windows
revalidation cannot execute this Unix-only case; Linux/macOS CI remains required.
The same review reproduced a false-positive mutation verdict when outcome JSON
lacked its baseline. `target/mutation-baseline-red.log` records the failing
assertion. Require exactly one successful baseline and reject a mutant labeled
as baseline success; the tools suite rechecks these corrupt-evidence cases.

Comparing the real captured cargo-mutants 27.1.0 output with the adapter exposed
an additional mock/producer mismatch: the enum serializes `CaughtMutant` and
`MissedMutant`. Preserve a projection of the actual result in `scripts/fixtures`;
the regression first failed and then passed with the corrected parser. The
[pinned producer implementation](https://github.com/sourcefrog/cargo-mutants/blob/v27.1.0/src/outcome.rs)
confirms the protocol. Red/green logs are under `review-corrections/` in the
verification directory. This is adapter verification, not another full mutation run.

The CI timeout review found that a 30-minute child could outlive a 10-minute job,
preventing evidence export. Record each job's start before setup, match its
declared limit, reserve five minutes and refuse to start work after the budget.
Four tests cover setup time, a real timed-out child with surviving output, a
real common-entry `not_run`/exit-1 result, and all 11 evidence-producing jobs.
The latter initially failed on the previous workflow. Hosted hard cancellation
or artifact-service failure can still prevent upload and is not certified here.

The initial public-remote push was refused by automatic approval review. The user
subsequently explicitly authorized push, PR and merge, resolving that boundary.
[PR 11](https://github.com/P4suta/simple-blog/pull/11) now runs hosted verification.
The user prohibits release publication and tag creation; binary/symbol verification
is preparation only. Cloudflare conformance tests run under Node and do not certify
deployed Cloudflare resource behavior.

Hosted review pass 1 found an actual platform-schema blind spot: actionlint accepted
the workflow, but the GitHub runner rejected `timeout-minutes` inside composite
metadata before any verification could run. Job `101409500827` in run `34004636398`
records the error. A failing regression now checks every caller's bounded Node setup
and prohibits the unsupported composite key. Move Node setup into the workflows,
where the two-minute step deadline is supported; retain the global evidence reserve.
The [GitHub metadata reference](https://docs.github.com/en/actions/reference/workflows-and-actions/metadata-syntax)
defines the narrower composite-step contract. This failed run remains failed.

Windows cargo-fuzz failed linking instrumented `slug` as a cdylib (`main`
unresolved); this is not an executed fuzz success. Docker's daemon was unavailable.
Linux fuzz, the full critical mutation set, Unix-only regressions, macOS and Linux
symbol extraction were originally CI scope. Run `34004783489` now proves the
Linux/MSRV and macOS suites, Linux browser/recovery and Linux symbol extraction.
Full fuzz/mutation and corrected Windows/coverage jobs still require success.
The main ruleset now has all 15 versioned required checks; API readback matches
every submitted field, preserving existing protections and no bypass actors.
Windows stable and MSRV both passed locally after the last native guard.

Hosted review pass 2: Windows job `101409885967` selected Git Bash's incomplete
Perl (`Locale/Maketext/Simple.pm` missing). Select native Strawberry Perl through
`OPENSSL_SRC_PERL`, validate its IPC::Cmd module and record the actual Perl version
in run metadata. The paired symbol job also exposed that an absent symbol manifest
prevented independent logs from being exported. The new regression first failed
(`target/ci-incomplete-symbols-red.log`); an unvalidated symbol directory is now
omitted with stable `symbols.incomplete` and an explicit partial index, while safe
logs remain exportable. Invalid manifests or hash mismatches still fail closed.

Hosted review pass 3: CodeQL identified check-then-read races in source snapshots
and fuzz artifact reads. Use one verified regular-file descriptor, exact inode
identity, bounded reads and before/after metadata; a changed input fails rather
than becoming reproducible evidence. Tests assert size-growth and replacement
injection fired, no replacement bytes were read, and descriptors close on failure.
Unix additionally exercises an actual FIFO and symlink; Windows cannot certify it.

Coverage job `101409885804` and mutation shard 7 failed on unmutated observability
tests: concurrent temporary subscribers lost callsite/span events. Keep a single
subscriber with isolated current-thread capture, matching production's lifetime;
add eight runtimes with 128 total requests, asserting every completion and request
ID in its own buffer. Remove assertion interpolation of forbidden fixture values,
so a failed privacy test cannot echo the value it detected. The full Windows Rust
suite and tool/policy checks passed in `2026-09-06T01-58-55-478Z-09956b0f`, with
unchanged inputs (669.7 seconds for Rust). Hosted coverage/mutation must be rerun.

Independent artifact review rehashed every file in the downloaded Windows failure,
partial coverage, mutation-baseline failure and Linux symbol artifacts. LLVM
readobj reports matching GNU build IDs `52b0572d546e5bb0d16ad2e0ec8a657e91534714`
in the Linux executable and debug file, and the executable's GNU debuglink names
that file. These results validate evidence transport and pairing, not the failed
tests. No release, deployment or tag was created.

The next hosted run (`34005899373`, revision `fd5eab4`) passed the repaired Windows
tests/symbols, Linux coverage, MSRV, Linux/macOS, frontend, dependency, policy and
browser/recovery jobs. Mutation execution remains open. Its predecessor retained
all eight shards even when superseded: all three 60-second fuzz targets finished,
and partial mutation outcomes exposed missing search-limit, timestamp-shape,
release validation, authentication-success, object-integrity and incremental-reuse
assertions. Add public-contract regressions and a valid JSON activation record
padded across the 4,096-byte boundary. The ordinary Windows Rust/tool suite passed
in `2026-09-06T02-20-12-479Z-07735fce` before the subsequent review fixes below.

The diagnostic snapshot error-branch survivors prompted tests for every combination
of required database/optional sidecar and missing/permission-denied input. Collapse
duplicate error handling around a descriptor-based regular-file opener. A separate
fault review found the same metadata/open race could block on a substituted Unix
FIFO; nonblocking/no-follow opens and descriptor checks now reject it. Real Unix
FIFO/symlink replacement tests require hosted revalidation. No dependency version
changed: libc is now an explicit Unix dependency for the OS flag constants.

CodeRabbit review findings were checked against the actual consumers. A shared
JSON client omitted Accept, reproduced by a real Chromium request after clearing
authentication (`target/shared-client-red.log`). Add the header and a three-engine
regression. Restore/import now clean a failed staging area only after confirming
there is no activation intent; actual competing OS locks exercise that early
failure, while the existing process-death tests cover retained recovery inputs.

Evidence review fixes: redaction now buffers complete formatted JSON instead of
persisting its raw sensitive lines; incomplete structured output is evidence
failure. Raw gates include `key=` as well as the sanitizer's other query keys.
Server attachments expose only their classified locator, and exported server
records retain allowlisted IDs, codes and counts while omitting arbitrary message,
path and body fields. Unknown plain server lines get an explicit omission event.
Red/green reproductions are `target/privacy-review-*.log` and
`target/server-events-red.log`. Source snapshots accept regular files up to 64 MiB
per source file; this diagnostic safety bound does not limit product content.

The old PE matcher accepted an unrelated matching RSDS string. A new failing fixture
places a decoy outside the actual CodeView directory. Parse the DOS, COFF, optional
and section headers and only accept a single valid directory-referenced RSDS record.
The real Windows binary still matches GUID/age `8da7deb8cd7f764180b39b77a9b0bd22/1`
(`target/pe-directory-actual.log`). Git Bash discovery now supports custom Windows
locations and reports a stable prerequisite error when unavailable, excluding WSL
launchers without a companion sh.exe. Full revalidation is still required.

The combined local run `2026-09-06T02-37-33-744Z-7b41f872` passed format, lint,
all Rust targets/features, frontend/Worker/tools, dependency/workflow/policy/secret
checks, all 32 browser cases, process-death recovery and symbol pairing. Its input
fingerprint was unchanged. Windows-only execution does not certify Unix branches.

A further copy-path review found that fingerprinting used verified descriptors,
but copying reopened raw paths, and a growing input could extend a scan indefinitely.
Copy through the same regular-file opener and bound every hash/copy to the initial
length plus one byte; report growth or truncation as inconclusive. The new bounded
read regression failed before the fix (`target/diagnostic-bound-red.log`) and the
diagnostic unit suite then passed (`target/diagnostic-bound-green.log`).

Mutation execution was imbalanced because files, rather than mutants, were assigned
to shards. All eight producers now receive the same ordered scope and use the
[producer's round-robin partition](https://mutants.rs/shards.html), retaining their
baseline and all mutations. An actual cargo-mutants listing audit verifies exact
union, no duplication and counts differing by at most one. This is partition
verification only; full mutant execution remains required before phase closure.

Local run `2026-09-06T02-52-10-009Z-9f662ea7` passed all Rust and tooling checks but
failed lint on the new 64 KiB stack buffer. Allocate that buffer on the heap; add
the surviving empty-journal boundary case. Run `2026-09-06T02-55-21-154Z-bf28ba41`
then passed format, lint and every Rust target/feature with unchanged inputs.
The actual partition audit enumerated 935 mutants, distributed as seven shards
of 117 and one of 116, without duplicates or omissions.
