# simple-blog

[![CI](https://github.com/P4suta/simple-blog/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/P4suta/simple-blog/actions/workflows/ci.yml)

A writing-focused, single-owner CMS with a host-neutral Rust Core.

> v0.1 is under active development and is not yet a production release.

## Properties

- Axum and bundled SQLite in the native adapter; no runtime Node.js requirement
- Dynamic CMS/admin with immutable, content-addressed public releases
- Markdown as the canonical content source, rendered to sanitized HTML
- Passkey-only owner authentication
- Exact scheduled publishing, revisions, redirects, media variants, Atom and JSON feeds, instant search, and backups
- Dates in the site's own time zone and language; a real preview of drafts through the public theme, shareable by short-lived links
- Markdown import and export, backups from the settings page, and a daily backup schedule with rotation
- Atomic release activation and incremental reuse of unchanged generated objects
- Complete `.simple-blog` migration archives for conforming host adapters
- Structured tracing, stable diagnostic codes, deep `doctor` checks, fault injection, and failure-path tests

## Run locally

Rust 1.96 or newer is required. Bun 1.4 and Node.js 24 are development-only dependencies for rebuilding and testing embedded browser assets and the Cloudflare adapter.

```sh
cargo run --locked -- init
cargo run --locked -- serve
```

`init` prints a short-lived setup URL for registering the first owner passkey; `serve` prints the same link on start while no owner is registered, and always prints the site and admin addresses. Data is stored in `./data` by default.

`rust-toolchain.toml` pins current stable for day-to-day work; CI separately proves the declared MSRV in `Cargo.toml`.

Build or materialize the currently visible static release without introducing Git into the writing workflow:

```sh
cargo run --locked -- build
cargo run --locked -- build --output ./public
```

Move the complete durable site between conforming hosts while retaining its custom domain and passkey relying-party identity:

```sh
cargo run --locked -- migrate export --output site.simple-blog
cargo run --locked -- migrate import site.simple-blog
```

The existing installation must be absent for import unless `--force` is supplied. A forced native import preserves the previous directory for recovery.

Move writing, not installations, as Markdown. `export` writes `posts/`, `pages/`, and `media/`; `import` reads that folder back, or any folder of plain `.md` files (titled from their first heading), into the current site:

```sh
cargo run --locked -- export --output ./writing
cargo run --locked -- import ./writing
cargo run --locked -- import ./writing --force   # replace pieces whose address already exists
```

Backups are complete archives (`.tar.zst`) that `restore` reads. The settings page creates and downloads one on demand; `serve` also writes one ten minutes after start and every 24 hours, keeping the newest fourteen. `backup_retention = 0` in `config.toml` (or `SIMPLE_BLOG_BACKUP_RETENTION=0`) switches the schedule off:

```sh
cargo run --locked -- backup
cargo run --locked -- --data-dir ./restored restore ./data/backups/simple-blog-20260903-120000.tar.zst
```

The site's time zone is adopted from the browser when the first passkey is registered and can be changed in the settings; every public date, archive year, and feed timestamp follows it.

## Verify

The common local/CI entry is `node scripts/verify.mjs` after
`bun install --frozen-lockfile`. `browser`, `frontend`, `coverage` and `release`
select smaller scopes. Install browser engines with
`bun x playwright install chromium firefox webkit` (Linux CI also uses
`--with-deps`). Commands, input hashes, revision, tool versions, seed and outcomes
are retained under `target/verification`; `node scripts/collect-evidence.mjs`
exports the vetted subset to `target/shareable`. CI retains this evidence for
30 days even when checks fail. Do not share raw browser reports or test databases.

`node scripts/deep-checks.mjs pr` runs applicable parser fuzzing (60 seconds per
target) and mutation checks; set `VERIFY_BASE` to the actual comparison revision.
An unavailable comparison selects every scope. Daily fuzzing lasts 10 minutes
per target, and weekly mutation testing covers important decisions. Timeouts
and unexecuted checks never count as successful detections.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
PROPTEST_CASES=1024 cargo test --locked --all-targets --all-features
cargo deny check
PROPTEST_CASES=512 cargo llvm-cov --locked --all-features --all-targets \
  --fail-under-lines 79.5 --fail-under-functions 75 \
  --fail-under-regions 76 --fail-under-file-lines 35
bun install --frozen-lockfile
bun run check:admin
bun run test:frontend
bun run check:cloudflare
bun run test:cloudflare
bun audit --audit-level=high
bash tests/repository_policy.sh
```

Architectural decisions live only in [`docs/adr`](docs/adr/README.md). Changes follow Red–Green–Refactor and may not use unscoped lint suppression.

For a reproducible incident report, keep the first failing request ID and run the read-only doctor with machine-readable traces:

```sh
RUST_BACKTRACE=1 \
SIMPLE_BLOG_LOG_FORMAT=json \
RUST_LOG=simple_blog=debug \
cargo run --locked -- doctor --json
```

The JSON diagnostic schema, stable error codes, and secret-redaction rules are compatibility contracts; query strings, cookies, bearer capabilities, and request bodies do not belong in traces.

Doctor opens a verified private copy of the database and WAL, never the source
through SQLite. It does not migrate, checkpoint, recover an interrupted
replacement, or change source sidecars. Active writes or a hot rollback journal
can make diagnosis inconclusive; independent filesystem and release checks
still run. Temporary storage is required for the copy. Default filesystem
checks establish readability; `doctor --probe-writes` explicitly tests creation,
synchronization and cleanup. JSON `inspection_scope`, check `code` and `hint`
are additive to the existing keys and exit status.

Stop the server before restore or portable import: an OS installation lease
prevents concurrent CLI operations. Replacements retain the entire previous
directory beside the installation. A durable activation record lets the next
ordinary command recover an interrupted switch before loading configuration;
doctor only reports it. Keep sibling staging, previous and activation files
together until recovery is verified. Never delete the retained directory as an
automatic cleanup step. Windows directory flushes remain best effort; process
crash tests do not certify hardware power-loss behavior.
Creating the initial sibling lease and replacing directories require access to
the installation's parent. A writable data directory alone does not establish
those capabilities; doctor reports only the inspection scope and paths listed.

Release verification pairs Windows binaries and PDBs by compiler GUID and age;
Linux exports separate debug information with a debug link. Artifact manifests
include SHA-256 hashes of both. `target/verification/symbols-release` contains
the matching set; keep it when diagnosing that build.

`node scripts/verify.mjs performance` builds the disposable fixture and records
three samples against a fixed 100-article dataset. Set `PERFORMANCE_BASELINE` to
`docs/performance-baseline.json` for an environment-aware comparison. Different
platforms or toolchains are marked incomparable. The checked-in debug-profile
baseline documents time, peak memory and SQL counts; it is not a production SLA.
Verification retains private source snapshots and rejects changes during a run;
raw snapshots stay out of shared artifacts.
CI records the job's start before setup and stops verification early enough to
reserve five minutes for evidence. Exhausted work is `not_run` with
`verification.budget_exhausted`; timed-out children remain `timeout`. A forcibly
terminated runner or unavailable artifact service cannot guarantee an upload.

The native adapter is runnable today. The [Cloudflare host adapter](adapters/cloudflare/README.md) has executable conformance, staging, activation, registration, scheduling, and diagnostic boundaries; deployment additionally requires the compatible multi-site internal Core service described there.

## Contributing and security

Changes follow the diagnostic-first [contribution guide](CONTRIBUTING.md) and
the repository's [versioned governance policy](docs/repository-governance.md).
Please report suspected vulnerabilities privately according to the
[security policy](SECURITY.md). Participation is covered by the
[code of conduct](CODE_OF_CONDUCT.md).

## License

Licensed under either [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
