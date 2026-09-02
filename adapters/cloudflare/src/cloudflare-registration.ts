import type { D1Database, D1Result } from "./bindings.ts";
import {
  normalizeDomain,
  type DomainRegistrationState,
  type ProvisioningStatus,
} from "./domain.ts";
import type {
  ClaimSecret,
  ClaimSecrets,
  CustomHostname,
  CustomHostnameProvider,
  DnsInstruction,
  DomainRouteVerifier,
  RegistrationRecord,
  RegistrationRepository,
} from "./registration.ts";

export class D1RegistrationRepository implements RegistrationRepository {
  private readonly database: D1Database;

  constructor(database: D1Database) {
    this.database = database;
  }

  async reserve(record: RegistrationRecord): Promise<boolean> {
    const results = await this.database.batch([
      this.database.prepare(
      `INSERT INTO domain_registrations (
         id, domain, claim_hash, claim_expires_at, state,
         certificate_validation_json, created_at, updated_at
       ) VALUES (?, ?, ?, ?, ?, '[]', ?, ?)
       ON CONFLICT(domain) DO NOTHING`,
      ).bind(
        record.id,
        record.domain,
        record.claimHash,
        record.claimExpiresAt,
        record.state,
        record.createdAt,
        record.updatedAt,
      ),
      this.database.prepare(
        `INSERT INTO hosting_audit_events (
           registration_id, event, diagnostic_code, occurred_at
         )
         SELECT id, 'domain.reserved', 'HOST_DOMAIN_RESERVED', ?
         FROM domain_registrations
         WHERE id = ? AND claim_hash = ? AND changes() = 1`,
      ).bind(record.createdAt, record.id, record.claimHash),
    ]);
    verifyBatch(results, 2, "registration_write_failed");
    return confirmedChanges(results[0]!, "registration_write_unconfirmed") === 1;
  }

  async authorized(id: string, claimHash: string, now: string): Promise<RegistrationRecord | null> {
    const row = await this.database.prepare(
      `SELECT * FROM domain_registrations
       WHERE id = ? AND claim_hash = ? AND claim_expires_at > ?
         AND owner_registered_at IS NULL`,
    ).bind(id, claimHash, now).first<RegistrationRow>();
    return row === null ? null : registrationFromRow(row);
  }

  async save(record: RegistrationRecord): Promise<void> {
    const results = await this.database.batch([
      this.database.prepare(
        `UPDATE domain_registrations SET
           provider_hostname_id = ?, state = ?, ownership_verification_json = ?,
           certificate_validation_json = ?, provider_error_code = ?, last_observed_at = ?,
           owner_registered_at = ?, updated_at = ?, row_version = row_version + 1
         WHERE id = ? AND row_version = ?`,
      ).bind(
        record.providerHostnameId,
        record.state,
        encodeOptional(record.ownershipVerification),
        JSON.stringify(record.certificateValidation),
        record.providerErrorCode,
        record.lastObservedAt,
        record.ownerRegisteredAt,
        record.updatedAt,
        record.id,
        record.storageVersion,
      ),
      this.database.prepare(
        `INSERT INTO hosting_audit_events (
           registration_id, event, diagnostic_code, occurred_at
         )
         SELECT id, 'domain.observed', ?, ?
         FROM domain_registrations
         WHERE id = ? AND row_version = ? AND changes() = 1`,
      ).bind(
        `HOST_DOMAIN_${record.state.toUpperCase()}`,
        record.updatedAt,
        record.id,
        record.storageVersion + 1,
      ),
    ]);
    verifyBatch(results, 2, "registration_write_failed");
    const changes = confirmedChanges(results[0]!, "registration_write_unconfirmed");
    if (changes !== 1) {
      throw new Error(changes === 0
        ? "registration_stale_observation"
        : "registration_write_unconfirmed");
    }
    record.storageVersion += 1;
  }
}

function verifyBatch(results: D1Result[], expected: number, code: string): void {
  if (results.length !== expected || results.some((result) => !result.success)) {
    throw new Error(code);
  }
}

