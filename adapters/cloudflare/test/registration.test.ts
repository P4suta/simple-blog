import assert from "node:assert/strict";
import test from "node:test";

import {
  RegistrationService,
  type ClaimSecret,
  type CustomHostname,
  type CustomHostnameProvider,
  type DomainRouteVerifier,
  type RegistrationRecord,
  type RegistrationRepository,
} from "../src/registration.ts";

class Repository implements RegistrationRepository {
  readonly records = new Map<string, RegistrationRecord>();
  readonly domains = new Set<string>();
  saves = 0;

  async reserve(record: RegistrationRecord): Promise<boolean> {
    if (this.domains.has(record.domain)) return false;
    this.domains.add(record.domain);
    this.records.set(record.id, structuredClone(record));
    return true;
  }

  async authorized(id: string, claimHash: string, now: string): Promise<RegistrationRecord | null> {
    const record = this.records.get(id);
    return record !== undefined && record.claimHash === claimHash && record.claimExpiresAt > now
      ? structuredClone(record)
      : null;
  }

  async save(record: RegistrationRecord): Promise<void> {
    const current = this.records.get(record.id);
    if (current === undefined || current.storageVersion !== record.storageVersion) {
      throw new Error("registration_stale_observation");
    }
    record.storageVersion += 1;
    this.saves += 1;
    this.records.set(record.id, structuredClone(record));
  }
}

class Provider implements CustomHostnameProvider {
  creates = 0;
  failCreate = false;
  hostname: CustomHostname = {
    id: "cf-7",
    hostnameStatus: "pending",
    certificateStatus: "pending",
    providerErrorCode: null,
    ownershipVerification: { type: "txt", name: "_cf-custom-hostname", value: "ownership" },
    certificateValidation: [{ type: "txt", name: "_acme", value: "certificate" }],
  };

  async create(): Promise<CustomHostname> {
    this.creates += 1;
    if (this.failCreate) throw new Error("provider unavailable");
    return structuredClone(this.hostname);
  }

  async inspect(): Promise<CustomHostname> {
    return structuredClone(this.hostname);
  }
}

class Dns implements DomainRouteVerifier {
  routed = false;
  async routesToService(): Promise<boolean> { return this.routed; }
}

function service() {
  const repository = new Repository();
  const provider = new Provider();
  const dns = new Dns();
  let sequence = 0;
  const claims = {
    async issue(): Promise<ClaimSecret> {
      sequence += 1;
      return { token: `claim-${sequence}`, hash: `hash-${sequence}` };
    },
    async hash(token: string): Promise<string> {
      return token.replace("claim", "hash");
    },
  };
  const registration = new RegistrationService(
    repository,
    provider,
    dns,
    claims,
    () => "2026-09-02T12:00:00.000Z",
    () => `00000000-0000-4000-8000-00000000000${sequence + 1}`,
    "customers.service.dev",
  );
  return { registration, repository, provider, dns };
}

test("a domain reservation is unique and stores only the hash of its one-time claim", async () => {
  const state = service();
  const first = await state.registration.start("BLOG.Writer.Com");
  assert.equal(first.domain, "blog.writer.com");
  assert.equal(first.claimToken, "claim-1");
  assert.equal(first.cnameTarget, "customers.service.dev");
  assert.equal(first.state, "pending_ownership");
  const stored = state.repository.records.get(first.id)!;
  assert.equal(stored.claimHash, "hash-1");
  assert.equal(JSON.stringify(stored).includes("claim-1"), false);

  await assert.rejects(state.registration.start("blog.writer.com"), /domain_unavailable/);
  await assert.rejects(state.registration.start("random.service.dev"), /domain_unavailable/);
  assert.equal(state.provider.creates, 1);
});

test("refresh requires the claim and advances through provider plus DNS readiness", async () => {
  const state = service();
  const started = await state.registration.start("blog.writer.com");
  await assert.rejects(
    state.registration.refresh(started.id, "wrong-claim"),
    /registration_not_found/,
  );
  state.provider.hostname.hostnameStatus = "active";
  state.provider.hostname.certificateStatus = "pending";
  assert.equal(
    (await state.registration.refresh(started.id, started.claimToken)).state,
    "pending_certificate",
  );
  state.provider.hostname.certificateStatus = "active";
  assert.equal(
    (await state.registration.refresh(started.id, started.claimToken)).state,
    "pending_dns",
  );
  state.dns.routed = true;
  assert.equal(
    (await state.registration.refresh(started.id, started.claimToken)).state,
    "ready_for_owner",
  );
  const ready = await state.registration.refresh(started.id, started.claimToken);
  assert.equal(
    ready.ownerSetupUrl,
    `https://blog.writer.com/admin/setup/#claim=${started.claimToken}`,
  );
});

test("provider failures become diagnosable action-required records instead of partial accounts", async () => {
  const state = service();
  state.provider.hostname.hostnameStatus = "failed";
  state.provider.hostname.providerErrorCode = "hostname_validation_failed";

  const started = await state.registration.start("blog.writer.com");

  assert.equal(started.state, "action_required");
  const stored = state.repository.records.get(started.id)!;
  assert.equal(stored.providerErrorCode, "hostname_validation_failed");
  assert.equal(stored.ownerRegisteredAt, null);
});

test("provider creation failure persists one retryable observation exactly once", async () => {
  const state = service();
  state.provider.failCreate = true;

  const started = await state.registration.start("blog.writer.com");

  assert.equal(started.state, "action_required");
  assert.equal(state.repository.saves, 1);
  const stored = state.repository.records.get(started.id)!;
  assert.equal(stored.providerErrorCode, "provider_create_failed");
  assert.equal(stored.storageVersion, 1);
});
