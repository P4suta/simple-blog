import assert from "node:assert/strict";
import test from "node:test";

import type { Fetcher } from "../src/bindings.ts";
import {
  CoreAdminGateway,
  type AdminRegistrationDirectory,
} from "../src/admin.ts";
import type { HostDirectory, HostedSite } from "../src/public.ts";

const siteId = "12345678-1234-1234-1234-123456789abc";

class Hosts implements HostDirectory {
  site: HostedSite | null = null;
  async lookup() { return this.site; }
}

class Registrations implements AdminRegistrationDirectory {
  site: { siteId: string; state: "ready_for_owner" | "active" } | null = null;
  async lookup() { return this.site; }
}

function setup() {
  const hosts = new Hosts();
  const registrations = new Registrations();
  const requests: Request[] = [];
  const core: Fetcher = {
    async fetch(request) {
      requests.push(request);
      return new Response("admin", {
        status: 200,
        headers: { "Set-Cookie": "sb_session=opaque; Secure; HttpOnly" },
      });
    },
  };
  const gateway = new CoreAdminGateway(
    hosts,
    registrations,
    core,
    "internal-secret-with-at-least-32-bytes",
  );
  return { hosts, registrations, requests, gateway };
}

test("active custom-domain admin requests preserve browser state but gain trusted site context", async () => {
  const state = setup();
  state.hosts.site = {
    siteId,
    activeRelease: "a".repeat(64),
    state: "active",
  };
  const request = new Request("https://writing.example.com/admin/content/7/edit/?tab=history", {
    headers: {
      Cookie: "sb_session=browser-capability",
      Authorization: "Bearer attacker-value",
      "CF-Connecting-IP": "203.0.113.9",
    },
  });

  const response = await state.gateway.handle(request);

  assert.equal(response?.status, 200);
  assert.equal(response?.headers.get("set-cookie"), "sb_session=opaque; Secure; HttpOnly");
  assert.equal(response?.headers.get("cache-control"), "no-store");
  assert.equal(state.requests.length, 1);
  const upstream = state.requests[0]!;
  assert.equal(
    upstream.url,
    `https://core.internal/internal/sites/${siteId}/http/admin/content/7/edit/?tab=history`,
  );
  assert.equal(upstream.headers.get("cookie"), "sb_session=browser-capability");
  assert.equal(
    upstream.headers.get("authorization"),
    "Bearer internal-secret-with-at-least-32-bytes",
  );
  assert.equal(upstream.headers.get("x-simple-blog-canonical-origin"), "https://writing.example.com");
  assert.equal(upstream.headers.get("cf-connecting-ip"), null);
});

test("a fully validated domain can reach owner setup before an active release exists", async () => {
  const state = setup();
  state.registrations.site = { siteId, state: "ready_for_owner" };

  const response = await state.gateway.handle(
    new Request("https://writing.example.com/admin/setup/#claim=not-sent-to-server"),
  );

  assert.equal(response?.status, 200);
  assert.equal(new URL(state.requests[0]!.url).pathname, `/internal/sites/${siteId}/http/admin/setup/`);
});

test("an activated owner retains admin access while the first release is still staging", async () => {
  const state = setup();
  state.registrations.site = { siteId, state: "active" };

  const response = await state.gateway.handle(
    new Request("https://writing.example.com/admin/"),
  );

  assert.equal(response?.status, 200);
  assert.equal(state.requests[0]?.headers.get("x-simple-blog-site-id"), siteId);
});

test("the gateway ignores public paths and fails closed for unknown admin hosts", async () => {
  const state = setup();
  assert.equal(await state.gateway.handle(new Request("https://writing.example.com/essay/")), null);

  const unknown = await state.gateway.handle(new Request("https://unknown.example.com/admin/"));
  assert.equal(unknown?.status, 404);
  assert.equal(state.requests.length, 0);

  const disallowed = await state.gateway.handle(new Request(
    "https://unknown.example.com/admin/",
    { method: "PUT" },
  ));
  assert.equal(disallowed?.status, 405);
});
