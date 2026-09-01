import assert from "node:assert/strict";
import test from "node:test";

import type {
  DurableObjectId,
  DurableObjectNamespace,
  DurableObjectStub,
  KvNamespace,
  R2Bucket,
  R2ObjectBody,
} from "../src/bindings.ts";
import {
  DurableObjectEngagement,
  KvHostDirectory,
  R2MediaDirectory,
  R2ReleaseDirectory,
} from "../src/cloudflare-store.ts";

class Kv implements KvNamespace {
  readonly value: unknown;
  constructor(value: unknown) { this.value = value; }
  async get() { return this.value; }
  async put() {}
  async delete() {}
}

class R2Object implements R2ObjectBody {
  readonly bytes: Uint8Array;
  readonly customMetadata?: Record<string, string>;
  readonly httpMetadata?: { contentType?: string };

  constructor(
    bytes: Uint8Array,
    customMetadata?: Record<string, string>,
    httpMetadata?: { contentType?: string },
  ) {
    this.bytes = bytes;
    this.customMetadata = customMetadata;
    this.httpMetadata = httpMetadata;
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    const copy = new Uint8Array(this.bytes.length);
    copy.set(this.bytes);
    return copy.buffer;
  }
}

class R2 implements R2Bucket {
  readonly reads: string[] = [];
  readonly objects: Map<string, R2ObjectBody>;
  constructor(objects: Map<string, R2ObjectBody>) { this.objects = objects; }
  async get(key: string) {
    this.reads.push(key);
    return this.objects.get(key) ?? null;
  }
  async put() { return {}; }
}

test("KV host records and R2 keys are strict, site-scoped adapter boundaries", async () => {
  const releaseId = "a".repeat(64);
  const objectId = "b".repeat(64);
  const siteId = "12345678-1234-1234-1234-123456789abc";
  const host = await new KvHostDirectory(new Kv({
    format_version: 1,
    site_id: siteId,
    active_release: releaseId,
    state: "active",
  })).lookup("writing.example.com");
  assert.deepEqual(host, { siteId, activeRelease: releaseId, state: "active" });

  const manifest = {
    format_version: 1,
    compiler_version: "test",
    public_revision: 1,
    canonical_origin: "https://writing.example.com",
    routes: {
      "/404/": {
        kind: "asset",
        object_id: objectId,
        content_type: "text/plain",
        cache_control: "public, max-age=0, must-revalidate",
        status: 404,
      },
    },
  };
  const r2 = new R2(new Map([
    [
      `sites/${siteId}/manifests/${releaseId}.json`,
      new R2Object(new TextEncoder().encode(JSON.stringify(manifest)), {
        "simple-blog-kind": "manifest",
        blake3: releaseId,
        sha256: "a".repeat(64),
      }),
    ],
    [
      `sites/${siteId}/objects/${objectId}`,
      new R2Object(new TextEncoder().encode("missing"), {
        "simple-blog-kind": "object",
        blake3: objectId,
        sha256: "a".repeat(64),
      }),
    ],
  ]));
  const reader = new R2ReleaseDirectory(r2).forSite(host!);
  assert.deepEqual(await reader.manifest(releaseId), manifest);
  assert.equal(new TextDecoder().decode(await reader.object(objectId) ?? undefined), "missing");
  assert.deepEqual(r2.reads, [
    `sites/${siteId}/manifests/${releaseId}.json`,
    `sites/${siteId}/objects/${objectId}`,
  ]);

  const invalidMetadata = new R2(new Map([
    [
      `sites/${siteId}/objects/${objectId}`,
      new R2Object(new TextEncoder().encode("missing"), {
        "simple-blog-kind": "object",
        blake3: objectId,
      }),
    ],
  ]));
  await assert.rejects(
    new R2ReleaseDirectory(invalidMetadata).forSite(host!).object(objectId),
    /release_object_metadata_invalid/,
  );

  await assert.rejects(
    new KvHostDirectory(new Kv({ ...host, future: true })).lookup("writing.example.com"),
    /host_mapping_invalid/,
  );
});

test("R2 media requires trusted type metadata and Durable Object writes carry an internal token", async () => {
  const siteId = "12345678-1234-1234-1234-123456789abc";
  const filename = `${"c".repeat(64)}.gif`;
  const bucket = new R2(new Map([
    [
      `sites/${siteId}/media/${filename}`,
      new R2Object(
        new Uint8Array([1, 2, 3]),
        { "simple-blog-kind": "media" },
        { contentType: "image/gif" },
      ),
    ],
  ]));
  const media = await new R2MediaDirectory(bucket).get(siteId, filename);
  assert.equal(media?.contentType, "image/gif");
  assert.deepEqual(media?.bytes, new Uint8Array([1, 2, 3]));

  const requests: Request[] = [];
  const stub: DurableObjectStub = {
    async fetch(request) {
      requests.push(request);
      return new Response(null, { status: 204 });
    },
  };
  const namespace: DurableObjectNamespace = {
    idFromName: () => ({} as DurableObjectId),
    get: () => stub,
  };
  const engagement = new DurableObjectEngagement(namespace, "internal-secret");
  await engagement.recordView(siteId, 7);
  await engagement.toggleLike(siteId, 7, "like");
  assert.equal(requests[0]?.headers.get("authorization"), "Bearer internal-secret");
  assert.equal(new URL(requests[0]!.url).pathname, "/views/7");
  assert.deepEqual(await requests[1]!.json(), { operation: "like" });
});