function confirmedChanges(result: D1Result, code: string): number {
  const changes = result.meta?.["changes"];
  if (typeof changes !== "number" || !Number.isSafeInteger(changes) || changes < 0) {
    throw new Error(code);
  }
  return changes;
}

interface RegistrationRow {
  id: string;
  domain: string;
  claim_hash: string;
  claim_expires_at: string;
  provider_hostname_id: string | null;
  state: string;
  ownership_verification_json: string | null;
  certificate_validation_json: string;
  provider_error_code: string | null;
  last_observed_at: string | null;
  owner_registered_at: string | null;
  created_at: string;
  updated_at: string;
  row_version: number;
}

function registrationFromRow(row: RegistrationRow): RegistrationRecord {
  const state = registrationState(row.state);
  if (!Number.isSafeInteger(row.row_version) || row.row_version < 0) {
    throw new Error("registration_version_invalid");
  }
  return {
    id: row.id,
    domain: normalizeDomain(row.domain),
    claimHash: row.claim_hash,
    claimExpiresAt: validTimestamp(row.claim_expires_at),
    providerHostnameId: row.provider_hostname_id,
    state,
    ownershipVerification: decodeInstruction(row.ownership_verification_json),
    certificateValidation: decodeInstructions(row.certificate_validation_json),
    providerErrorCode: row.provider_error_code,
    lastObservedAt: row.last_observed_at === null ? null : validTimestamp(row.last_observed_at),
    ownerRegisteredAt: row.owner_registered_at === null ? null : validTimestamp(row.owner_registered_at),
    createdAt: validTimestamp(row.created_at),
    updatedAt: validTimestamp(row.updated_at),
    storageVersion: row.row_version,
  };
}

const REGISTRATION_STATES = new Set<DomainRegistrationState>([
  "pending_ownership",
  "pending_certificate",
  "pending_dns",
  "ready_for_owner",
  "active",
  "action_required",
]);

function registrationState(value: string): DomainRegistrationState {
  if (!REGISTRATION_STATES.has(value as DomainRegistrationState)) {
    throw new Error("registration_state_invalid");
  }
  return value as DomainRegistrationState;
}

export class WebCryptoClaimSecrets implements ClaimSecrets {
  async issue(): Promise<ClaimSecret> {
    const bytes = crypto.getRandomValues(new Uint8Array(32));
    const token = base64Url(bytes);
    return { token, hash: await this.hash(token) };
  }

  async hash(token: string): Promise<string> {
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
    return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  }
}

export class CloudflareCustomHostnameProvider implements CustomHostnameProvider {
  private readonly zoneId: string;
  private readonly apiToken: string;
  private readonly request: typeof fetch;

  constructor(zoneId: string, apiToken: string, request: typeof fetch = fetch) {
    this.zoneId = zoneId;
    this.apiToken = apiToken;
    this.request = request;
  }

  async create(domain: string): Promise<CustomHostname> {
    return this.call("", {
      method: "POST",
      body: JSON.stringify({ hostname: domain, ssl: { method: "txt", type: "dv" } }),
    });
  }

  async inspect(providerHostnameId: string): Promise<CustomHostname> {
    if (!/^[a-zA-Z0-9_-]+$/.test(providerHostnameId)) throw new Error("provider_hostname_id_invalid");
    return this.call(`/${providerHostnameId}`, { method: "GET" });
  }

  private async call(suffix: string, init: RequestInit): Promise<CustomHostname> {
    const response = await this.request(
      `https://api.cloudflare.com/client/v4/zones/${this.zoneId}/custom_hostnames${suffix}`,
      {
        ...init,
        headers: {
          Authorization: `Bearer ${this.apiToken}`,
          "Content-Type": "application/json",
        },
      },
    );
    if (!response.ok) throw new Error(`cloudflare_custom_hostname_http_${response.status}`);
    return parseCloudflareHostname(await response.json());
  }
}

