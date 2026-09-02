import { normalizeDomain, type DomainRegistrationState } from "./domain.ts";
import {
  parseManifest,
  resolveRelease,
  responseFromResolved,
  type ReleaseReader,
} from "./release.ts";
import { boundedBytes } from "./request-body.ts";

export interface HostedSite {
  siteId: string;
  activeRelease: string;
  state: DomainRegistrationState;
}

export interface HostDirectory {
  lookup(hostname: string): Promise<HostedSite | null>;
}

export interface ReleaseDirectory {
  forSite(site: HostedSite): ReleaseReader;
}

export interface MediaObject {
  bytes: Uint8Array;
  contentType: string;
}

export interface MediaDirectory {
  get(siteId: string, filename: string): Promise<MediaObject | null>;
}

export interface EngagementWriter {
  recordView(siteId: string, contentId: number): Promise<void>;
  toggleLike(siteId: string, contentId: number, operation: "like" | "unlike"): Promise<void>;
}

export interface Diagnostics {
  failure(event: string, error: unknown): void;
}

export interface PublicDependencies {
  directory: HostDirectory;
  releases: ReleaseDirectory;
  media: MediaDirectory;
  engagement: EngagementWriter;
  diagnostics?: Diagnostics;
}

export interface WaitUntil {
  waitUntil(promise: Promise<unknown>): void;
}

export async function handlePublicRequest(
  request: Request,
  dependencies: PublicDependencies,
  context: WaitUntil,
): Promise<Response> {
  const site = await resolveHostedSite(request, dependencies.directory);
  if (site === null || site.state !== "active") return notFound();
  const url = new URL(request.url);
  if (url.pathname.startsWith("/media/")) {
    return serveMedia(request, site, dependencies.media);
  }
  const likeId = likeContentId(url.pathname);
  if (likeId !== null) {
    return toggleLike(request, site, likeId, dependencies);
  }
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response(null, { status: 405, headers: { Allow: "GET, HEAD" } });
  }
  let route;
  try {
    route = await resolveRelease(dependencies.releases.forSite(site), url.pathname);
  } catch (error) {
    if (error instanceof Error && error.message === "release_active_missing") {
      dependencies.diagnostics?.failure("release.not_found", error);
      return new Response("Service Unavailable", {
        status: 503,
        headers: { "Retry-After": "5", "Cache-Control": "no-store" },
      });
    }
    throw error;
  }
  if (
    request.method === "GET" &&
    route.kind === "asset" &&
    route.content_id !== undefined &&
    !probablyBot(request.headers.get("user-agent"))
  ) {
    const view = dependencies.engagement
      .recordView(site.siteId, route.content_id)
      .catch((error: unknown) => dependencies.diagnostics?.failure("views.record_failed", error));
    context.waitUntil(view);
  }
  return responseFromResolved(request, route);
}

async function resolveHostedSite(
  request: Request,
  directory: HostDirectory,
): Promise<HostedSite | null> {
  try {
    return await directory.lookup(normalizeDomain(new URL(request.url).hostname));
  } catch {
    return null;
  }
}

async function serveMedia(
  request: Request,
  site: HostedSite,
  media: MediaDirectory,
): Promise<Response> {
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response(null, { status: 405, headers: { Allow: "GET, HEAD" } });
  }
  const filename = decodeURIComponent(new URL(request.url).pathname.slice("/media/".length));
  if (filename.length === 0 || filename.length > 200 || !/^[a-z0-9.-]+$/.test(filename)) {
    return notFound();
  }
  const etag = `"media-${filename}"`;
  if (request.headers.get("if-none-match") === etag) {
    return new Response(null, { status: 304, headers: { ETag: etag } });
  }
  const object = await media.get(site.siteId, filename);
  if (object === null) return notFound();
  const body = new Uint8Array(object.bytes.byteLength);
  body.set(object.bytes);
  return new Response(request.method === "HEAD" ? null : body.buffer, {
    headers: {
      "Content-Type": object.contentType,
      "Cache-Control": "public, max-age=31536000, immutable",
      ETag: etag,
      "X-Content-Type-Options": "nosniff",
    },
  });
}

async function toggleLike(
  request: Request,
  site: HostedSite,
  contentId: number,
  dependencies: PublicDependencies,
): Promise<Response> {
  if (request.method !== "POST") {
    return new Response(null, { status: 405, headers: { Allow: "POST" } });
  }
  if (request.headers.get("content-type")?.split(";", 1)[0]?.trim() !== "application/json") {
    return new Response("application/json required", { status: 415 });
  }
  let operation: unknown;
  try {
    const bytes = await boundedBytes(request, 1024, "invalid_json", "request_too_large");
    operation = (JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    ) as { op?: unknown }).op;
  } catch (error) {
    if (error instanceof Error && error.message === "request_too_large") {
      return new Response("request too large", { status: 413 });
    }
    return new Response("invalid JSON", { status: 400 });
  }
  if (operation !== "like" && operation !== "unlike") {
    return new Response("op must be like or unlike", { status: 422 });
  }
  if (!(await contentIsPublic(dependencies.releases.forSite(site), contentId))) return notFound();
  await dependencies.engagement.toggleLike(site.siteId, contentId, operation);
  return new Response(null, { status: 204 });
}

async function contentIsPublic(reader: ReleaseReader, contentId: number): Promise<boolean> {
  const active = await reader.activeRelease();
  if (active === null) return false;
  const manifest = parseManifest(await reader.manifest(active));
  return Object.values(manifest.routes).some(
    (route) => route.kind === "asset" && route.status === 200 && route.content_id === contentId,
  );
}

function likeContentId(path: string): number | null {
  const match = /^\/likes\/([1-9][0-9]*)$/.exec(path);
  if (match === null) return null;
  const id = Number(match[1]);
  return Number.isSafeInteger(id) ? id : null;
}

function probablyBot(userAgent: string | null): boolean {
  return userAgent !== null && /bot|crawler|spider|preview|curl|wget/i.test(userAgent);
}

function notFound(): Response {
  return new Response("not found\n", {
    status: 404,
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
}
