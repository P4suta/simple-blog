import assert from "node:assert/strict";
import test from "node:test";

import {
  handlePublicRequest,
  type HostedSite,
  type PublicDependencies,
  type WaitUntil,
} from "../src/public.ts";
import type { ReleaseManifest, ReleaseReader } from "../src/release.ts";

const releaseId = "a".repeat(64);
const objectId = "c".repeat(64);
const manifest: ReleaseManifest = {
  format_version: 1,
  compiler_version: "test",
  public_revision: 1,
  canonical_origin: "https://writing.example.com",
  routes: {
    "/": {
      kind: "asset",
      object_id: objectId,
      content_type: "text/html; charset=utf-8",
      cache_control: "public, max-age=0, must-revalidate",
      status: 200,
      content_id: 42,
    },
    "/404/": {
      kind: "asset",
      object_id: "b".repeat(64),
      content_type: "text/html; charset=utf-8",
      cache_control: "public, max-age=0, must-revalidate",
      status: 404,
    },
  },
};

class Context implements WaitUntil {
  readonly pending: Promise<unknown>[] = [];

  waitUntil(promise: Promise<unknown>): void {
    this.pending.push(promise);
  }
}

function dependencies(state: "active" | "pending_dns" = "active") {
  const views: number[] = [];
  const likes: Array<[number, "like" | "unlike"]> = [];
  const site: HostedSite = { siteId: "site-7", activeRelease: releaseId, state };
  const reader: ReleaseReader = {
    async activeRelease() {
      return releaseId;
    },
    async manifest() {
      return manifest;
    },
    async object(id) {
      if (id === objectId) return new TextEncoder().encode("home");
      return new TextEncoder().encode("missing");
    },
  };
  const deps: PublicDependencies = {
    directory: {
      async lookup(hostname) {
        return hostname === "writing.example.com" ? site : null;
      },
    },
    releases: { forSite: () => reader },
    media: {
      async get(_siteId, filename) {
        return filename === `${"d".repeat(64)}.gif`
          ? { bytes: new Uint8Array([1, 2]), contentType: "image/gif" }
          : null;
      },
    },
    engagement: {
      async recordView(_siteId, contentId) {
        views.push(contentId);
      },
      async toggleLike(_siteId, contentId, operation) {
        likes.push([contentId, operation]);
      },
    },
  };
  return { deps, views, likes };
}

test("only active, explicitly registered domains reach immutable releases", async () => {
  const active = dependencies();
  const context = new Context();
  const response = await handlePublicRequest(
    new Request("https://writing.example.com/"),
    active.deps,
    context,
  );
  assert.equal(response.status, 200);
  assert.equal(await response.text(), "home");
  await Promise.all(context.pending);
  assert.deepEqual(active.views, [42]);

  const unknown = await handlePublicRequest(
    new Request("https://made-up.service.example/"),
    active.deps,
    new Context(),
  );
  assert.equal(unknown.status, 404);

  const pending = dependencies("pending_dns");
  const unavailable = await handlePublicRequest(
    new Request("https://writing.example.com/"),
    pending.deps,
    new Context(),
  );
  assert.equal(unavailable.status, 404);
});

test("an active domain without a published release is temporarily unavailable", async () => {
  const state = dependencies();
  state.deps.releases = {
    forSite() {
      return {
        async activeRelease() { return null; },
        async manifest() { throw new Error("must not read a manifest"); },
        async object() { throw new Error("must not read an object"); },
      };
    },
  };

  const response = await handlePublicRequest(
    new Request("https://writing.example.com/"),
    state.deps,
    new Context(),
  );

  assert.equal(response.status, 503);
  assert.equal(response.headers.get("retry-after"), "5");
  assert.equal(response.headers.get("cache-control"), "no-store");
});

test("media and engagement stay dynamic without weakening release routing", async () => {
  const state = dependencies();
  const context = new Context();
  const filename = `${"d".repeat(64)}.gif`;
  const media = await handlePublicRequest(
    new Request(`https://writing.example.com/media/${filename}`),
    state.deps,
    context,
  );
  assert.equal(media.status, 200);
  assert.equal(media.headers.get("cache-control"), "public, max-age=31536000, immutable");
  assert.equal(media.headers.get("etag"), `"media-${filename}"`);

  const liked = await handlePublicRequest(
    new Request("https://writing.example.com/likes/42", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ op: "like" }),
    }),
    state.deps,
    context,
  );
  assert.equal(liked.status, 204);
  assert.deepEqual(state.likes, [[42, "like"]]);

  const crossSiteForm = await handlePublicRequest(
    new Request("https://writing.example.com/likes/42", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: "op=like",
    }),
    state.deps,
    context,
  );
  assert.equal(crossSiteForm.status, 415);
});

test("likes reject an oversized body even when Content-Length is absent", async () => {
  const state = dependencies();
  const response = await handlePublicRequest(
    new Request("https://writing.example.com/likes/42", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ op: "like", padding: "x".repeat(2048) }),
    }),
    state.deps,
    new Context(),
  );

  assert.equal(response.status, 413);
  assert.deepEqual(state.likes, []);
});

test("a rejected asynchronous host lookup fails closed as not found", async () => {
  const state = dependencies();
  state.deps.directory = {
    async lookup() {
      throw new Error("untrusted adapter failure");
    },
  };

  const response = await handlePublicRequest(
    new Request("https://writing.example.com/"),
    state.deps,
    new Context(),
  );

  assert.equal(response.status, 404);
});
