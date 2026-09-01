export interface AssetRoute {
  kind: "asset";
  object_id: string;
  content_type: string;
  cache_control: string;
  status: 200 | 404 | 410;
  last_modified?: string;
  content_id?: number;
}

export interface RedirectRoute {
  kind: "redirect";
  status: 301 | 302 | 307 | 308;
  location: string;
}

export type ReleaseRoute = AssetRoute | RedirectRoute;

export interface ReleaseManifest {
  format_version: 1;
  compiler_version: string;
  public_revision: number;
  canonical_origin: string;
  routes: Record<string, ReleaseRoute>;
}

export interface ReleaseReader {
  activeRelease(): Promise<string | null>;
  manifest(releaseId: string): Promise<unknown | null>;
  object(objectId: string): Promise<Uint8Array | null>;
}

export interface ResolvedAsset extends AssetRoute {
  release_id: string;
  body: Uint8Array;
  fallback: boolean;
}

export interface ResolvedRedirect extends RedirectRoute {
  release_id: string;
}

export type ResolvedRoute = ResolvedAsset | ResolvedRedirect;

const DIGEST = /^[0-9a-f]{64}$/;
const VALID_ASSET_STATUS = new Set([200, 404, 410]);
const VALID_REDIRECT_STATUS = new Set([301, 302, 307, 308]);

export async function resolveRelease(
  reader: ReleaseReader,
  path: string,
): Promise<ResolvedRoute> {
  validatePath(path);
  const releaseId = await reader.activeRelease();
  if (releaseId === null || !DIGEST.test(releaseId)) throw new Error("release_active_missing");
  const manifest = parseManifest(await reader.manifest(releaseId));
  const selected = manifest.routes[path];
  const route = selected ?? manifest.routes["/404/"];
  if (route === undefined) throw new Error("release_route_missing");
  const fallback = selected === undefined;
  if (route.kind === "redirect") return { ...route, release_id: releaseId };
  const body = await reader.object(route.object_id);
  if (body === null) throw new Error(`release_object_missing:${route.object_id}`);
  return { ...route, release_id: releaseId, body, fallback };
}

export async function serveRelease(
  request: Request,
  reader: ReleaseReader,
): Promise<Response> {
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response(null, { status: 405, headers: { Allow: "GET, HEAD" } });
  }
  const route = await resolveRelease(reader, new URL(request.url).pathname);
  return responseFromResolved(request, route);
}

export function responseFromResolved(request: Request, route: ResolvedRoute): Response {
  if (route.kind === "redirect") {
    return new Response(null, {
      status: route.status,
      headers: {
        Location: route.location,
        "Cache-Control": "public, max-age=0, must-revalidate",
        "x-simple-blog-release": route.release_id,
      },
    });
  }
  const etag = `"blake3-${route.object_id}"`;
  const headers = releaseHeaders(route, etag);
  if (route.status === 200 && requestIsFresh(request.headers, etag, route.last_modified)) {
    headers.delete("Content-Type");
    return new Response(null, { status: 304, headers });
  }
  return new Response(request.method === "HEAD" ? null : copiedBuffer(route.body), {
    status: route.status,
    headers,
  });
}

function copiedBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer;
}

export function parseManifest(value: unknown): ReleaseManifest {
  if (!isRecord(value)) throw new Error("release_manifest_invalid");
  const keys = Object.keys(value).sort();
  const expected = [
    "canonical_origin",
    "compiler_version",
    "format_version",
    "public_revision",
    "routes",
  ];
  if (JSON.stringify(keys) !== JSON.stringify(expected)) throw new Error("release_manifest_fields");
  if (
    value.format_version !== 1 ||
    typeof value.compiler_version !== "string" ||
    value.compiler_version.trim() === "" ||
    !Number.isSafeInteger(value.public_revision) ||
    typeof value.canonical_origin !== "string" ||
    !isRecord(value.routes)
  ) {
    throw new Error("release_manifest_invalid");
  }
  const origin = new URL(value.canonical_origin);
  if (
    origin.protocol !== "https:" ||
    origin.username !== "" ||
    origin.password !== "" ||
    origin.pathname !== "/" ||
    origin.search !== "" ||
    origin.hash !== "" ||
    value.canonical_origin.endsWith("/")
  ) {
    throw new Error("release_origin_invalid");
  }
  const routes: Record<string, ReleaseRoute> = {};
  for (const [path, route] of Object.entries(value.routes)) {
    validatePath(path);
    routes[path] = parseRoute(route);
  }
  return { ...value, routes } as ReleaseManifest;
}

