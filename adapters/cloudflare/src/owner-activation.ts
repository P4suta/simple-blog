import type { D1Database, D1Result } from "./bindings.ts";
import { normalizeDomain } from "./domain.ts";
import { authorizedBearer } from "./internal-auth.ts";

export interface OwnerActivationResult {
  changed: boolean;
}

export interface OwnerActivationRepository {
  activateOwner(
    registrationId: string,
    domain: string,
    registeredAt: string,
  ): Promise<OwnerActivationResult>;
}

interface RegistrationRow {
  id: unknown;
  domain: unknown;
  state: unknown;
  owner_registered_at: unknown;
  row_version: unknown;
}

const REGISTRATION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const OWNER_ROUTE = /^\/internal\/registrations\/([0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})\/owner$/;

export class D1OwnerActivationRepository implements OwnerActivationRepository {
  private readonly database: D1Database;

  constructor(database: D1Database) {
    this.database = database;
  }

  async activateOwner(
    registrationId: string,
    domain: string,
    registeredAt: string,
  ): Promise<OwnerActivationResult> {
    validateInput(registrationId, domain, registeredAt);
    const row = await this.registration(registrationId);
    const canonical = normalizeDomain(domain);
    if (row === null || row.domain !== canonical) throw new Error("owner_activation_conflict");
    if (row.ownerRegisteredAt !== null) {
      await this.repairHostedSite(row, registeredAt);
      return { changed: false };
    }
    if (row.state !== "ready_for_owner") throw new Error("owner_activation_conflict");

    const results = await this.database.batch([
      this.database.prepare(
        `UPDATE domain_registrations
         SET owner_registered_at = ?, state = 'active', claim_expires_at = ?,
             updated_at = ?, row_version = row_version + 1
         WHERE id = ? AND row_version = ? AND owner_registered_at IS NULL
           AND state = 'ready_for_owner'`,
      ).bind(registeredAt, registeredAt, registeredAt, registrationId, row.rowVersion),
      this.database.prepare(
        `INSERT INTO hosting_audit_events (
           registration_id, event, diagnostic_code, occurred_at
         )
         SELECT id, 'owner.activated', 'HOST_OWNER_ACTIVATED', ?
         FROM domain_registrations
         WHERE id = ? AND owner_registered_at = ? AND changes() = 1`,
      ).bind(registeredAt, registrationId, registeredAt),
      this.hostedSiteStatement(row, registeredAt),
    ]);
    verifyBatch(results, 3);
    if (changes(results[0]!) === 1) return { changed: true };

    const concurrent = await this.registration(registrationId);
    if (concurrent !== null && concurrent.ownerRegisteredAt !== null &&
        concurrent.domain === canonical) {
      await this.repairHostedSite(concurrent, registeredAt);
      return { changed: false };
    }
    throw new Error("owner_activation_conflict");
  }

  private async registration(registrationId: string) {
    const row = await this.database.prepare(
      `SELECT id, domain, state, owner_registered_at, row_version
       FROM domain_registrations WHERE id = ?`,
    ).bind(registrationId).first<RegistrationRow>();
    if (row === null) return null;
    if (row.id !== registrationId || typeof row.domain !== "string" ||
        typeof row.state !== "string" ||
        (row.owner_registered_at !== null && typeof row.owner_registered_at !== "string") ||
        typeof row.row_version !== "number" || !Number.isSafeInteger(row.row_version) ||
        row.row_version < 0) {
      throw new Error("owner_activation_record_invalid");
    }
    return {
      id: registrationId,
      domain: normalizeDomain(row.domain),
      state: row.state,
      ownerRegisteredAt: row.owner_registered_at,
      rowVersion: row.row_version,
    };
  }

  private hostedSiteStatement(
    row: { id: string; domain: string; ownerRegisteredAt: string | null },
    registeredAt: string,
  ) {
    const activationTime = row.ownerRegisteredAt ?? registeredAt;
    return this.database.prepare(
      `INSERT INTO hosted_sites (
         site_id, registration_id, domain, active_release, created_at, updated_at
       )
       SELECT id, id, domain, NULL, ?, ?
       FROM domain_registrations
       WHERE id = ? AND owner_registered_at IS NOT NULL
       ON CONFLICT(registration_id) DO NOTHING`,
    ).bind(activationTime, activationTime, row.id);
  }

  private async repairHostedSite(
    row: { id: string; domain: string; ownerRegisteredAt: string | null },
    registeredAt: string,
  ): Promise<void> {
    const result = await this.hostedSiteStatement(row, registeredAt).run();
    if (!result.success) throw new Error("owner_activation_write_failed");
  }
}

export async function handleOwnerActivationRequest(
  request: Request,
  repository: OwnerActivationRepository,
  internalToken: string,
): Promise<Response> {
  const match = OWNER_ROUTE.exec(new URL(request.url).pathname);
  if (match === null) return json({ error: "owner_activation_route_not_found" }, 404);
  if (!(await authorizedBearer(request, internalToken))) {
    const response = json({ error: "owner_activation_capability_required" }, 401);
    response.headers.set("WWW-Authenticate", "Bearer");
    return response;
  }
  if (request.method !== "POST") {
    const response = json({ error: "owner_activation_method_not_allowed" }, 405);
    response.headers.set("Allow", "POST");
    return response;
  }
  let body: Record<string, unknown>;
  try {
    body = await strictJson(request);
    if (typeof body.domain !== "string" || typeof body.registered_at !== "string") {
      throw new Error("owner_activation_invalid");
    }
    validateInput(match[1]!, body.domain, body.registered_at);
  } catch {
    return json({ error: "owner_activation_invalid" }, 422);
  }
  try {
    return json(await repository.activateOwner(match[1]!, body.domain, body.registered_at), 200);
  } catch (error) {
    if (error instanceof Error && error.message === "owner_activation_conflict") {
      return json({ error: error.message }, 409);
    }
    throw error;
  }
}

function validateInput(registrationId: string, domain: string, registeredAt: string): void {
  if (!REGISTRATION_ID.test(registrationId) || normalizeDomain(domain) !== domain ||
      new Date(registeredAt).toISOString() !== registeredAt) {
    throw new Error("owner_activation_invalid");
  }
}

async function strictJson(request: Request): Promise<Record<string, unknown>> {
  if (request.headers.get("content-type")?.split(";", 1)[0]?.trim() !== "application/json") {
    throw new Error("owner_activation_invalid");
  }
  const text = await request.text();
  if (new TextEncoder().encode(text).byteLength > 2048) throw new Error("owner_activation_invalid");
  const value = JSON.parse(text) as unknown;
  if (!isRecord(value) || JSON.stringify(Object.keys(value).sort()) !==
      JSON.stringify(["domain", "registered_at"])) {
    throw new Error("owner_activation_invalid");
  }
  return value;
}

function verifyBatch(results: D1Result[], expected: number): void {
  if (results.length !== expected || results.some((result) => !result.success)) {
    throw new Error("owner_activation_write_failed");
  }
}

function changes(result: D1Result): number {
  const value = result.meta?.["changes"];
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new Error("owner_activation_write_unconfirmed");
  }
  return value;
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
