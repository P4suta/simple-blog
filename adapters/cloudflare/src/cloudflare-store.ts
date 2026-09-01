import type {
  DurableObjectNamespace,
  KvNamespace,
  R2Bucket,
  R2ObjectBody,
} from "./bindings.ts";
import type { DomainRegistrationState } from "./domain.ts";
import type {
  EngagementWriter,
  HostDirectory,
  HostedSite,
  MediaDirectory,
  MediaObject,
  ReleaseDirectory,
} from "./public.ts";
import { parseManifest, type ReleaseReader } from "./release.ts";

const DIGEST = /^[0-9a-f]{64}$/;
const SITE_ID = /^[0-9a-f-]{36}$/;
const STATES = new Set<DomainRegistrationState>([
  "pending_ownership",
  "pending_certificate",
  "pending_dns",
  "ready_for_owner",
  "active",
  "action_required",
]);

export class KvHostDirectory implements HostDirectory {
  private readonly hosts: KvNamespace;

  constructor(hosts: KvNamespace) {
    this.hosts = hosts;
  }

  async lookup(hostname: string): Promise<HostedSite | null> {
    const value = await this.hosts.get(`hosts/${hostname}`, "json");
    if (value === null) return null;
    if (!isRecord(value) || !exactKeys(value, ["active_release", "format_version", "site_id", "state"])) {
      throw new Error("host_mapping_invalid");
    }
    if (
      value.format_version !== 1 ||
      typeof value.site_id !== "string" ||
      !SITE_ID.test(value.site_id) ||
      typeof value.active_release !== "string" ||
      !DIGEST.test(value.active_release) ||
      typeof value.state !== "string" ||
      !STATES.has(value.state as DomainRegistrationState)
    ) {
      throw new Error("host_mapping_invalid");
    }
    return {
      siteId: value.site_id,
      activeRelease: value.active_release,
      state: value.state as DomainRegistrationState,
    };
  }
}

export class R2ReleaseDirectory implements ReleaseDirectory {
  private readonly bucket: R2Bucket;

  constructor(bucket: R2Bucket) {
    this.bucket = bucket;
  }

  forSite(site: HostedSite): ReleaseReader {
    return new R2ReleaseReader(this.bucket, site.siteId, site.activeRelease);
  }
}

class R2ReleaseReader implements ReleaseReader {
  private readonly bucket: R2Bucket;
  private readonly siteId: string;
  private readonly releaseId: string;

  constructor(
    bucket: R2Bucket,
    siteId: string,
    releaseId: string,
  ) {
    this.bucket = bucket;
    this.siteId = siteId;
    this.releaseId = releaseId;
  }

  async activeRelease(): Promise<string> {
    return this.releaseId;
  }

  async manifest(releaseId: string): Promise<unknown | null> {
    if (releaseId !== this.releaseId || !DIGEST.test(releaseId)) return null;
    const object = await this.bucket.get(`sites/${this.siteId}/manifests/${releaseId}.json`);
    if (object === null) return null;
    verifyMetadata(object, "manifest", releaseId);
    const value = JSON.parse(new TextDecoder().decode(await object.arrayBuffer())) as unknown;
    return parseManifest(value);
  }

  async object(objectId: string): Promise<Uint8Array | null> {
    if (!DIGEST.test(objectId)) return null;
    const object = await this.bucket.get(`sites/${this.siteId}/objects/${objectId}`);
    if (object === null) return null;
    verifyMetadata(object, "object", objectId);
    return new Uint8Array(await object.arrayBuffer());
  }
}

export class R2MediaDirectory implements MediaDirectory {
  private readonly bucket: R2Bucket;

  constructor(bucket: R2Bucket) {
    this.bucket = bucket;
  }

  async get(siteId: string, filename: string): Promise<MediaObject | null> {
    const object = await this.bucket.get(`sites/${siteId}/media/${filename}`);
    if (object === null) return null;
    if (object.customMetadata?.["simple-blog-kind"] !== "media") {
      throw new Error("media_metadata_invalid");
    }
    const contentType = object.httpMetadata?.contentType;
    if (contentType === undefined || !/^image\/(?:gif|jpeg|png|webp)$/.test(contentType)) {
      throw new Error("media_content_type_invalid");
    }
    return { bytes: new Uint8Array(await object.arrayBuffer()), contentType };
  }
}

export class DurableObjectEngagement implements EngagementWriter {
  private readonly sites: DurableObjectNamespace;
  private readonly internalToken: string;

  constructor(
    sites: DurableObjectNamespace,
    internalToken: string,
  ) {
    this.sites = sites;
    this.internalToken = internalToken;
  }

  async recordView(siteId: string, contentId: number): Promise<void> {
    const response = await this.stub(siteId).fetch(this.request(`/views/${contentId}`, undefined));
    if (!response.ok) throw new Error(`view_write_failed:${response.status}`);
  }

  async toggleLike(
    siteId: string,
    contentId: number,
    operation: "like" | "unlike",
  ): Promise<void> {
    const response = await this.stub(siteId).fetch(this.request(`/likes/${contentId}`, { operation }));
    if (!response.ok) throw new Error(`like_write_failed:${response.status}`);
  }

  private stub(siteId: string) {
    return this.sites.get(this.sites.idFromName(siteId));
  }

  private request(path: string, body: unknown): Request {
    return new Request(`https://site.internal${path}`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${this.internalToken}`,
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  }
}

function verifyMetadata(object: R2ObjectBody, kind: string, id: string): void {
  if (
    object.customMetadata?.["simple-blog-kind"] !== kind ||
    object.customMetadata?.["blake3"] !== id ||
    !DIGEST.test(object.customMetadata?.["sha256"] ?? "")
  ) {
    throw new Error(`release_${kind}_metadata_invalid`);
  }
}

function exactKeys(value: Record<string, unknown>, expected: string[]): boolean {
  return JSON.stringify(Object.keys(value).sort()) === JSON.stringify(expected);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
