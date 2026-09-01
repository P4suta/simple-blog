import type { RegistrationView, StartedRegistration } from "./registration.ts";

export interface RegistrationControl {
  start(domain: string): Promise<StartedRegistration>;
  refresh(id: string, claimToken: string): Promise<RegistrationView>;
}

const REGISTRATION = "/v1/registrations";
const REFRESH = /^\/v1\/registrations\/([0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})\/refresh$/;

export async function handleControlRequest(
  request: Request,
  registrations: RegistrationControl,
): Promise<Response> {
  const path = new URL(request.url).pathname;
  if (path === "/healthz" && request.method === "GET") {
    return new Response("ok\n", { headers: { "Content-Type": "text/plain; charset=utf-8" } });
  }
  try {
    if (path === REGISTRATION && request.method === "POST") {
      const body = await strictJson(request, new Set(["domain"]));
      if (typeof body.domain !== "string") return problem(422, "invalid_registration");
      return json(await registrations.start(body.domain), 201);
    }
    const refresh = REFRESH.exec(path);
    if (refresh !== null && request.method === "POST") {
      const claim = bearer(request);
      if (claim === null) {
        const response = problem(401, "claim_required");
        response.headers.set("WWW-Authenticate", "Bearer");
        return response;
      }
      return json(await registrations.refresh(refresh[1]!, claim), 200);
    }
    return problem(404, "control_route_not_found");
  } catch (error) {
    const message = error instanceof Error ? error.message : "";
    if (message === "invalid_json" || message === "invalid_fields") {
      return problem(422, "invalid_registration");
    }
    if (message === "request_too_large") return problem(413, message);
    if (message === "invalid_domain") return problem(422, message);
    if (message === "domain_unavailable") return problem(409, message);
    if (message === "registration_not_found") return problem(404, message);
    throw error;
  }
}

async function strictJson(
  request: Request,
  fields: Set<string>,
): Promise<Record<string, unknown>> {
  if (request.headers.get("content-type")?.split(";", 1)[0]?.trim() !== "application/json") {
    throw new Error("invalid_json");
  }
  const text = await request.text();
  if (new TextEncoder().encode(text).byteLength > 2048) throw new Error("request_too_large");
  let value: unknown;
  try {
    value = JSON.parse(text) as unknown;
  } catch {
    throw new Error("invalid_json");
  }
  if (!isRecord(value) || Object.keys(value).some((key) => !fields.has(key)) ||
      [...fields].some((key) => !(key in value))) {
    throw new Error("invalid_fields");
  }
  return value;
}

function bearer(request: Request): string | null {
  const authorization = request.headers.get("authorization");
  if (authorization === null || !authorization.startsWith("Bearer ")) return null;
  const token = authorization.slice("Bearer ".length);
  return token.length >= 16 && token.length <= 256 ? token : null;
}

function json(value: unknown, status: number): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}

function problem(status: number, code: string): Response {
  return json({ error: code }, status);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
