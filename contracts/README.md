# Contracts

The fixtures in this directory are what "conforming host adapter" means. The
Core (Rust) and every adapter consume the same files; an adapter that passes
them behaves like the native one where a reader or a writer could tell the
difference. They are compatibility surfaces: a fixture changes only by adding
cases or fields, and a change in meaning is a new `-v2` file.

| Fixture | What it fixes | Consumed by |
| --- | --- | --- |
| [`release-resolution-v1.json`](release-resolution-v1.json) | How a request path resolves against an immutable release: canonical routes, redirects, the 404 fallback, and the exact `Content-Type` and `Cache-Control` an asset is served with. | `tests/cross_adapter_spec.rs`; `adapters/cloudflare/test/release.test.ts`, `adapters/cloudflare/test/public.test.ts` |
| [`domain-registration-v1.json`](domain-registration-v1.json) | The lifecycle of a custom domain on the official host: which observations move a registration to which state. | `tests/hosting_domain_spec.rs`; `adapters/cloudflare/test/domain.test.ts` |
| [`diagnostics-v1.json`](diagnostics-v1.json) | Every stable error code, doctor check, and internal API error identifier an adapter may emit, explained in [`docs/diagnostics.md`](../docs/diagnostics.md). | `tests/observability_spec.rs`, `tests/operations_spec.rs`; `adapters/cloudflare/test/doctor.test.ts`; `tests/repository_policy.sh` |
| [`portable-site-v1.json`](portable-site-v1.json) | The logical model a `.simple-blog` archive carries (`site.json`), field order and omissions included, so a second implementation can prove it reads and writes the same site. | `tests/portable_archive_spec.rs` |

## Proving an adapter conforms

1. Resolve every case in `release-resolution-v1.json` against the manifest
   and objects it carries. `kind`, `status`, `object_id`, `fallback`,
   `location`, `content_type`, and `cache_control` must match exactly.
2. Drive `domain-registration-v1.json` through the registration lifecycle
   and reach `expected` for every case.
3. Emit only the error codes and doctor checks that `diagnostics-v1.json`
   lists for you, and emit all of them.
4. Read `portable-site-v1.json` as a site, write it back, and produce the same
   bytes; write it into an archive and read it back unchanged. The archive
   framing (tar in zstd with a checksummed manifest) is described in ADR
   [0011](../docs/adr/0011-portable-core-and-host-adapters.md) and implemented
   in `src/portable.rs`. Today only the native adapter reads archives; the
   Cloudflare adapter's migration API is specified in its README and is not
   yet covered by an executable fixture.

For the Core:

```sh
cargo test --locked --all-features --test cross_adapter_spec --test hosting_domain_spec \
  --test observability_spec --test operations_spec --test portable_archive_spec
```

For the Cloudflare adapter:

```sh
bun run test:cloudflare
```

## Adding to a contract

Add the case or field to the fixture, make every consumer assert it, and run
both suites. A new error code is a constant in `src/observability.rs`, an
entry here, and a row in `docs/diagnostics.md`, in the same change.
