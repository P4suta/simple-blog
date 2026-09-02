import assert from "node:assert/strict";
import test from "node:test";

import type {
  DurableObjectState,
  DurableObjectStorage,
  Fetcher,
  SiteCoordinatorEnv,
} from "../src/bindings.ts";
import { SiteCoordinator } from "../src/site-object.ts";

const internalCapability = `test-only-internal-${"x".repeat(32)}`;

class Storage implements DurableObjectStorage {
  readonly values = new Map<string, unknown>();
  alarm: number | null = null;
  async get<T>(key: string): Promise<T | undefined> { return this.values.get(key) as T | undefined; }
  async put<T>(key: string, value: T): Promise<void> { this.values.set(key, structuredClone(value)); }
  async delete(key: string): Promise<boolean> { return this.values.delete(key); }
  async getAlarm(): Promise<number | null> { return this.alarm; }
  async setAlarm(value: number): Promise<void> { this.alarm = value; }
  async deleteAlarm(): Promise<void> { this.alarm = null; }
}

function setup(core: Fetcher = { async fetch() { return new Response(null, { status: 204 }); } }) {
  const storage = new Storage();
  const state: DurableObjectState = { storage };
  const env: SiteCoordinatorEnv = { INTERNAL_DO_TOKEN: internalCapability, CORE: core };
  return { object: new SiteCoordinator(state, env), storage };
}

function internal(path: string, body?: unknown, token = internalCapability): Request {
  return new Request(`https://site.internal${path}`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

test("Durable Object rejects public calls and gates engagement on the active release", async () => {
  const state = setup();
  assert.equal((await state.object.fetch(internal("/views/7", undefined, "wrong-token"))).status, 401);
  assert.equal((await state.object.fetch(internal("/likes/7", { operation: "like" }))).status, 404);

  const scheduled = await state.object.fetch(internal("/publication/state", {
    site_id: "12345678-1234-1234-1234-123456789abc",
    public_content_ids: [7],
    next_publish_at: "2026-09-02T13:00:00.000Z",
  }));
  assert.equal(scheduled.status, 204);
  assert.equal(state.storage.alarm, Date.parse("2026-09-02T13:00:00.000Z"));

  assert.equal((await state.object.fetch(internal("/views/7"))).status, 204);
  assert.equal((await state.object.fetch(internal("/likes/7", { operation: "like" }))).status, 204);
  assert.equal((await state.object.fetch(internal("/likes/7", { operation: "unlike" }))).status, 204);
  assert.deepEqual(state.storage.values.get("engagement/7"), { likes: 0, views: 1 });
});

test("Durable Object exposes an authenticated read-only diagnostic probe", async () => {
  const state = setup();
  state.storage.alarm = Date.parse("2026-09-02T13:00:00.000Z");

  const response = await state.object.fetch(internal("/diagnostics"));

  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { status: "ok", alarm_scheduled: true });
  assert.equal(state.storage.values.size, 0);
});

test("alarm invokes the Core publisher and installs a durable retry after downstream failure", async () => {
  const calls: Request[] = [];
  const core: Fetcher = {
    async fetch(request) {
      calls.push(request);
      return new Response("unavailable", { status: 503 });
    },
  };
  const state = setup(core);
  await state.object.fetch(internal("/publication/state", {
    site_id: "12345678-1234-1234-1234-123456789abc",
    public_content_ids: [],
    next_publish_at: "2026-09-02T13:00:00.000Z",
  }));
  const before = Date.now();

  await state.object.alarm();

  assert.equal(calls.length, 1);
  assert.equal(new URL(calls[0]!.url).pathname, "/internal/sites/12345678-1234-1234-1234-123456789abc/publish");
  assert.ok((state.storage.alarm ?? 0) >= before + 4 * 60 * 1000);
});
