import assert from "node:assert/strict";
import test from "node:test";

import type {
  D1Database,
  D1PreparedStatement,
  D1Result,
  DurableObjectId,
  DurableObjectNamespace,
  DurableObjectStub,
  Fetcher,
  KvNamespace,
  R2Bucket,
  WorkerEnv,
} from "../src/bindings.ts";
import { handleDoctorRequest } from "../src/doctor.ts";

const internalCapability = `test-only-internal-${"x".repeat(32)}`;
const diagnosticCapability = `test-only-diagnostic-${"x".repeat(32)}`;
const providerCapability = `test-only-provider-${"x".repeat(32)}`;

class Statement implements D1PreparedStatement {
  bind() { return this; }
  async first<T>() { return { ok: 1 } as T; }
  async run(): Promise<D1Result> { return { success: true }; }
  async all<T>() { return { results: [] as T[], success: true }; }
}

function environment(failures = new Set<string>()): WorkerEnv {
  const database: D1Database = {
    prepare() {
      if (failures.has("d1")) throw new Error("secret d1 detail");
      return new Statement();
    },
    async batch() { return []; },
  };
  const hosts: KvNamespace = {
    async get() {
      if (failures.has("kv")) throw new Error("secret kv detail");
      return null;
    },
    async put() {},
    async delete() {},
  };
  const releases: R2Bucket = {
    async get() { return null; },
    async head() {
      if (failures.has("r2")) throw new Error("secret r2 detail");
      return null;
    },
    async put() { return {}; },
  };
  const core: Fetcher = {
    async fetch() {
      return new Response(null, { status: failures.has("core") ? 503 : 204 });
    },
  };
  const stub: DurableObjectStub = {
    async fetch() {
      return new Response(null, { status: failures.has("durable_object") ? 503 : 200 });
    },
  };
  const sites: DurableObjectNamespace = {
    idFromName: () => ({} as DurableObjectId),
    get: () => stub,
  };
  return {
    HOSTS: hosts,
    RELEASES: releases,
    REGISTRY: database,
    SITES: sites,
    CONTROL_HOSTNAME: "control.service.dev",
    ANONYMOUS_DEMO_HOSTNAME: "demo.service.dev",
    SAAS_CNAME_TARGET: "customers.service.dev",
    CF_ZONE_ID: "zone-id",
    CF_API_TOKEN: providerCapability,
    INTERNAL_DO_TOKEN: internalCapability,
    DIAGNOSTIC_TOKEN: diagnosticCapability,
    CORE: core,
  };
}

function request(token = diagnosticCapability) {
  return new Request("https://control.service.dev/internal/doctor", {
    headers: { Authorization: `Bearer ${token}` },
  });
}

test("host doctor checks every adapter dependency without mutating service data", async () => {
  const response = await handleDoctorRequest(request(), environment(), () => 17);

  assert.equal(response.status, 200);
  assert.equal(response.headers.get("cache-control"), "no-store");
  const report = await response.json() as {
    status: string;
    checks: Array<{ component: string; status: string; diagnostic_code: string | null }>;
  };
  assert.equal(report.status, "ok");
  assert.deepEqual(report.checks.map((check) => check.component), [
    "configuration",
    "d1",
    "durable_object",
    "kv",
    "r2",
    "core",
  ]);
  assert.ok(report.checks.every((check) => check.status === "ok"));
  assert.equal(JSON.stringify(report).includes("secret"), false);
});

test("doctor exhausts independent checks and returns only stable diagnostic codes", async () => {
  const response = await handleDoctorRequest(
    request(),
    environment(new Set(["d1", "r2", "core"])),
    () => 23,
  );

  assert.equal(response.status, 503);
  const report = await response.json() as {
    status: string;
    checks: Array<{ diagnostic_code: string | null }>;
  };
  assert.equal(report.status, "degraded");
  assert.deepEqual(report.checks.flatMap((check) => check.diagnostic_code ?? []), [
    "CF_D1_UNREACHABLE",
    "CF_R2_UNREACHABLE",
    "CF_CORE_UNHEALTHY",
  ]);
  assert.equal(JSON.stringify(report).includes("secret"), false);
});

test("doctor rejects missing or incorrect operator capabilities", async () => {
  assert.equal((await handleDoctorRequest(
    new Request("https://control.service.dev/internal/doctor"),
    environment(),
  )).status, 401);
  assert.equal((await handleDoctorRequest(request("wrong-token"), environment())).status, 401);
  assert.equal((await handleDoctorRequest(
    new Request("https://control.service.dev/internal/doctor", {
      method: "POST",
      headers: { Authorization: `Bearer ${diagnosticCapability}` },
    }),
    environment(),
  )).status, 405);
});
