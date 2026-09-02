import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import type {
  R2Bucket,
  R2ObjectBody,
  R2PutOptions,
} from "../src/bindings.ts";
import { R2ReleaseStager } from "../src/staging.ts";

const fixture = JSON.parse(
  readFileSync(new URL("../../../contracts/release-resolution-v1.json", import.meta.url), "utf8"),
) as { active_release: string; manifest: unknown };
const siteId = "12345678-1234-1234-1234-123456789abc";

class Body implements R2ObjectBody {
  readonly bytes: Uint8Array;
  readonly customMetadata?: Record<string, string>;
  readonly httpMetadata?: { contentType?: string };

  constructor(bytes: Uint8Array, options?: R2PutOptions) {
    this.bytes = bytes.slice();
    this.customMetadata = options?.customMetadata;
    this.httpMetadata = options?.httpMetadata;
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    return this.bytes.slice().buffer;
  }
}

class Bucket implements R2Bucket {
  readonly values = new Map<string, Body>();
  readonly puts: Array<{ key: string; options?: R2PutOptions }> = [];
  failNext = false;

  async get(key: string) { return this.values.get(key) ?? null; }
  async head(key: string) { return this.values.get(key) ?? null; }

  async put(
    key: string,
    value: ArrayBuffer | Uint8Array | string,
    options?: R2PutOptions,
  ) {
    this.puts.push({ key, options });
    if (this.failNext) {
      this.failNext = false;
      throw new Error("injected_r2_failure");
    }
    if (options?.onlyIf?.etagDoesNotMatch === "*" && this.values.has(key)) return null;
    const bytes = typeof value === "string"
      ? new TextEncoder().encode(value)
      : value instanceof Uint8Array
      ? value
      : new Uint8Array(value);
    const body = new Body(bytes, options);
    this.values.set(key, body);
    return body;
  }
}

async function sha256(bytes: Uint8Array): Promise<string> {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

test("immutable object staging verifies transport bytes and is idempotent", async () => {
  const bucket = new Bucket();
  const stager = new R2ReleaseStager(bucket);
  const bytes = new TextEncoder().encode("immutable page");
  const digest = await sha256(bytes);
  const objectId = "b".repeat(64);

  assert.deepEqual(
    await stager.stageObject({ siteId, objectId, sha256: digest, bytes }),
    { created: true },
  );
  assert.deepEqual(
    await stager.stageObject({ siteId, objectId, sha256: digest, bytes }),
    { created: false },
  );

  const key = `sites/${siteId}/objects/${objectId}`;
  assert.equal(bucket.puts[0]?.key, key);
  assert.equal(bucket.puts[0]?.options?.onlyIf?.etagDoesNotMatch, "*");
  assert.deepEqual(bucket.values.get(key)?.customMetadata, {
    "simple-blog-kind": "object",
    blake3: objectId,
    sha256: digest,
  });
});

test("checksum errors and conflicting immutable keys never overwrite R2", async () => {
  const bucket = new Bucket();
  const stager = new R2ReleaseStager(bucket);
  const bytes = new TextEncoder().encode("expected");
  const digest = await sha256(bytes);
  const objectId = "c".repeat(64);

  await assert.rejects(
    stager.stageObject({ siteId, objectId, sha256: "0".repeat(64), bytes }),
    /release_transport_checksum_mismatch/,
  );
  assert.equal(bucket.puts.length, 0);

  bucket.values.set(
    `sites/${siteId}/objects/${objectId}`,
    new Body(new TextEncoder().encode("other"), {
      customMetadata: {
        "simple-blog-kind": "object",
        blake3: objectId,
        sha256: "d".repeat(64),
      },
    }),
  );
  await assert.rejects(
    stager.stageObject({ siteId, objectId, sha256: digest, bytes }),
    /release_object_collision/,
  );
  assert.equal(new TextDecoder().decode(bucket.values.get(
    `sites/${siteId}/objects/${objectId}`,
  )?.bytes), "other");
});

test("manifest staging validates schema and canonical origin before writing", async () => {
  const bucket = new Bucket();
  const stager = new R2ReleaseStager(bucket);
  const bytes = new TextEncoder().encode(JSON.stringify(fixture.manifest));
  const digest = await sha256(bytes);

  assert.deepEqual(await stager.stageManifest({
    siteId,
    releaseId: fixture.active_release,
    domain: "writing.example.com",
    sha256: digest,
    bytes,
  }), { created: true });

  const invalidBytes = new TextEncoder().encode(JSON.stringify({ ...fixture.manifest as object,
    canonical_origin: "https://elsewhere.example.com" }));
  await assert.rejects(stager.stageManifest({
    siteId,
    releaseId: "e".repeat(64),
    domain: "writing.example.com",
    sha256: await sha256(invalidBytes),
    bytes: invalidBytes,
  }), /release_origin_mismatch/);
  assert.equal(bucket.puts.length, 1);
});

test("transient R2 failure is retryable without weakening create-only writes", async () => {
  const bucket = new Bucket();
  bucket.failNext = true;
  const stager = new R2ReleaseStager(bucket);
  const bytes = new TextEncoder().encode("retry me");
  const input = {
    siteId,
    objectId: "f".repeat(64),
    sha256: await sha256(bytes),
    bytes,
  };

  await assert.rejects(stager.stageObject(input), /injected_r2_failure/);
  assert.deepEqual(await stager.stageObject(input), { created: true });
  assert.equal(bucket.puts[0]?.options?.onlyIf?.etagDoesNotMatch, "*");
  assert.equal(bucket.puts[1]?.options?.onlyIf?.etagDoesNotMatch, "*");
});
