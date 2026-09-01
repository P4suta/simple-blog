import type { D1Database, Fetcher } from "./bindings.ts";
import { normalizeDomain } from "./domain.ts";
import type { HostDirectory } from "./public.ts";

export interface AdminSiteIdentity {
  siteId: string;
  state: "ready_for_owner" | "active" | "action_required";
}

export interface AdminRegistrationDirectory {
  lookup(domain: string): Promise<AdminSiteIdentity | null>;
}

const SITE_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const ADMIN_METHODS = new Set(["GET", "HEAD", "POST"]);

export class D1AdminRegistrationDirectory implements AdminRegistrationDirectory {
  private readonly database: D1Database;

  constructor(database: D1Database) {
    this.database = database;
  }

  async lookup(domain: string): Promise<AdminSiteIdentity | null> {
    const canonical = normalizeDomain(domain);
    const row = await this.database.prepare(
      `SELECT site_id, state FROM (
         SELECT hosted_sites.site_id, domain_registrations.state, 0 AS priority
         FROM hosted_sites
         JOIN domain_registrations
           ON domain_registrations.id = hosted_sites.registration_id
         WHERE hosted_sites.domain = ? COLLATE NOCASE
         UNION ALL
         SELECT id AS site_id, state, 1 AS priority
         FROM domain_registrations
         WHERE domain = ? COLLATE NOCASE
           AND state = 'ready_for_owner'
           AND owner_registered_at IS NULL
       ) ORDER BY priority LIMIT 1`,
    ).bind(canonical, canonical).first<{ site_id: unknown; state: unknown }>();
    if (row === null) return null;
    if (typeof row.site_id !== "string" || !SITE_ID.test(row.site_id) ||
        (row.state !== "ready_for_owner" && row.state !== "active" &&
          row.state !== "action_required")) {
      throw new Error("admin_registration_invalid");
    }
    return { siteId: row.site_id, state: row.state };
  }
}

/** Routes only the dynamic CMS surface; public paths remain immutable releases. */
export class CoreAdminGateway {
  private readonly hosts: HostDirectory;
  private readonly registrations: AdminRegistrationDirectory;
  private readonly core: Fetcher;
  private readonly internalToken: string;

  constructor(
    hosts: HostDirectory,
    registrations: AdminRegistrationDirectory,
    core: Fetcher,
    internalToken: string,
  ) {
    this.hosts = hosts;
    this.registrations = registrations;
    this.core = core;
    this.internalToken = internalToken;
  }

  async handle(request: Request): Promise<Response | null> {
    const url = new URL(request.url);
    if (url.pathname !== "/admin" && !url.pathname.startsWith("/admin/")) return null;
    if (!ADMIN_METHODS.has(request.method)) {
      return new Response(null, { status: 405, headers: { Allow: "GET, HEAD, POST" } });
    }
    const domain = normalizeDomain(url.hostname);
    const hosted = await this.hosts.lookup(domain);
    const site = hosted !== null && (hosted.state === "active" || hosted.state === "action_required")
      ? { siteId: hosted.siteId }
      : await this.registrations.lookup(domain);
    if (site === null) return new Response("not found\n", { status: 404 });

    const target = new URL(
      `/internal/sites/${site.siteId}/http${url.pathname}`,
      "https://core.internal",
    );
    target.search = url.search;
    const upstream = new Request(target, request);
    for (const header of [
      "authorization",
      "cf-connecting-ip",
      "cf-ipcountry",
      "cf-ray",
      "forwarded",
      "x-forwarded-for",
      "x-forwarded-host",
      "x-forwarded-proto",
      "x-real-ip",
      "x-simple-blog-canonical-origin",
      "x-simple-blog-site-id",
    ]) {
      upstream.headers.delete(header);
    }
    upstream.headers.set("Authorization", `Bearer ${this.internalToken}`);
    upstream.headers.set("X-Simple-Blog-Site-Id", site.siteId);
    upstream.headers.set("X-Simple-Blog-Canonical-Origin", `https://${domain}`);
    const response = await this.core.fetch(upstream);
    const wrapped = new Response(response.body, response);
    wrapped.headers.set("Cache-Control", "no-store");
    return wrapped;
  }
}
