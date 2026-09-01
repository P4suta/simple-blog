import type { R2Bucket, R2ObjectBody } from "./bindings.ts";
import { normalizeDomain } from "./domain.ts";
import { parseManifest } from "./release.ts";

export interface StageObjectInput {
  siteId: string;
  objectId: string;
  sha256: string;
  bytes: Uint8Array;
}

export interface StageManifestInput {
  siteId: string;
  releaseId: string;
  domain: string;
  sha256: string;
  bytes: Uint8Array;
}

export interface StageResult {
  created: boolean;
}

const DIGEST = /^[0-9a-f]{64}$/;
const SITE_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/**
 * Stages immutable Core compiler output without making it publicly visible.
 *
 * WebCrypto cannot calculate BLAKE3, so the Core remains responsible for the
 * content-addressed ID. This boundary independently checks SHA-256 over the
 * transferred bytes and records both digests before activation is allowed.
 */
export class R2ReleaseStager {
  private readonly bucket: R2Bucket;

  constructor(bucket: R2Bucket) {
    this.bucket = bucket;
  }

  async stageObject(input: StageObjectInput): Promise<StageResult> {
    validateSiteAndDigests(input.siteId, input.objectId, input.sha256);
    await verifyTransportChecksum(input.bytes, input.sha256);
    return this.stageImmutable(
      `sites/${input.siteId}/objects/${input.objectId}`,
      "object",
      input.objectId,
      input.sha256,
      input.bytes,
      "application/octet-stream",
    );
  }

  async stageManifest(input: StageManifestInput): Promise<StageResult> {
    validateSiteAndDigests(input.siteId, input.releaseId, input.sha256);
    if (normalizeDomain(input.domain) !== input.domain) {
      throw new Error("release_domain_invalid");
    }
    await verifyTransportChecksum(input.bytes, input.sha256);
    const manifest = decodeManifest(input.bytes);
    if (manifest.canonical_origin !== `https://${input.domain}`) {
      throw new Error("release_origin_mismatch");
    }
    return this.stageImmutable(
      `sites/${input.siteId}/manifests/${input.releaseId}.json`,
      "manifest",
      input.releaseId,
      input.sha256,
      input.bytes,
      "application/json; charset=utf-8",
    );
  }

  private async stageImmutable(
    key: string,
    kind: "manifest" | "object",
    blake3: string,
    sha256: string,
    bytes: Uint8Array,
    contentType: string,
  ): Promise<StageResult> {
    const stored = await this.bucket.put(key, copiedBytes(bytes), {
      onlyIf: { etagDoesNotMatch: "*" },
      sha256,
      customMetadata: {
        "simple-blog-kind": kind,
        blake3,
        sha256,
      },
      httpMetadata: { contentType },
    });
    if (stored !== null) return { created: true };

    const existing = await this.bucket.get(key);
    if (existing === null) throw new Error(`release_${kind}_race_missing`);
    await verifyExisting(existing, kind, blake3, sha256);
    return { created: false };
  }
}

function validateSiteAndDigests(siteId: string, blake3: string, sha256: string): void {
  if (!SITE_ID.test(siteId)) throw new Error("release_site_id_invalid");
  if (!DIGEST.test(blake3)) throw new Error("release_blake3_invalid");
  if (!DIGEST.test(sha256)) throw new Error("release_sha256_invalid");
}

function decodeManifest(bytes: Uint8Array) {
  try {
    const json = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return parseManifest(JSON.parse(json) as unknown);
  } catch (error) {
    if (error instanceof Error && error.message.startsWith("release_")) throw error;
    throw new Error("release_manifest_invalid", { cause: error });
  }
}

async function verifyExisting(
  object: R2ObjectBody,
  kind: "manifest" | "object",
  blake3: string,
  sha256: string,
): Promise<void> {
  if (
    object.customMetadata?.["simple-blog-kind"] !== kind ||
    object.customMetadata?.["blake3"] !== blake3 ||
    object.customMetadata?.["sha256"] !== sha256
  ) {
    throw new Error(`release_${kind}_collision`);
  }
  const existing = new Uint8Array(await object.arrayBuffer());
  try {
    await verifyTransportChecksum(existing, sha256);
  } catch (error) {
    throw new Error(`release_${kind}_collision`, { cause: error });
  }
}

async function verifyTransportChecksum(bytes: Uint8Array, expected: string): Promise<void> {
  const digest = await crypto.subtle.digest("SHA-256", copiedBuffer(bytes));
  const actual = [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
  if (actual !== expected) throw new Error("release_transport_checksum_mismatch");
}

function copiedBytes(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  return new Uint8Array(copiedBuffer(bytes));
}

function copiedBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer;
}
