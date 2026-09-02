import assert from "node:assert/strict";
import test from "node:test";

import type {
  D1Database,
  D1PreparedStatement,
  D1Result,
} from "../src/bindings.ts";
import {
  D1OwnerActivationRepository,
  handleOwnerActivationRequest,
  type OwnerActivationRepository,
} from "../src/owner-activation.ts";

const registrationId = "00000000-0000-4000-8000-000000000001";
const token = `test-only-owner-${"x".repeat(32)}`;

class Repository implements OwnerActivationRepository {
  calls: unknown[][] = [];
  conflict = false;
  async activateOwner(id: string, domain: string, registeredAt: string) {
    this.calls.push([id, domain, registeredAt]);
    if (this.conflict) throw new Error("owner_activation_conflict");
    return { changed: true };
  }
}

function request(presented = token, body: unknown = {
  domain: "writing.example.com",
  registered_at: "2026-09-02T12:00:00.000Z",
}) {
  return new Request(
    `https://control.service.dev/internal/registrations/${registrationId}/owner`,
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${presented}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(body),
    },
  );
}

test("Core can idempotently commit an owner only through the internal capability", async () => {
  const repository = new Repository();
  const response = await handleOwnerActivationRequest(request(), repository, token);

  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { changed: true });
  assert.deepEqual(repository.calls, [[
    registrationId,
    "writing.example.com",
    "2026-09-02T12:00:00.000Z",
  ]]);
  assert.equal(response.headers.get("cache-control"), "no-store");

  assert.equal((await handleOwnerActivationRequest(
    request("attacker-token"),
    repository,
    token,
  )).status, 401);
  assert.equal(repository.calls.length, 1);
});

test("owner activation validates exact input and exposes a stable CAS conflict", async () => {
  const repository = new Repository();
  assert.equal((await handleOwnerActivationRequest(request(token, {
    domain: "writing.example.com",
    registered_at: "2026-09-02T12:00:00.000Z",
    role: "admin",
  }), repository, token)).status, 422);
  assert.equal(repository.calls.length, 0);

  repository.conflict = true;
  const conflict = await handleOwnerActivationRequest(request(), repository, token);
  assert.equal(conflict.status, 409);
  assert.deepEqual(await conflict.json(), { error: "owner_activation_conflict" });
});

class Statement implements D1PreparedStatement {
  readonly query: string;
  readonly database: Database;
  values: unknown[] = [];
  constructor(query: string, database: Database) {
    this.query = query;
    this.database = database;
  }
  bind(...values: unknown[]) { this.values = values; return this; }
  async first<T>() { return this.database.row as T | null; }
  async run(): Promise<D1Result> { return { success: true, meta: { changes: 1 } }; }
  async all<T>() { return { results: [] as T[], success: true }; }
}

class Database implements D1Database {
  readonly prepared: Statement[] = [];
  batches: D1PreparedStatement[][] = [];
  row: Record<string, unknown> | null = {
    id: registrationId,
    domain: "writing.example.com",
    state: "ready_for_owner",
    owner_registered_at: null,
    row_version: 3,
  };
  prepare(query: string) {
    const statement = new Statement(query, this);
    this.prepared.push(statement);
    return statement;
  }
  async batch(statements: D1PreparedStatement[]) {
    this.batches.push(statements);
    return statements.map(() => ({ success: true, meta: { changes: 1 } }));
  }
}

test("D1 owner activation groups registry, hosted site, and audit writes transactionally", async () => {
  const database = new Database();
  const result = await new D1OwnerActivationRepository(database).activateOwner(
    registrationId,
    "writing.example.com",
    "2026-09-02T12:00:00.000Z",
  );

  assert.deepEqual(result, { changed: true });
  assert.equal(database.batches.length, 1);
  assert.equal(database.batches[0]?.length, 3);
  assert.match((database.batches[0]?.[0] as Statement).query, /row_version = row_version \+ 1/);
  assert.match((database.batches[0]?.[1] as Statement).query, /INSERT INTO hosting_audit_events/);
  assert.match((database.batches[0]?.[1] as Statement).query, /changes\(\) = 1/);
  assert.match((database.batches[0]?.[2] as Statement).query, /INSERT INTO hosted_sites/);
});

test("idempotent owner repair preserves the original activation timestamp", async () => {
  const database = new Database();
  database.row = {
    id: registrationId,
    domain: "writing.example.com",
    state: "active",
    owner_registered_at: "2026-09-02T12:00:00.000Z",
    row_version: 4,
  };

  const result = await new D1OwnerActivationRepository(database).activateOwner(
    registrationId,
    "writing.example.com",
    "2026-09-03T09:30:00.000Z",
  );

  assert.deepEqual(result, { changed: false });
  const repair = database.prepared.at(-1)!;
  assert.match(repair.query, /INSERT INTO hosted_sites/);
  assert.deepEqual(
    repair.values.slice(0, 2),
    ["2026-09-02T12:00:00.000Z", "2026-09-02T12:00:00.000Z"],
  );
});
