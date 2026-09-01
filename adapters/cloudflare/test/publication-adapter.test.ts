import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import type { KvNamespace, R2Bucket, R2ObjectBody } from "../src/bindings.ts";
import {
  CloudflareReleaseActivator,
  type ActivationStorage,
  type ReleaseCandidate,
} from "../src/publication.ts";

const fixture = JSON.parse(
  readFileSync(new URL("../../../contracts/release-resolution-v1.json", import.meta.url), "utf8"),
) as { active_release: string; manifest: unknown };
const oldRelease = "d".repeat(64);
const siteId = "12345678-1234-1234-1234-123456789abc";

class ObjectBody implements R2ObjectBody {
  readonly bytes: Uint8Array;
  readonly customMetadata: Record<string, string>;
  constructor(
    bytes: Uint8Array,
    customMetadata: Record<string, string>,
  ) {
    this.bytes = bytes;
    this.customMetadata = customMetadata;
  }
  async arrayBuffer(): Promise<ArrayBuffer> {
    const copy = new Uint8Array(this.bytes.length);
    copy.set(this.bytes);
    return copy.buffer;
  }
}

class Bucket implements R2Bucket {
  readonly values = new Map<string, ObjectBody>();
  readonly gets: string[] = [];
  async get(key: string) {
    this.gets.push(key);
    return this.values.get(key) ?? null;
  }
  async head() { throw new Error("activation must verify object bytes, not metadata alone"); }
  async put() { return {}; }
}

class Kv implements KvNamespace {
  readonly writes: Array<[string, string]> = [];
  fail = false;
  async get() { return null; }
  async put(key: string, value: string) {
    if (this.fail) throw new Error("injected_kv_failure");
    this.writes.push([key, value]);
  }
  async delete() {}
}

class Storage implements ActivationStorage {
  readonly values = new Map<string, unknown>([["active_release", oldRelease]]);
  async get<T>(key: string): Promise<T | undefined> { return this.values.get(key) as T | undefined; }
  async put<T>(key: string, value: T): Promise<void> { this.values.set(key, value); }
  async delete(key: string): Promise<boolean> { return this.values.delete(key); }
}

function candidate(): ReleaseCandidate {
  return {
    siteId,
    domain: "writing.example.com",
    expectedRelease: oldRelease,
    replacementRelease: fixture.active_release,
    nextPublishAt: null,
  };
}

async function sha256(bytes: Uint8Array): Promise<string> {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

async function setup() {
  const bucket = new Bucket();
  const manifestBytes = new TextEncoder().encode(JSON.stringify(fixture.manifest));
  bucket.values.set(
    `sites/${siteId}/manifests/${fixture.active_release}.json`,
    new ObjectBody(manifestBytes, {
      "simple-blog-kind": "manifest",
      blake3: fixture.active_release,
      sha256: await sha256(manifestBytes),
    }),
  );
  for (const objectId of ["b".repeat(64), "c".repeat(64)]) {
    const bytes = new Uint8Array([1]);
    bucket.values.set(
      `sites/${siteId}/objects/${objectId}`,
      new ObjectBody(bytes, {
        "simple-blog-kind": "object",
        blake3: objectId,
        sha256: await sha256(bytes),
      }),
    );
  }
  const hosts = new Kv();
  const storage = new Storage();
  return {
    bucket,
    hosts,
    storage,
    activator: new CloudflareReleaseActivator(bucket, hosts, storage),
  };
}

test("activation verifies the complete immutable graph before one visible KV pointer write", async () => {
  const state = await setup();

  const result = await state.activator.activate(candidate());

  assert.equal(result.changed, true);
  assert.equal(state.storage.values.get("active_release"), fixture.active_release);
  assert.equal(state.hosts.writes.length, 1);
  assert.equal(state.hosts.writes[0]![0], "hosts/writing.example.com");
  assert.deepEqual(state.bucket.gets.sort(), [
    `sites/${siteId}/manifests/${fixture.active_release}.json`,
    `sites/${siteId}/objects/${"b".repeat(64)}`,
    `sites/${siteId}/objects/${"c".repeat(64)}`,
  ].sort());
});

test("missing objects and CAS conflicts leave the visible pointer untouched", async () => {
  const missing = await setup();
  missing.bucket.values.delete(`sites/${siteId}/objects/${"c".repeat(64)}`);
  await assert.rejects(missing.activator.activate(candidate()), /release_object_missing/);
  assert.equal(missing.storage.values.get("active_release"), oldRelease);
  assert.equal(missing.hosts.writes.length, 0);

  const conflict = await setup();
  const value = candidate();
  value.expectedRelease = "e".repeat(64);
  await assert.rejects(conflict.activator.activate(value), /release_activation_conflict/);
  assert.equal(conflict.hosts.writes.length, 0);
});

test("unverified transport metadata cannot become visible", async () => {
  const state = await setup();
  state.bucket.values.get(`sites/${siteId}/objects/${"b".repeat(64)}`)!
    .customMetadata.sha256 = "not-a-digest";

  await assert.rejects(state.activator.activate(candidate()), /release_object_metadata_invalid/);
  assert.equal(state.storage.values.get("active_release"), oldRelease);
  assert.equal(state.hosts.writes.length, 0);
});

test("payload tampering cannot become visible when retained metadata still looks valid", async () => {
  const state = await setup();
  const key = `sites/${siteId}/objects/${"b".repeat(64)}`;
  const stored = state.bucket.values.get(key)!;
  state.bucket.values.set(key, new ObjectBody(
    new Uint8Array([2]),
    structuredClone(stored.customMetadata),
  ));

  await assert.rejects(state.activator.activate(candidate()), /release_object_integrity_invalid/);
  assert.equal(state.storage.values.get("active_release"), oldRelease);
  assert.equal(state.hosts.writes.length, 0);
});

test("a failed KV activation retains old visibility and is safely retryable", async () => {
  const state = await setup();
  state.hosts.fail = true;
  await assert.rejects(state.activator.activate(candidate()), /injected_kv_failure/);
  assert.equal(state.storage.values.get("active_release"), oldRelease);
  assert.equal(state.storage.values.get("pending_release"), fixture.active_release);

  state.hosts.fail = false;
  const retried = await state.activator.activate(candidate());
  assert.equal(retried.changed, true);
  assert.equal(state.storage.values.get("active_release"), fixture.active_release);
  assert.equal(state.storage.values.has("pending_release"), false);
});

test("an idempotent retry clears a crash-left pending release", async () => {
  const state = await setup();
  state.storage.values.set("active_release", fixture.active_release);
  state.storage.values.set("pending_release", fixture.active_release);

  const retried = await state.activator.activate(candidate());

  assert.equal(retried.changed, false);
  assert.equal(state.storage.values.get("active_release"), fixture.active_release);
  assert.equal(state.storage.values.has("pending_release"), false);
});
