import type {
  DurableObjectState,
  DurableObjectStorage,
  SiteCoordinatorEnv,
} from "./bindings.ts";
import { authorizedBearer } from "./internal-auth.ts";
import { CloudflareReleaseActivator, type ReleaseCandidate } from "./publication.ts";

interface Engagement {
  likes: number;
  views: number;
}

interface PublicationState {
  site_id: string;
  public_content_ids: number[];
  next_publish_at: string | null;
}

const SITE_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const RETRY_MILLISECONDS = 5 * 60 * 1000;

export class SiteCoordinator {
  private readonly storage: DurableObjectStorage;
  private readonly env: SiteCoordinatorEnv;

  constructor(state: DurableObjectState, env: SiteCoordinatorEnv) {
    this.storage = state.storage;
    this.env = env;
  }

  async fetch(request: Request): Promise<Response> {
    if (!(await authorizedBearer(request, this.env.INTERNAL_DO_TOKEN))) {
      return new Response(null, { status: 401 });
    }
    if (request.method !== "POST") {
      return new Response(null, { status: 405, headers: { Allow: "POST" } });
    }
    const path = new URL(request.url).pathname;
    const view = /^\/views\/([1-9][0-9]*)$/.exec(path);
    if (view !== null) return this.engage(Number(view[1]), "view");
    const like = /^\/likes\/([1-9][0-9]*)$/.exec(path);
    if (like !== null) return this.like(request, Number(like[1]));
    if (path === "/publication/state") return this.publicationState(request);
    if (path === "/publication/activate") return this.activateRelease(request);
    if (path === "/diagnostics") {
      return Response.json({
        status: "ok",
        alarm_scheduled: await this.storage.getAlarm() !== null,
      });
    }
    return new Response(null, { status: 404 });
  }

  async alarm(): Promise<void> {
    const publication = await this.storage.get<PublicationState>("publication");
    if (publication === undefined) {
      console.error(JSON.stringify({ event: "publication.alarm.missing_state" }));
      return;
    }
    try {
      const response = await this.env.CORE.fetch(new Request(
        `https://core.internal/internal/sites/${publication.site_id}/publish`,
        {
          method: "POST",
          headers: { Authorization: `Bearer ${this.env.INTERNAL_DO_TOKEN}` },
        },
      ));
      if (!response.ok) throw new Error(`core_publish_${response.status}`);
      console.log(JSON.stringify({ event: "publication.alarm.completed", site_id: publication.site_id }));
    } catch (error) {
      const retryAt = Date.now() + RETRY_MILLISECONDS;
      await this.storage.setAlarm(retryAt);
      console.error(JSON.stringify({
        event: "publication.alarm.retry_scheduled",
        site_id: publication.site_id,
        retry_at: new Date(retryAt).toISOString(),
        error: error instanceof Error ? error.message : "unknown error",
      }));
    }
  }

  private async engage(contentId: number, operation: "view"): Promise<Response> {
    if (!(await this.isPublic(contentId))) return new Response(null, { status: 404 });
    const key = `engagement/${contentId}`;
    const current = await this.storage.get<Engagement>(key) ?? { likes: 0, views: 0 };
    if (operation === "view") current.views = Math.min(Number.MAX_SAFE_INTEGER, current.views + 1);
    await this.storage.put(key, current);
    return new Response(null, { status: 204 });
  }

  private async like(request: Request, contentId: number): Promise<Response> {
    if (!(await this.isPublic(contentId))) return new Response(null, { status: 404 });
    let operation: unknown;
    try {
      operation = (await request.json() as { operation?: unknown }).operation;
    } catch {
      return new Response(null, { status: 400 });
    }
    if (operation !== "like" && operation !== "unlike") return new Response(null, { status: 422 });
    const key = `engagement/${contentId}`;
    const current = await this.storage.get<Engagement>(key) ?? { likes: 0, views: 0 };
    current.likes = operation === "like"
      ? Math.min(Number.MAX_SAFE_INTEGER, current.likes + 1)
      : Math.max(0, current.likes - 1);
    await this.storage.put(key, current);
    return new Response(null, { status: 204 });
  }

  private async publicationState(request: Request): Promise<Response> {
    let value: unknown;
    try {
      value = await request.json();
    } catch {
      return new Response(null, { status: 400 });
    }
    if (!validPublication(value)) return new Response(null, { status: 422 });
    await this.setPublication({
      site_id: value.site_id,
      public_content_ids: [...new Set(value.public_content_ids)].sort((left, right) => left - right),
      next_publish_at: value.next_publish_at,
    });
    return new Response(null, { status: 204 });
  }

  private async activateRelease(request: Request): Promise<Response> {
    if (this.env.RELEASES === undefined || this.env.HOSTS === undefined) {
      return new Response(null, { status: 503 });
    }
    let value: unknown;
    try {
      value = await request.json();
    } catch {
      return new Response(null, { status: 400 });
    }
    const candidate = releaseCandidate(value);
    if (candidate === null) return new Response(null, { status: 422 });
    try {
      const result = await new CloudflareReleaseActivator(
        this.env.RELEASES,
        this.env.HOSTS,
        this.storage,
      ).activate(candidate);
      await this.setPublication({
        site_id: candidate.siteId,
        public_content_ids: result.publicContentIds,
        next_publish_at: candidate.nextPublishAt,
      });
      return Response.json({ changed: result.changed }, { status: 200 });
    } catch (error) {
      if (error instanceof Error && error.message === "release_activation_conflict") {
        return Response.json({ error: error.message }, { status: 409 });
      }
      throw error;
    }
  }

  private async setPublication(canonical: PublicationState): Promise<void> {
    await this.storage.put("publication", canonical);
    if (canonical.next_publish_at === null) {
      await this.storage.deleteAlarm();
    } else {
      await this.storage.setAlarm(Date.parse(canonical.next_publish_at));
    }
  }

  private async isPublic(contentId: number): Promise<boolean> {
    const publication = await this.storage.get<PublicationState>("publication");
    return publication?.public_content_ids.includes(contentId) ?? false;
  }
}

function releaseCandidate(value: unknown): ReleaseCandidate | null {
  if (!isRecord(value) || JSON.stringify(Object.keys(value).sort()) !== JSON.stringify([
    "domain",
    "expected_release",
    "next_publish_at",
    "replacement_release",
    "site_id",
  ])) return null;
  if (typeof value.site_id !== "string" || typeof value.domain !== "string" ||
      (value.expected_release !== null && typeof value.expected_release !== "string") ||
      typeof value.replacement_release !== "string" ||
      (value.next_publish_at !== null && typeof value.next_publish_at !== "string")) return null;
  return {
    siteId: value.site_id,
    domain: value.domain,
    expectedRelease: value.expected_release,
    replacementRelease: value.replacement_release,
    nextPublishAt: value.next_publish_at,
  };
}

function validPublication(value: unknown): value is PublicationState {
  if (!isRecord(value) ||
      JSON.stringify(Object.keys(value).sort()) !==
        JSON.stringify(["next_publish_at", "public_content_ids", "site_id"]) ||
      typeof value.site_id !== "string" || !SITE_ID.test(value.site_id) ||
      !Array.isArray(value.public_content_ids) ||
      !value.public_content_ids.every((id) => Number.isSafeInteger(id) && id > 0)) return false;
  return value.next_publish_at === null ||
    (typeof value.next_publish_at === "string" && Number.isFinite(Date.parse(value.next_publish_at)));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
