import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync, type StatementSync } from "node:sqlite";
import test from "node:test";

import type {
  D1Database,
  D1PreparedStatement,
  D1Result,
} from "../src/bindings.ts";
import { D1RegistrationRepository } from "../src/cloudflare-registration.ts";
import { D1OwnerActivationRepository } from "../src/owner-activation.ts";
import type { RegistrationRecord } from "../src/registration.ts";

class SqliteStatement implements D1PreparedStatement {
  private readonly statement: StatementSync;
  private values: unknown[] = [];

  constructor(statement: StatementSync) {
    this.statement = statement;
  }

  bind(...values: unknown[]) {
    this.values = values;
    return this;
  }

  async first<T>() {
    return (this.statement.get(...this.values) ?? null) as T | null;
  }

  async run(): Promise<D1Result> {
    const result = this.statement.run(...this.values);
    return { success: true, meta: { changes: Number(result.changes) } };
  }

  async all<T>() {
    return { results: this.statement.all(...this.values) as T[], success: true };
  }
}

class SqliteD1 implements D1Database {
  private readonly database = new DatabaseSync(":memory:");

  constructor() {
    this.database.exec(readFileSync(
      new URL("../migrations/0001_registry.sql", import.meta.url),
      "utf8",
    ));
  }

  prepare(query: string): D1PreparedStatement {
    return new SqliteStatement(this.database.prepare(query));
  }

  async batch(statements: D1PreparedStatement[]): Promise<D1Result[]> {
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const results: D1Result[] = [];
      for (const statement of statements) results.push(await statement.run());
      this.database.exec("COMMIT");
      return results;
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
  }

  run(query: string, ...values: unknown[]): void {
    this.database.prepare(query).run(...values);
  }

  scalar(query: string, ...values: unknown[]): unknown {
    return Object.values(this.database.prepare(query).get(...values) ?? {})[0];
  }
}

function registration(): RegistrationRecord {
  return {
    id: "00000000-0000-4000-8000-000000000001",
    domain: "writing.example.com",
    claimHash: "a".repeat(64),
    claimExpiresAt: "2026-09-03T00:00:00.000Z",
    providerHostnameId: null,
    state: "pending_ownership",
    ownershipVerification: null,
    certificateValidation: [],
    providerErrorCode: null,
    lastObservedAt: null,
    ownerRegisteredAt: null,
    createdAt: "2026-09-02T00:00:00.000Z",
    updatedAt: "2026-09-02T00:00:00.000Z",
    storageVersion: 0,
  };
}

test("production registration SQL never audits a no-op reservation or stale CAS", async () => {
  const database = new SqliteD1();
  const repository = new D1RegistrationRepository(database);
  const value = registration();

  assert.equal(await repository.reserve(value), true);
  assert.equal(await repository.reserve(value), false);
  assert.equal(database.scalar("SELECT count(*) FROM hosting_audit_events"), 1);

  value.providerHostnameId = "hostname_7";
  value.state = "pending_certificate";
  value.lastObservedAt = "2026-09-02T00:01:00.000Z";
  value.updatedAt = value.lastObservedAt;
  await repository.save(value);
  assert.equal(value.storageVersion, 1);

  const stale = { ...value, state: "pending_dns" as const, storageVersion: 0 };
  await assert.rejects(repository.save(stale), {
    message: "registration_stale_observation",
  });
  assert.equal(database.scalar("SELECT count(*) FROM hosting_audit_events"), 2);
  assert.equal(
    database.scalar(
      "SELECT diagnostic_code FROM hosting_audit_events ORDER BY id DESC LIMIT 1",
    ),
    "HOST_DOMAIN_PENDING_CERTIFICATE",
  );
});

test("production owner SQL is atomic and repairs from the durable activation time", async () => {
  const database = new SqliteD1();
  const value = registration();
  assert.equal(await new D1RegistrationRepository(database).reserve(value), true);
  database.run(
    "UPDATE domain_registrations SET state = 'ready_for_owner' WHERE id = ?",
    value.id,
  );
  const owners = new D1OwnerActivationRepository(database);
  const activatedAt = "2026-09-02T12:00:00.000Z";

  assert.deepEqual(
    await owners.activateOwner(value.id, value.domain, activatedAt),
    { changed: true },
  );
  assert.equal(
    database.scalar(
      "SELECT count(*) FROM hosting_audit_events WHERE event = 'owner.activated'",
    ),
    1,
  );

  database.run("DELETE FROM hosted_sites WHERE registration_id = ?", value.id);
  assert.deepEqual(
    await owners.activateOwner(
      value.id,
      value.domain,
      "2026-09-03T09:30:00.000Z",
    ),
    { changed: false },
  );
  assert.equal(
    database.scalar(
      "SELECT created_at FROM hosted_sites WHERE registration_id = ?",
      value.id,
    ),
    activatedAt,
  );
});
