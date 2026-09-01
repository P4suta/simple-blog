import assert from "node:assert/strict";
import test from "node:test";

import type { D1Database, D1PreparedStatement, D1Result } from "../src/bindings.ts";
import {
  CnameDomainRouteVerifier,
  CloudflareCustomHostnameProvider,
  D1RegistrationRepository,
  WebCryptoClaimSecrets,
} from "../src/cloudflare-registration.ts";
import type { RegistrationRecord } from "../src/registration.ts";

class Statement implements D1PreparedStatement {
  readonly query: string;
  readonly row: Record<string, unknown> | null;
  readonly result: D1Result;
  values: unknown[] = [];
  constructor(query: string, row: Record<string, unknown> | null, result: D1Result) {
    this.query = query;
    this.row = row;
    this.result = result;
  }
  bind(...values: unknown[]) { this.values = values; return this; }
  async first<T>() { return this.row as T | null; }
  async run() { return this.result; }
  async all<T>() { return { results: [] as T[], success: true }; }
}

class Database implements D1Database {
  readonly statements: Statement[] = [];
  readonly row: Record<string, unknown> | null;
  readonly result: D1Result;
  constructor(row: Record<string, unknown> | null, result: D1Result) {
    this.row = row;
    this.result = result;
  }
  prepare(query: string) {
    const statement = new Statement(query, this.row, this.result);
    this.statements.push(statement);
    return statement;
  }
  async batch(statements: D1PreparedStatement[]) {
    return Promise.all(statements.map((statement) => statement.run()));
  }
}

function registration(): RegistrationRecord {
  return {
    id: "00000000-0000-4000-8000-000000000001",
    domain: "blog.writer.com",
    claimHash: "a".repeat(64),
    claimExpiresAt: "2026-09-03T00:00:00.000Z",
    providerHostnameId: "hostname_7",
    state: "pending_certificate",
    ownershipVerification: null,
    certificateValidation: [],
    providerErrorCode: null,
    lastObservedAt: "2026-09-02T00:00:00.000Z",
    ownerRegisteredAt: null,
    createdAt: "2026-09-02T00:00:00.000Z",
    updatedAt: "2026-09-02T00:00:00.000Z",
    storageVersion: 2,
  };
}

test("Cloudflare for SaaS responses become provider-neutral observations", async () => {
  const requests: Request[] = [];
  const request = async (input: string | URL | Request, init?: RequestInit) => {
    requests.push(new Request(input, init));
    return Response.json({
      success: true,
      result: {
        id: "hostname_7",
        status: "active",
        ownership_verification: {
          type: "txt",
          name: "_cf-custom-hostname.blog.writer.com",
          value: "ownership-token",
        },
        ssl: {
          status: "pending_validation",
          validation_records: [{ txt_name: "_acme.blog.writer.com", txt_value: "cert-token" }],
        },
      },
    }, { status: 201 });
  };
  const provider = new CloudflareCustomHostnameProvider("zone-1", "api-secret", request);

  const hostname = await provider.create("blog.writer.com");

  assert.equal(hostname.id, "hostname_7");
  assert.equal(hostname.hostnameStatus, "active");
  assert.equal(hostname.certificateStatus, "pending");
  assert.deepEqual(hostname.certificateValidation, [
    { type: "txt", name: "_acme.blog.writer.com", value: "cert-token" },
  ]);
  assert.equal(requests[0]!.headers.get("authorization"), "Bearer api-secret");
  assert.deepEqual(await requests[0]!.json(), {
    hostname: "blog.writer.com",
    ssl: { method: "txt", type: "dv" },
  });
});

test("CNAME readiness is exact and claim tokens are 256-bit values stored as SHA-256", async () => {
  const dns = new CnameDomainRouteVerifier(async () => Response.json({
    Status: 0,
    Answer: [{ type: 5, data: "customers.service.dev." }],
  }));
  assert.equal(
    await dns.routesToService("blog.writer.com", "customers.service.dev"),
    true,
  );
  assert.equal(
    await dns.routesToService("blog.writer.com", "attacker.service.dev"),
    false,
  );

  const claims = new WebCryptoClaimSecrets();
  const issued = await claims.issue();
  assert.equal(issued.token.length, 43);
  assert.match(issued.hash, /^[0-9a-f]{64}$/);
  assert.equal(await claims.hash(issued.token), issued.hash);
  assert.notEqual((await claims.issue()).token, issued.token);
});

test("provider HTTP errors expose only stable diagnostic status", async () => {
  const provider = new CloudflareCustomHostnameProvider(
    "zone-1",
    "api-secret",
    async () => new Response("sensitive upstream detail", { status: 503 }),
  );
  await assert.rejects(provider.create("blog.writer.com"), {
    message: "cloudflare_custom_hostname_http_503",
  });
});

test("D1 rejects a stale observation instead of silently reporting success", async () => {
  const database = new Database(null, { success: true, meta: { changes: 0 } });
  const repository = new D1RegistrationRepository(database);

  await assert.rejects(repository.save(registration()), {
    message: "registration_stale_observation",
  });
  assert.match(database.statements[1]!.query, /changes\(\) = 1/);
});

test("D1 reservation and its audit event are one change-coupled batch", async () => {
  const database = new Database(null, { success: true, meta: { changes: 1 } });
  const value = registration();

  assert.equal(await new D1RegistrationRepository(database).reserve(value), true);

  assert.equal(database.statements.length, 2);
  assert.match(database.statements[0]!.query, /INSERT INTO domain_registrations/);
  assert.match(database.statements[1]!.query, /INSERT INTO hosting_audit_events/);
  assert.match(database.statements[1]!.query, /changes\(\) = 1/);
  assert.equal(database.statements[0]!.values.includes(value.claimHash), true);
  assert.equal(database.statements[0]!.values.some((item) => item === "claim-token"), false);
});

test("D1 rows are decoded as an untrusted adapter boundary", async () => {
  const value = registration();
  const database = new Database({
    id: value.id,
    domain: value.domain,
    claim_hash: value.claimHash,
    claim_expires_at: value.claimExpiresAt,
    provider_hostname_id: value.providerHostnameId,
    state: "future_provider_state",
    ownership_verification_json: null,
    certificate_validation_json: "[]",
    provider_error_code: null,
    last_observed_at: value.lastObservedAt,
    owner_registered_at: null,
    created_at: value.createdAt,
    updated_at: value.updatedAt,
    row_version: value.storageVersion,
  }, { success: true, meta: { changes: 1 } });

  await assert.rejects(
    new D1RegistrationRepository(database).authorized(value.id, value.claimHash, value.createdAt),
    { message: "registration_state_invalid" },
  );
});
