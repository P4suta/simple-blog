# Diagnostics

Every failure can be explained. A request carries an `x-request-id` header
and every trace of it carries the same `request_id`; a failure adds a stable
`error_code`; `doctor` checks everything it can reach and names each check.
This page explains each identifier. The machine-readable list is
[`contracts/diagnostics-v1.json`](../contracts/diagnostics-v1.json); the two
are kept in step by the repository policy check, and the native binary is
tested to emit exactly the codes listed.

Codes are a compatibility contract: a code never changes meaning. A new one is
added to the contract, explained here, and emitted only through a constant in
`src/observability.rs`, never a literal.

## Reading a trace

Run with `SIMPLE_BLOG_LOG_FORMAT=json` and every line is one event. Keep the
first failing `request_id`; every event of that request, including the
completion with its status, carries it. Nothing sensitive is traced: no query
strings, cookies, bearer capabilities, or request bodies.

## Error codes (native adapter)

| Code | Emitted by | Meaning and where to look first |
| --- | --- | --- |
| `repository.conflict` | web | A save carried a version that is no longer current. The writer sees both versions and loses nothing. |
| `repository.slug_taken` | web | The requested address belongs to another piece, live or historical. |
| `repository.not_found` | web | The piece, revision, or media the request named does not exist. |
| `repository.validation` | web | A field broke a rule the editor also enforces; the message names the field. |
| `repository.storage` | web | SQLite refused or failed a query. Check the database file and the disk, then `doctor`. |
| `template.render` | web | An admin page could not be rendered from its embedded template. |
| `auth.storage` | web | Sessions, tokens, or recovery codes could not be read or written. |
| `auth.passkey` | web | A passkey ceremony failed or did not match the site's origin (`public_url`). |
| `media.processing` | web | An upload could not be decoded or re-encoded; the limit or the file is named. |
| `media.storage` | web | Media records or files could not be read or written. |
| `publication.build` | web | Publishing on request failed. The last complete release stays visible and the dashboard offers to publish now. |
| `site.compile` | web | The public site could not be compiled from the current snapshot. |
| `release.integrity` | web | A stored release object or manifest failed its checksum. Run `doctor`. |
| `release.not_found` | web | The active release names an object that is not in the store. |
| `release.read` | web | The release store could not be read. |
| `web.internal` | web | An unexpected failure; the request ID leads to the trace with the cause. |
| `security.rate_limited` | web | A client exceeded the authentication or like rate limit; `Retry-After` says when to try again. `doctor` reports both limits under `limits.rate`. |
| `publication_repository_failed` | publication | The public snapshot could not be read from the database. |
| `publication_compile_failed` | publication | The compiler rejected the snapshot; the site keeps its last release. |
| `publication_release_store_failed` | publication | The compiled release could not be stored or activated. |
| `publication_scheduler_state_failed` | scheduler | The scheduler could not read the publication clock; it retries with backoff. |
| `release_object_store_failed` | release | Writing a content-addressed object to the release store failed. |
| `release_manifest_store_failed` | release | Writing the release manifest failed; nothing was activated. |
| `release_activation_failed` | release | The active pointer could not be switched; the previous release stays active. |
| `backup_scheduled_failed` | scheduler | A scheduled backup could not be written; the next run tries again. |

## Error codes (Cloudflare host adapter)

The worker's `/internal/doctor` reports one check per component, each with a
`diagnostic_code` when it fails.

| Code | Component | Meaning |
| --- | --- | --- |
| `CF_CONFIGURATION_INVALID` | `configuration` | A required binding, secret, or zone setting of the worker is missing or malformed. |
| `CF_D1_UNREACHABLE` | `d1` | The site registry database did not answer a probe. |
| `CF_DURABLE_OBJECT_UNREACHABLE` | `durable_object` | A site's durable object did not answer a probe. |
| `CF_KV_UNREACHABLE` | `kv` | The host routing store did not answer a probe. |
| `CF_R2_UNREACHABLE` | `r2` | The release object store did not answer a probe. |
| `CF_CORE_UNHEALTHY` | `core` | The multi-site Core service did not answer its health check. |

The worker's internal endpoints answer a refused request with one of these
identifiers in an `error` field: `diagnostic_capability_required`,
`diagnostic_method_not_allowed`, `diagnostic_route_not_found`,
`owner_activation_capability_required`, `owner_activation_invalid`,
`owner_activation_method_not_allowed`, `owner_activation_route_not_found`,
`publication_capability_required`, `publication_method_not_allowed`,
`publication_route_invalid`, and `publication_route_not_found`.

## Doctor checks (native adapter)

`simple-blog doctor` runs every check below and reports each as `ok` or
`error` with a detail line; `doctor --json` adds a `limits` object with the
values in force.

| Check | What it verifies |
| --- | --- |
| `sqlite.quick_check` | SQLite's own integrity check answers `ok`. |
| `sqlite.foreign_keys` | No row violates a foreign key. |
| `sqlite.runtime_pragmas` | Foreign keys are on, the journal is WAL, and the busy timeout is at least five seconds. |
| `sqlite.migrations` | Every embedded migration is applied with the same checksum, and none is unknown. |
| `filesystem.data` | The data directory exists and is writable. |
| `filesystem.media` | The media directory exists and is writable. |
| `filesystem.backups` | The backup directory exists and is writable. |
| `filesystem.releases` | The release directory exists and is writable. |
| `media.records` | Every media record has its files, with the recorded size, type, dimensions, and (for originals) checksum. |
| `media.orphans` | No file in the media directory is unreferenced or an interrupted upload. |
| `content.trash` | How many pieces wait in the trash. |
| `release.active` | The active release exists and every object it names verifies. |
| `release.history` | Every kept manifest and object verifies, and no object is unreferenced. |
| `release.temporary_files` | No interrupted release write was left behind. |
| `limits.upload` | The upload size limit (`max_upload_bytes`). |
| `limits.text` | The Markdown, title, and summary limits per piece. |
| `limits.image` | The pixel and side limits per image. |
| `limits.theme` | The stylesheet size and navigation item limits. |
| `limits.search` | The query length and term count limits. |
| `limits.rate` | The authentication and like rate limits per client and minute. |
| `limits.history` | How many autosave revisions per piece and how many settings states are kept. |
| `limits.backups` | How many scheduled backups are kept (`backup_retention`), or that the schedule is off. |

None of the limits is a quota. There is no cap on pieces, on bytes in total,
or on readers; each limit protects one request or one piece, and each can be
seen here before anyone runs into it.

## Doctor checks (Cloudflare host adapter)

`/internal/doctor` probes `configuration`, `d1`, `durable_object`, `kv`,
`r2`, and `core`, each within a two-second deadline, and reports `ok` or
`degraded` overall.
