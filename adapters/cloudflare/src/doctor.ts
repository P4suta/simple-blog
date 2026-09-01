import type { WorkerEnv } from "./bindings.ts";
import { normalizeDomain } from "./domain.ts";
import { authorizedBearer } from "./internal-auth.ts";

type CheckStatus = "ok" | "failed";

interface DoctorCheck {
  component: string;
  status: CheckStatus;
  diagnostic_code: string | null;
  latency_ms: number;
}

interface DoctorReport {
  format_version: 1;
  status: "ok" | "degraded";
  checks: DoctorCheck[];
}

const DEADLINE_MILLISECONDS = 2_000;

export async function handleDoctorRequest(
  request: Request,
  env: WorkerEnv,
  monotonic: () => number = () => performance.now(),
): Promise<Response> {
  if (new URL(request.url).pathname !== "/internal/doctor") {
    return response({ error: "diagnostic_route_not_found" }, 404);
  }
  if (!(await authorizedBearer(request, env.DIAGNOSTIC_TOKEN))) {
    const unauthorized = response({ error: "diagnostic_capability_required" }, 401);
    unauthorized.headers.set("WWW-Authenticate", "Bearer");
    return unauthorized;
  }
  if (request.method !== "GET") {
    const invalid = response({ error: "diagnostic_method_not_allowed" }, 405);
    invalid.headers.set("Allow", "GET");
    return invalid;
  }

  const definitions: Array<[string, string, () => Promise<void>]> = [
    ["configuration", "CF_CONFIGURATION_INVALID", async () => validateConfiguration(env)],
    ["d1", "CF_D1_UNREACHABLE", async () => {
      const row = await env.REGISTRY.prepare("SELECT 1 AS ok").first<{ ok: number }>();
      if (row?.ok !== 1) throw new Error("unexpected probe response");
    }],
    ["durable_object", "CF_DURABLE_OBJECT_UNREACHABLE", async () => {
      const stub = env.SITES.get(env.SITES.idFromName("__simple_blog_diagnostics__"));
      const result = await stub.fetch(new Request("https://site.internal/diagnostics", {
        method: "POST",
        headers: { Authorization: `Bearer ${env.INTERNAL_DO_TOKEN}` },
      }));
      if (!result.ok) throw new Error("durable object probe failed");
    }],
    ["kv", "CF_KV_UNREACHABLE", async () => {
      await env.HOSTS.get("diagnostics/connectivity-probe", "json");
    }],
    ["r2", "CF_R2_UNREACHABLE", async () => {
      if (env.RELEASES.head === undefined) throw new Error("R2 head is unavailable");
      await env.RELEASES.head("diagnostics/connectivity-probe");
    }],
    ["core", "CF_CORE_UNHEALTHY", async () => {
      const result = await env.CORE.fetch(new Request("https://core.internal/internal/healthz", {
        method: "GET",
        headers: { Authorization: `Bearer ${env.INTERNAL_DO_TOKEN}` },
      }));
      if (!result.ok) throw new Error("Core probe failed");
    }],
  ];
  const checks = await Promise.all(definitions.map(([component, code, check]) =>
    runCheck(component, code, check, monotonic)
  ));
  const healthy = checks.every((check) => check.status === "ok");
  const report: DoctorReport = {
    format_version: 1,
    status: healthy ? "ok" : "degraded",
    checks,
  };
  return response(report, healthy ? 200 : 503);
}

async function runCheck(
  component: string,
  diagnosticCode: string,
  check: () => Promise<void>,
  monotonic: () => number,
): Promise<DoctorCheck> {
  const started = monotonic();
  try {
    await deadline(check(), DEADLINE_MILLISECONDS);
    return {
      component,
      status: "ok",
      diagnostic_code: null,
      latency_ms: elapsed(started, monotonic()),
    };
  } catch {
    console.warn(JSON.stringify({
      event: "doctor.check.failed",
      component,
      diagnostic_code: diagnosticCode,
    }));
    return {
      component,
      status: "failed",
      diagnostic_code: diagnosticCode,
      latency_ms: elapsed(started, monotonic()),
    };
  }
}

function validateConfiguration(env: WorkerEnv): void {
  const control = normalizeDomain(env.CONTROL_HOSTNAME);
  const demo = normalizeDomain(env.ANONYMOUS_DEMO_HOSTNAME);
  const target = normalizeDomain(env.SAAS_CNAME_TARGET);
  if (
    control !== env.CONTROL_HOSTNAME ||
    demo !== env.ANONYMOUS_DEMO_HOSTNAME ||
    target !== env.SAAS_CNAME_TARGET ||
    new Set([control, demo, target]).size !== 3 ||
    env.CF_ZONE_ID.trim() === "" ||
    env.CF_API_TOKEN.length < 32 ||
    env.INTERNAL_DO_TOKEN.length < 32 ||
    env.DIAGNOSTIC_TOKEN.length < 32
  ) {
    throw new Error("configuration invalid");
  }
}

async function deadline<T>(operation: Promise<T>, milliseconds: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<T>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error("diagnostic deadline exceeded")), milliseconds);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function elapsed(started: number, finished: number): number {
  return Math.max(0, Math.round((finished - started) * 100) / 100);
}

function response(value: unknown, status: number): Response {
  return Response.json(value, {
    status,
    headers: {
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
}
