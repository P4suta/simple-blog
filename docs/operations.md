# Operations

Commands use `./data` unless `--data-dir` selects another installation.
The server adopts the browser's time zone when the first owner registers a passkey.
The settings page controls the site's time zone and language afterward.

## Publish and move content

Build the currently visible release without adding Git to the writing workflow.

```sh
cargo run --locked -- build
cargo run --locked -- build --output ./public
```

Markdown export writes `posts/`, `pages/`, and `media/`.
Import accepts that layout or plain Markdown files, using each file's first heading as its title.

```sh
cargo run --locked -- export --output ./writing
cargo run --locked -- import ./writing
```

Use `import --force` to replace content at an existing address.
Draft preview links expire after a short interval; keep those capabilities private.

## Move an installation

The `.simple-blog` archive moves the complete durable site between conforming hosts while keeping its custom domain and passkey identity.

```sh
cargo run --locked -- migrate export --output site.simple-blog
cargo run --locked -- migrate import site.simple-blog
```

Import needs an absent installation unless `--force` permits replacement.
A forced native import keeps the previous directory for recovery.

## Back up and restore

The settings page creates and downloads a complete `.tar.zst` backup on demand.
The server also creates a backup ten minutes after startup and every 24 hours, keeping the newest fourteen.
Set `backup_retention = 0` in `config.toml` or `SIMPLE_BLOG_BACKUP_RETENTION=0` to stop the schedule.

```sh
cargo run --locked -- backup
cargo run --locked -- --data-dir ./restored restore ./data/backups/simple-blog-20260903-120000.tar.zst
```

Stop the server before restore or portable import.
An installation lease prevents concurrent CLI operations.
Replacements keep the complete previous directory beside the installation.
Keep sibling staging, previous, and activation files together until recovery succeeds.
Keep the previous directory after automatic recovery; cleanup needs an explicit decision.

A durable activation record lets the next ordinary command recover an interrupted switch before it loads configuration.
Doctor reports that state without performing the recovery.
Windows directory flushes remain best effort, and process crash tests supply no hardware power-loss guarantee.
Creating the sibling lease and replacing directories need access to the installation's parent.

## Diagnose an installation

Keep the first failing request ID for a reproducible report.
Set `RUST_BACKTRACE=1`, `SIMPLE_BLOG_LOG_FORMAT=json`, and `RUST_LOG=simple_blog=debug` in your shell for detailed traces.

```sh
cargo run --locked -- doctor --json
```

Doctor inspects a verified private copy of the database and its write-ahead log.
It leaves the source database, configuration, and sidecars unchanged.
Active writes or a hot rollback journal can make database inspection inconclusive while independent filesystem and release checks continue.
The private copy needs temporary storage.

Default filesystem checks establish readability.
Use `doctor --probe-writes` to test creation, synchronization, and cleanup explicitly.
A writable data directory alone supplies no evidence about replacing entries in its parent.
The JSON `inspection_scope`, check `code`, and `hint` describe the verified scope.

Diagnostic JSON, stable error codes, and secret redaction form compatibility contracts.
Keep query strings, cookies, bearer capabilities, request bodies, raw databases, and private source snapshots out of shared reports.
The [contribution guide](../CONTRIBUTING.md#verification) explains the vetted evidence export.

Commands exit `0` after success, `1` after an executed failure, and `2` when parsing or diagnostic setup prevents execution.
Use `cargo run --locked -- --help` and each subcommand's `--help` for flags.