function parseRoute(value: unknown): ReleaseRoute {
  if (!isRecord(value)) throw new Error("release_route_invalid");
  if (value.kind === "asset") {
    const optional = new Set(["last_modified", "content_id"]);
    exactFields(
      value,
      new Set(["kind", "object_id", "content_type", "cache_control", "status"]),
      optional,
    );
    if (
      typeof value.object_id !== "string" ||
      !DIGEST.test(value.object_id) ||
      typeof value.content_type !== "string" ||
      !safeHeader(value.content_type) ||
      typeof value.cache_control !== "string" ||
      !safeHeader(value.cache_control) ||
      typeof value.status !== "number" ||
      !VALID_ASSET_STATUS.has(value.status) ||
      (value.last_modified !== undefined && !validTimestamp(value.last_modified)) ||
      (value.content_id !== undefined && !Number.isSafeInteger(value.content_id))
    ) {
      throw new Error("release_asset_invalid");
    }
    return value as unknown as AssetRoute;
  }
  if (value.kind === "redirect") {
    exactFields(value, new Set(["kind", "status", "location"]), new Set());
    if (
      typeof value.status !== "number" ||
      !VALID_REDIRECT_STATUS.has(value.status) ||
      typeof value.location !== "string"
    ) {
      throw new Error("release_redirect_invalid");
    }
    validatePath(value.location);
    return value as unknown as RedirectRoute;
  }
  throw new Error("release_route_invalid");
}

function releaseHeaders(route: ResolvedAsset, etag: string): Headers {
  const headers = new Headers({
    "Content-Type": route.content_type,
    "Cache-Control": route.cache_control,
    ETag: etag,
    "x-simple-blog-release": route.release_id,
  });
  if (route.last_modified !== undefined) {
    headers.set("Last-Modified", new Date(route.last_modified).toUTCString());
  }
  return headers;
}

function requestIsFresh(headers: Headers, etag: string, lastModified?: string): boolean {
  const ifNoneMatch = headers.get("If-None-Match");
  if (ifNoneMatch !== null) {
    return ifNoneMatch.split(",").some((candidate) => {
      const value = candidate.trim();
      return value === "*" || value.replace(/^W\//, "") === etag;
    });
  }
  const since = headers.get("If-Modified-Since");
  if (since === null || lastModified === undefined) return false;
  const sinceTime = Date.parse(since);
  const modifiedTime = Date.parse(lastModified);
  return Number.isFinite(sinceTime) && Number.isFinite(modifiedTime) && modifiedTime <= sinceTime;
}

function validatePath(path: string): void {
  if (
    !path.startsWith("/") ||
    path.startsWith("//") ||
    /[?#\\\0\r\n]/.test(path) ||
    path.split("/").some((segment) => segment === "." || segment === "..")
  ) {
    throw new Error("release_path_invalid");
  }
}

function safeHeader(value: string): boolean {
  return value.length > 0 && value.length <= 256 && !/[\0\r\n]/.test(value);
}

function validTimestamp(value: unknown): value is string {
  return typeof value === "string" && Number.isFinite(Date.parse(value));
}

function exactFields(
  value: Record<string, unknown>,
  required: Set<string>,
  optional: Set<string>,
): void {
  const keys = new Set(Object.keys(value));
  if (
    [...required].some((key) => !keys.has(key)) ||
    [...keys].some((key) => !required.has(key) && !optional.has(key))
  ) {
    throw new Error("release_route_fields");
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
