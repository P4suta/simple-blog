import type { DurableObjectNamespace, R2Bucket } from "./bindings.ts";
import { normalizeDomain } from "./domain.ts";
import { authorizedBearer } from "./internal-auth.ts";
import type { ReleaseCandidate } from "./publication.ts";
import { boundedBytes } from "./request-body.ts";
import { R2ReleaseStager, type StageManifestInput, type StageObjectInput } from "./staging.ts";

export interface InternalActivationResult {
  changed: boolean;
}

export interface InternalPublication {
  stageObject(input: StageObjectInput): Promise<{ created: boolean }>;
  stageManifest(input: StageManifestInput): Promise<{ created: boolean }>;
  activate(input: ReleaseCandidate): Promise<InternalActivationResult>;
}

const SITE_ID_PART = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
const DIGEST_PART = "[0-9a-f]{64}";
const OBJECT_ROUTE = new RegExp(`^/internal/sites/(${SITE_ID_PART})/release-objects/(${DIGEST_PART})$`);
const MANIFEST_ROUTE = new RegExp(`^/internal/sites/(${SITE_ID_PART})/release-manifests/(${DIGEST_PART})$`);
const ACTIVATE_ROUTE = new RegExp(`^/internal/sites/(${SITE_ID_PART})/releases/(${DIGEST_PART})/activate$`);
const DIGEST = /^[0-9a-f]{64}$/;
const MAX_OBJECT_BYTES = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES = 16 * 1024 * 1024;
const MAX_ACTIVATION_BYTES = 4096;

export class CloudflareInternalPublication implements InternalPublication {
  private readonly stager: R2ReleaseStager;
  private readonly sites: DurableObjectNamespace;
  private readonly internalToken: string;

  constructor(bucket: R2Bucket, sites: DurableObjectNamespace, internalToken: string) {
    this.stager = new R2ReleaseStager(bucket);
    this.sites = sites;
    this.internalToken = internalToken;
  }

  stageObject(input: StageObjectInput) {
    return this.stager.stageObject(input);
  }

  stageManifest(input: StageManifestInput) {
    return this.stager.stageManifest(input);
  }

  async activate(input: ReleaseCandidate): Promise<InternalActivationResult> {
    const stub = this.sites.get(this.sites.idFromName(input.siteId));
    const response = await stub.fetch(new Request("https://site.internal/publication/activate", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${this.internalToken}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        site_id: input.siteId,
        domain: input.domain,
        expected_release: input.expectedRelease,
        replacement_release: input.replacementRelease,
        next_publish_at: input.nextPublishAt,
      }),
    }));
    if (response.status === 409) throw new Error("release_activation_conflict");
    if (!response.ok) throw new Error(`release_activation_failed_${response.status}`);
    const value = await response.json() as unknown;
    if (!isRecord(value) || JSON.stringify(Object.keys(value)) !== JSON.stringify(["changed"]) ||
        typeof value.changed !== "boolean") {
      throw new Error("release_activation_response_invalid");
    }
    return { changed: value.changed };
  }
}

