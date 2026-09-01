import type { WorkerEnv, WorkerExecutionContext } from "./bindings.ts";
import { CoreAdminGateway, D1AdminRegistrationDirectory } from "./admin.ts";
import {
  CnameDomainRouteVerifier,
  CloudflareCustomHostnameProvider,
  D1RegistrationRepository,
  WebCryptoClaimSecrets,
} from "./cloudflare-registration.ts";
import {
  DurableObjectEngagement,
  KvHostDirectory,
  R2MediaDirectory,
  R2ReleaseDirectory,
} from "./cloudflare-store.ts";
import { handleControlRequest } from "./control.ts";
import { handleDoctorRequest } from "./doctor.ts";
import {
  D1OwnerActivationRepository,
  handleOwnerActivationRequest,
} from "./owner-activation.ts";
import {
  CloudflareInternalPublication,
  handleInternalPublicationRequest,
} from "./internal-publication.ts";
import { handlePublicRequest } from "./public.ts";
import { RegistrationService } from "./registration.ts";

export { SiteCoordinator } from "./site-object.ts";

export default {
  async fetch(request: Request, env: WorkerEnv, context: WorkerExecutionContext): Promise<Response> {
    const requestId = crypto.randomUUID();
    const started = Date.now();
    let status = 500;
    try {
      const hostname = new URL(request.url).hostname.toLowerCase();
      if (hostname === env.CONTROL_HOSTNAME.toLowerCase()) {
        if (new URL(request.url).pathname.startsWith("/internal/sites/")) {
          const response = await handleInternalPublicationRequest(
            request,
            new CloudflareInternalPublication(env.RELEASES, env.SITES, env.INTERNAL_DO_TOKEN),
            env.INTERNAL_DO_TOKEN,
          );
          status = response.status;
          return withRequestId(response, requestId);
        }
        if (new URL(request.url).pathname.startsWith("/internal/registrations/")) {
          const response = await handleOwnerActivationRequest(
            request,
            new D1OwnerActivationRepository(env.REGISTRY),
            env.INTERNAL_DO_TOKEN,
          );
          status = response.status;
          return withRequestId(response, requestId);
        }
        if (new URL(request.url).pathname === "/internal/doctor") {
          const response = await handleDoctorRequest(request, env);
          status = response.status;
          return withRequestId(response, requestId);
        }
        const registrations = new RegistrationService(
          new D1RegistrationRepository(env.REGISTRY),
          new CloudflareCustomHostnameProvider(env.CF_ZONE_ID, env.CF_API_TOKEN),
          new CnameDomainRouteVerifier(),
          new WebCryptoClaimSecrets(),
          () => new Date().toISOString(),
          () => crypto.randomUUID(),
          env.SAAS_CNAME_TARGET,
        );
        const response = await handleControlRequest(request, registrations);
        status = response.status;
        return withRequestId(response, requestId);
      }
      if (isUnissuedServiceSubdomain(hostname, env)) {
        const response = new Response("not found\n", { status: 404 });
        status = response.status;
        return withRequestId(response, requestId);
      }
      const hosts = new KvHostDirectory(env.HOSTS);
      const admin = await new CoreAdminGateway(
        hosts,
        new D1AdminRegistrationDirectory(env.REGISTRY),
        env.CORE,
        env.INTERNAL_DO_TOKEN,
      ).handle(request);
      if (admin !== null) {
        status = admin.status;
        return withRequestId(admin, requestId);
      }
      const response = await handlePublicRequest(
        request,
        {
          directory: hosts,
          releases: new R2ReleaseDirectory(env.RELEASES),
          media: new R2MediaDirectory(env.RELEASES),
          engagement: new DurableObjectEngagement(env.SITES, env.INTERNAL_DO_TOKEN),
          diagnostics: {
            failure(event, error) {
              diagnostic("warn", event, requestId, request, error);
            },
          },
        },
        context,
      );
      status = response.status;
      return withRequestId(response, requestId);
    } catch (error) {
      diagnostic("error", "request.failed", requestId, request, error);
      return withRequestId(new Response("internal server error\n", { status: 500 }), requestId);
    } finally {
      console.log(JSON.stringify({
        event: "request.completed",
        request_id: requestId,
        method: request.method,
        path: new URL(request.url).pathname,
        status,
        latency_ms: Date.now() - started,
      }));
    }
  },
};

function isUnissuedServiceSubdomain(hostname: string, env: WorkerEnv): boolean {
  const labels = env.SAAS_CNAME_TARGET.toLowerCase().split(".");
  const providerZone = labels.length > 2 ? labels.slice(1).join(".") : labels.join(".");
  return (hostname === providerZone || hostname.endsWith(`.${providerZone}`)) &&
    hostname !== env.ANONYMOUS_DEMO_HOSTNAME.toLowerCase();
}

function withRequestId(response: Response, requestId: string): Response {
  const wrapped = new Response(response.body, response);
  wrapped.headers.set("x-request-id", requestId);
  return wrapped;
}

function diagnostic(
  level: "warn" | "error",
  event: string,
  requestId: string,
  request: Request,
  error: unknown,
): void {
  console[level](JSON.stringify({
    event,
    request_id: requestId,
    method: request.method,
    path: new URL(request.url).pathname,
    error: error instanceof Error ? error.message : "unknown error",
  }));
}