export class CnameDomainRouteVerifier implements DomainRouteVerifier {
  private readonly request: typeof fetch;

  constructor(request: typeof fetch = fetch) {
    this.request = request;
  }

  async routesToService(domain: string, cnameTarget: string): Promise<boolean> {
    const query = new URL("https://cloudflare-dns.com/dns-query");
    query.searchParams.set("name", normalizeDomain(domain));
    query.searchParams.set("type", "CNAME");
    const response = await this.request(query, { headers: { Accept: "application/dns-json" } });
    if (!response.ok) throw new Error(`dns_query_http_${response.status}`);
    const value = await response.json() as unknown;
    if (!isRecord(value) || value.Status !== 0 || !Array.isArray(value.Answer)) return false;
    const target = normalizeDomain(cnameTarget);
    return value.Answer.some((answer) => isRecord(answer) && answer.type === 5 &&
      typeof answer.data === "string" && answer.data.toLowerCase().replace(/\.$/, "") === target);
  }
}

function parseCloudflareHostname(value: unknown): CustomHostname {
  if (!isRecord(value) || value.success !== true || !isRecord(value.result)) {
    throw new Error("cloudflare_custom_hostname_invalid");
  }
  const result = value.result;
  if (typeof result.id !== "string" || typeof result.status !== "string" || !isRecord(result.ssl)) {
    throw new Error("cloudflare_custom_hostname_invalid");
  }
  const sslStatus = result.ssl.status;
  if (typeof sslStatus !== "string") throw new Error("cloudflare_custom_hostname_invalid");
  const ownership = instruction(result.ownership_verification);
  const validation = Array.isArray(result.ssl.validation_records)
    ? result.ssl.validation_records.map(validationInstruction).filter(isInstruction)
    : [];
  return {
    id: result.id,
    hostnameStatus: mapStatus(result.status),
    certificateStatus: mapStatus(sslStatus),
    providerErrorCode: providerError(result),
    ownershipVerification: ownership,
    certificateValidation: validation,
  };
}

function mapStatus(status: string): ProvisioningStatus {
  if (status === "active") return "active";
  if (/failed|deleted|timed_out|blocked|error/.test(status)) return "failed";
  return "pending";
}

function providerError(result: Record<string, unknown>): string | null {
  if (!Array.isArray(result.verification_errors) || result.verification_errors.length === 0) return null;
  const first = result.verification_errors[0];
  if (isRecord(first) && (typeof first.code === "string" || typeof first.code === "number")) {
    return `cf_${String(first.code)}`;
  }
  return "cf_hostname_verification_failed";
}

function instruction(value: unknown): DnsInstruction | null {
  if (!isRecord(value) || value.type !== "txt" ||
      typeof value.name !== "string" || typeof value.value !== "string") return null;
  return { type: "txt", name: value.name, value: value.value };
}

function validationInstruction(value: unknown): DnsInstruction | null {
  if (!isRecord(value) || typeof value.txt_name !== "string" || typeof value.txt_value !== "string") {
    return null;
  }
  return { type: "txt", name: value.txt_name, value: value.txt_value };
}

function isInstruction(value: DnsInstruction | null): value is DnsInstruction {
  return value !== null;
}

function encodeOptional(value: unknown): string | null {
  return value === null ? null : JSON.stringify(value);
}

function decodeInstruction(value: string | null): DnsInstruction | null {
  return value === null ? null : instruction(JSON.parse(value) as unknown);
}

function decodeInstructions(value: string): DnsInstruction[] {
  const decoded = JSON.parse(value) as unknown;
  if (!Array.isArray(decoded)) throw new Error("registration_validation_records_invalid");
  const instructions = decoded.map(instruction);
  if (instructions.some((item) => item === null)) throw new Error("registration_validation_records_invalid");
  return instructions as DnsInstruction[];
}

function validTimestamp(value: string): string {
  if (!Number.isFinite(Date.parse(value))) throw new Error("registration_timestamp_invalid");
  return value;
}

function base64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
