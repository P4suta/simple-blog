# Cloudflare host adapter

This adapter is the official-hosting edge boundary for the host-neutral simple-blog Core. It uses Cloudflare for SaaS custom hostnames with a Worker as fallback origin; it does not use Workers for Platforms and it does not issue a service subdomain for each site.

The adapter is under active development. Its domain lifecycle, static resolver, immutable upload/activation protocol, engagement coordinator, alarm retry behavior, and operator diagnostics are executable contracts. A deploy still needs a compatible `CORE` service implementing the internal admin, publication, and portable-migration API.

## Durable boundaries

| Binding | Role |
| --- | --- |
| `REGISTRY` (D1) | Unique domain reservations, custom-hostname state, hosted-site registry, audit events |
| `RELEASES` (R2) | Site-scoped immutable manifests, generated objects, and media |
| `HOSTS` (KV) | One public domain-to-active-release pointer; never staging state |
| `SITES` (Durable Objects) | Per-site activation CAS, public-content gate, counters, and publication alarm |
| `REGISTRATION_RATE_LIMITER` | Per-location registration admission control before D1 or provider work |
| `CORE` (service binding) | CMS/admin, compiler, archive import/export, and release production |

R2 staging verifies SHA-256 over transferred bytes and performs create-only writes. The Core supplies the BLAKE3 content identity; activation and public reads independently hash the retrieved R2 bytes, require matching BLAKE3 and SHA-256 metadata, and reject a payload changed underneath retained metadata. Activation verifies every manifest reference before the single KV visibility write.

## Domain lifecycle

`POST /v1/registrations` on `CONTROL_HOSTNAME` reserves a custom domain and returns a claim token once. The `REGISTRATION_RATE_LIMITER` binding is keyed by Cloudflare's authenticated source address and checked before parsing the body, reserving D1 state, or calling Cloudflare for SaaS; the example permits ten attempts per source per minute in each Cloudflare location. Choose an account-unique positive integer for its `namespace_id`, monitor 429 events in Workers Observability, and add Turnstile if public launch traffic needs stronger proof-of-human admission. Refresh calls authenticate with the claim token. The token is valid for 24 hours and D1 stores only its hash. Provider ownership, certificate, and CNAME traffic readiness progress independently through:

`pending_ownership` → `pending_certificate` → `pending_dns` → `ready_for_owner` → `active`

Provider failures or regression after ownership produce `action_required`; they do not erase the account. The initial adapter progresses traffic readiness only after observing an exact CNAME to `SAAS_CNAME_TARGET`. It does not promise apex routing; apex support requires an explicit provider capability and conformance contract.

The ready response carries the claim in `https://DOMAIN/admin/setup/#claim=…`. URL fragments never travel in HTTP, so the Core setup page must read it in the browser and explicitly submit it during the owner ceremony. The edge never places the claim in a query string or log. Once Core commits the owner, it calls the authenticated owner-activation route; D1 atomically updates the registration, creates the hosted-site record, and appends its audit event. Retries repair a missing hosted-site projection from the original durable activation timestamp.

`ANONYMOUS_DEMO_HOSTNAME` is one shared trial surface. `workers.dev`, preview URLs, and arbitrary provider-owned per-blog names are disabled by the deployment contract.

## Prepare a deployment

1. Use Wrangler 4.36.0 or newer, copy `wrangler.example.jsonc` to `wrangler.jsonc`, replace every placeholder, and assign an account-unique positive integer to the rate-limit `namespace_id`.
2. Create the D1 database, KV namespace, and R2 bucket named in the file.
3. Apply `migrations/0001_registry.sql` with Wrangler's D1 migration command.
4. Configure Cloudflare for SaaS on the zone, use the Worker fallback origin, and set `SAAS_CNAME_TARGET` to its customer CNAME target.
5. Bind a compatible Core Worker/service.
6. Install secrets; never put them in the Wrangler file:

```sh
bun x wrangler secret put CF_API_TOKEN
bun x wrangler secret put INTERNAL_DO_TOKEN
bun x wrangler secret put DIAGNOSTIC_TOKEN
```

Use independent random values of at least 32 bytes for the two internal capabilities. The Cloudflare API token should be restricted to the one SaaS zone and the custom-hostname permissions it needs.

## Internal Core contract

The Core control routes below exist only on `CONTROL_HOSTNAME`, require the constant-time checked `INTERNAL_DO_TOKEN`, reject unknown shapes, and return `Cache-Control: no-store`:

| Route | Purpose |
| --- | --- |
| `POST /internal/registrations/{registration-id}/owner` | Commit or idempotently repair owner activation |
| `PUT /internal/sites/{site-id}/release-objects/{blake3}` | Create-only object staging with an explicit SHA-256 transport checksum |
| `PUT /internal/sites/{site-id}/release-manifests/{release-id}` | Create-only manifest staging and domain binding |
| `POST /internal/sites/{site-id}/releases/{release-id}/activate` | Durable Object CAS activation after graph verification |

Separately, `/admin/*` on a custom domain is the browser-facing CMS surface. The gateway proxies it to `/internal/sites/{site-id}/http/admin/*` on Core, strips caller-supplied forwarding and site-context headers, injects the internal capability and canonical site identity, and preserves browser cookies and CSRF state. Public paths never pass through Core after release activation.

## Verification and rescue

```sh
bun run check:cloudflare
bun run test:cloudflare
curl --fail-with-body \
  -H "Authorization: Bearer $SIMPLE_BLOG_DIAGNOSTIC_TOKEN" \
  https://control.example.com/internal/doctor
```

Doctor is authenticated, read-only, bounded by per-check deadlines, and always attempts all independent probes. It emits only stable codes such as `CF_D1_UNREACHABLE`; upstream response bodies, claims, cookies, query strings, and secret material never appear in its report. Worker request logs include a generated request ID, method, path, status, and latency. D1 reservation, observation, and owner transitions write their audit records in the same transaction; audit inserts are coupled to the immediately preceding mutation so a no-op or stale CAS cannot fabricate timeline entries.

There is no product-level post, byte, or traffic quota in this adapter. Cloudflare resource limits, abuse protection, and explicit request/upload safety limits still apply.
