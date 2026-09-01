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
  readonly heads: string[] = [];
  async get(key: string) { return this.values.get(key) ?? null; }
  async head(key: string) { this.heads.push(key); return this.values.get(key) ?? null; }
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

function setup() {
  const bucket = new Bucket();
  const manifestBytes = new TextEncoder().encode(JSON.stringify(fixture.manifest));
  bucket.values.set(
    `sites/${siteId}/manifests/${fixture.active_release}.json`,
    new ObjectBody(manifestBytes, {
      "simple-blog-kind": "manifest",
      blake3: fixture.active_release,
      sha256: "a".repeat(64),
    }),
  );
  for (const objectId of ["b".repeat(64), "c".repeat(64)]) {
    bucket.values.set(
      `sites/${siteId}/objects/${objectId}`,
      new ObjectBody(new Uint8Array([1]), {
        "simple-blog-kind": "object",
        blake3: objectId,
        sha256: "a".repeat(64),
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
  const state = setup();

  const result = await state.activator.activate(candidate());

  assert.equal(result.changed, true);
  assert.equal(state.storage.values.get("active_release"), fixture.active_release);
  assert.equal(state.hosts.writes.length, 1);
  assert.equal(state.hosts.writes[0]![0], "hosts/writing.example.com");
  assert.deepEqual(state.bucket.heads.sort(), [
    `sites/${siteId}/objects/${"b".repeat(64)}`,
    `sites/${siteId}/objects/${"c".repeat(64)}`,
  ].sort());
});

test("missing objects and CAS conflicts leave the visible pointer untouched", async () => {
  const missing = setup();
  missing.bucket.values.delete(`sites/${siteId}/objects/${"c".repeat(64)}`);
  await assert.rejects(missing.activator.activate(candidate()), /release_object_missing/);
  assert.equal(missing.storage.values.get("active_release"), oldRelease);
  assert.equal(missing.hosts.writes.length, 0);

  const conflict = setup();
  const value = candidate();
  value.expectedRelease = "e".repeat(64);
  await assert.rejects(conflict.activator.activate(value), /release_activation_conflict/);
  assert.equal(conflict.hosts.writes.length, 0);
});

test("unverified transport metadata cannot become visible", async () => {
  const state = setup();
  state.bucket.values.get(`sites/${siteId}/objects/${"b".repeat(64)}`)!
    .customMetadata.sha256 = "not-a-digest";

  await assert.rejects(state.activator.activate(candidate()), /release_object_metadata_invalid/);
  assert.equal(state.storage.values.get("active_release"), oldRelease);
  assert.equal(state.hosts.writes.length, 0);
});

test("a failed KV activation retains old visibility and is safely retryable", async () => {
  const state = setup();
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