export async function handleInternalPublicationRequest(
  request: Request,
  publication: InternalPublication,
  internalToken: string,
): Promise<Response> {
  if (!(await authorizedBearer(request, internalToken))) {
    const unauthorized = json({ error: "publication_capability_required" }, 401);
    unauthorized.headers.set("WWW-Authenticate", "Bearer");
    return unauthorized;
  }
  const url = new URL(request.url);
  if (url.search !== "") return json({ error: "publication_route_invalid" }, 404);
  const object = OBJECT_ROUTE.exec(url.pathname);
  const manifest = MANIFEST_ROUTE.exec(url.pathname);
  const activate = ACTIVATE_ROUTE.exec(url.pathname);
  if (object === null && manifest === null && activate === null) {
    return json({ error: "publication_route_not_found" }, 404);
  }
  if (object !== null || manifest !== null) {
    if (request.method !== "PUT") return methodNotAllowed("PUT");
  } else if (request.method !== "POST") {
    return methodNotAllowed("POST");
  }

  try {
    if (object !== null) {
      const result = await publication.stageObject({
        siteId: object[1]!,
        objectId: object[2]!,
        sha256: checksumHeader(request),
        bytes: await publicationBytes(request, MAX_OBJECT_BYTES),
      });
      console.log(JSON.stringify({
        event: "release.object.staged",
        site_id: object[1],
        object_id: object[2],
        created: result.created,
      }));
      return json(result, result.created ? 201 : 200);
    }
    if (manifest !== null) {
      if (mediaType(request) !== "application/json") throw new Error("release_manifest_invalid");
      const domain = request.headers.get("x-simple-blog-domain");
      if (domain === null || normalizeDomain(domain) !== domain) {
        throw new Error("release_domain_invalid");
      }
      const result = await publication.stageManifest({
        siteId: manifest[1]!,
        releaseId: manifest[2]!,
        domain,
        sha256: checksumHeader(request),
        bytes: await publicationBytes(request, MAX_MANIFEST_BYTES),
      });
      console.log(JSON.stringify({
        event: "release.manifest.staged",
        site_id: manifest[1],
        release_id: manifest[2],
        created: result.created,
      }));
      return json(result, result.created ? 201 : 200);
    }
    const candidate = await activationCandidate(request, activate![1]!, activate![2]!);
    const result = await publication.activate(candidate);
    console.log(JSON.stringify({
      event: "release.activated",
      site_id: candidate.siteId,
      release_id: candidate.replacementRelease,
      changed: result.changed,
    }));
    return json(result, 200);
  } catch (error) {
    const code = error instanceof Error ? error.message : "";
    if (code === "publication_request_too_large") return json({ error: code }, 413);
    if (code.endsWith("_collision") || code === "release_activation_conflict") {
      return json({ error: code }, 409);
    }
    if (
      code.startsWith("release_activation_failed_") ||
      code === "release_activation_response_invalid" ||
      code === "release_manifest_missing" ||
      code === "release_manifest_integrity_invalid" ||
      code === "release_head_unavailable" ||
      code.startsWith("release_object_missing:") ||
      code === "release_object_integrity_invalid"
    ) {
      return json({ error: code }, 502);
    }
    if (code.startsWith("release_") || code === "publication_request_invalid") {
      return json({ error: code }, 422);
    }
    throw error;
  }
}

async function activationCandidate(
  request: Request,
  siteId: string,
  releaseId: string,
): Promise<ReleaseCandidate> {
  if (mediaType(request) !== "application/json") throw new Error("publication_request_invalid");
  const bytes = await publicationBytes(request, MAX_ACTIVATION_BYTES);
  let value: unknown;
  try {
    value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as unknown;
  } catch {
    throw new Error("publication_request_invalid");
  }
  if (!isRecord(value) || JSON.stringify(Object.keys(value).sort()) !== JSON.stringify([
    "domain",
    "expected_release",
    "next_publish_at",
  ]) || typeof value.domain !== "string" || normalizeDomain(value.domain) !== value.domain ||
      (value.expected_release !== null &&
        (typeof value.expected_release !== "string" || !DIGEST.test(value.expected_release))) ||
      (value.next_publish_at !== null &&
        (typeof value.next_publish_at !== "string" ||
          !Number.isFinite(Date.parse(value.next_publish_at))))) {
    throw new Error("publication_request_invalid");
  }
  return {
    siteId,
    domain: value.domain,
    expectedRelease: value.expected_release,
    replacementRelease: releaseId,
    nextPublishAt: value.next_publish_at,
  };
}

function checksumHeader(request: Request): string {
  const value = request.headers.get("x-simple-blog-sha256");
  if (value === null || !DIGEST.test(value)) throw new Error("release_sha256_invalid");
  return value;
}

function publicationBytes(request: Request, maximum: number): Promise<Uint8Array> {
  return boundedBytes(
    request,
    maximum,
    "publication_request_invalid",
    "publication_request_too_large",
  );
}

function mediaType(request: Request): string | null {
  return request.headers.get("content-type")?.split(";", 1)[0]?.trim() ?? null;
}

function methodNotAllowed(method: string): Response {
  const response = json({ error: "publication_method_not_allowed" }, 405);
  response.headers.set("Allow", method);
  return response;
}

function json(value: unknown, status: number): Response {
  return Response.json(value, {
    status,
    headers: { "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" },
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
