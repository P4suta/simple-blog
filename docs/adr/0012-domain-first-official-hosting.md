# ADR 0012: Domain-first official hosting

- Status: Accepted
- Date: 2026-09-02

## Context

Provider-issued blog subdomains create routing and certificate overhead, bind passkeys and URLs to the provider, and make account identity independent from the thing being hosted. They also encourage arbitrary plan limits even though the public output is a static release.

## Decision

Official hosting begins by reserving a normalized custom domain. A 256-bit claim capability is returned once, only its SHA-256 hash is stored, and it expires after 24 hours until an owner exists. Ownership validation, certificate readiness, and traffic DNS readiness are distinct observed states. Owner passkey setup becomes available only after all three are ready; the domain is the WebAuthn relying-party identity.

No per-site provider subdomain is issued. Only one explicitly configured, shared anonymous demo hostname exists, and deployment disables `workers.dev` and preview URLs. The initial Cloudflare adapter accepts CNAME routing; apex support requires an explicit platform capability rather than a misleading partial flow.

The official adapter uses Cloudflare for SaaS custom hostnames and a Worker fallback origin. D1 stores the registry and audit timeline, R2 stores immutable releases and media, one KV record is the visible host/release pointer, and one Durable Object per site serializes counters, publication alarms, and release activation. The Core is reached through a service binding.

There is no arbitrary product content or traffic quota. Backend safety limits, provider limits, abuse controls, and explicit upload-envelope limits remain enforceable and diagnosable.

## Consequences

- Bringing a domain is required for a durable account and is what makes same-domain migration seamless.
- Losing external DNS or certificate readiness moves an owned site to `action_required` without deleting its account or data.
- Public activation cannot expose an R2 graph lacking verified transport metadata.
- Operators get an authenticated, read-only doctor spanning configuration, D1, KV, R2, Durable Objects, and Core.
- The anonymous demo is not a durable identity or a source of unbounded subdomains.
