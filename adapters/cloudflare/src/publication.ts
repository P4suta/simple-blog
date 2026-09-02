import type { KvNamespace, R2Bucket } from "./bindings.ts";
import { normalizeDomain } from "./domain.ts";
import { verifiedReleaseBytes } from "./integrity.ts";
import { parseManifest, type ReleaseManifest } from "./release.ts";

export interface ActivationStorage {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
}

export interface ReleaseCandidate {
  siteId: string;
  domain: string;
  expectedRelease: string | null;
  replacementRelease: string;
  nextPublishAt: string | null;
}

export interface ActivationResult {
  changed: boolean;
  publicContentIds: number[];
}

const DIGEST = /^[0-9a-f]{64}$/;
const SITE_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export class CloudflareReleaseActivator {
  private readonly bucket: R2Bucket;
  private readonly hosts: KvNamespace;
  private readonly storage: ActivationStorage;

  constructor(bucket: R2Bucket, hosts: KvNamespace, storage: ActivationStorage) {
    this.bucket = bucket;
    this.hosts = hosts;
    this.storage = storage;
  }

  async activate(candidate: ReleaseCandidate): Promise<ActivationResult> {
    validateCandidate(candidate);
    const active = await this.storage.get<string>("active_release");
    const expected = candidate.expectedRelease ?? undefined;
    const pending = await this.storage.get<string>("pending_release");
    if (active === candidate.replacementRelease) {
      const manifest = await this.verifyGraph(candidate);
      await this.writeVisiblePointer(candidate);
      await this.storage.delete("pending_release");
      return { changed: false, publicContentIds: publicContentIds(manifest) };
    }
    if (active !== expected || (pending !== undefined && pending !== candidate.replacementRelease)) {
      throw new Error("release_activation_conflict");
    }
    const manifest = await this.verifyGraph(candidate);
    await this.storage.put("pending_release", candidate.replacementRelease);
    await this.writeVisiblePointer(candidate);
    await this.storage.put("active_release", candidate.replacementRelease);
    await this.storage.delete("pending_release");
    return { changed: true, publicContentIds: publicContentIds(manifest) };
  }

  private async verifyGraph(candidate: ReleaseCandidate): Promise<ReleaseManifest> {
    const manifestKey =
      `sites/${candidate.siteId}/manifests/${candidate.replacementRelease}.json`;
    const stored = await this.bucket.get(manifestKey);
    if (stored === null) throw new Error("release_manifest_missing");
    const manifestBytes = await verifiedReleaseBytes(
      stored,
      "manifest",
      candidate.replacementRelease,
    );
    const manifest = parseManifest(
      JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(manifestBytes)) as unknown,
    );
    if (manifest.canonical_origin !== `https://${candidate.domain}`) {
      throw new Error("release_origin_mismatch");
    }
    const objectIds = new Set<string>();
    for (const route of Object.values(manifest.routes)) {
      if (route.kind === "asset") objectIds.add(route.object_id);
    }
    for (const objectId of [...objectIds].sort()) {
      const object = await this.bucket.get(`sites/${candidate.siteId}/objects/${objectId}`);
      if (object === null) throw new Error(`release_object_missing:${objectId}`);
      await verifiedReleaseBytes(object, "object", objectId);
    }
    return manifest;
  }

  private async writeVisiblePointer(candidate: ReleaseCandidate): Promise<void> {
    await this.hosts.put(`hosts/${candidate.domain}`, JSON.stringify({
      format_version: 1,
      site_id: candidate.siteId,
      active_release: candidate.replacementRelease,
      state: "active",
    }));
  }
}

function publicContentIds(manifest: ReleaseManifest): number[] {
  return [...new Set(
    Object.values(manifest.routes)
      .filter((route) => route.kind === "asset" && route.status === 200)
      .flatMap((route) => route.kind === "asset" && route.content_id !== undefined
        ? [route.content_id]
        : []),
  )].sort((left, right) => left - right);
}

function validateCandidate(candidate: ReleaseCandidate): void {
  if (!SITE_ID.test(candidate.siteId) || normalizeDomain(candidate.domain) !== candidate.domain ||
      !DIGEST.test(candidate.replacementRelease) ||
      (candidate.expectedRelease !== null && !DIGEST.test(candidate.expectedRelease)) ||
      (candidate.nextPublishAt !== null && !Number.isFinite(Date.parse(candidate.nextPublishAt)))) {
    throw new Error("release_candidate_invalid");
  }
}
