import assert from "node:assert/strict";
import test from "node:test";

import {
  handleControlRequest,
  type RegistrationAbuseGuard,
  type RegistrationControl,
} from "../src/control.ts";

const allowed: RegistrationAbuseGuard = {
  async allow() { return true; },
};

function control(): { service: RegistrationControl; calls: unknown[][] } {
  const calls: unknown[][] = [];
  const service: RegistrationControl = {
    async start(domain) {
      calls.push(["start", domain]);
      return {
        id: "00000000-0000-4000-8000-000000000001",
        domain,
        state: "pending_ownership",
        cnameTarget: "customers.service.dev",
        ownershipVerification: null,
        certificateValidation: [],
        providerErrorCode: null,
        ownerSetupUrl: null,
        claimToken: "x".repeat(43),
      };
    },
    async refresh(id, claim) {
      calls.push(["refresh", id, claim]);
      return {
        id,
        domain: "blog.writer.com",
        state: "ready_for_owner",
        cnameTarget: "customers.service.dev",
        ownershipVerification: null,
        certificateValidation: [],
        providerErrorCode: null,
        ownerSetupUrl: "https://blog.writer.com/admin/setup/",
      };
    },
  };
  return { service, calls };
}

test("control API creates domain-first registrations and returns the claim once", async () => {
  const state = control();
  const response = await handleControlRequest(
    new Request("https://control.service.dev/v1/registrations", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ domain: "blog.writer.com" }),
    }),
    state.service,
    allowed,
  );

  assert.equal(response.status, 201);
  assert.equal(response.headers.get("cache-control"), "no-store");
  const body = await response.json() as Record<string, unknown>;
  assert.equal(body.claimToken, "x".repeat(43));
  assert.deepEqual(state.calls, [["start", "blog.writer.com"]]);
});

test("refresh requires a bearer claim and unknown input fails closed", async () => {
  const state = control();
  const path = "/v1/registrations/00000000-0000-4000-8000-000000000001/refresh";
  const unauthorized = await handleControlRequest(
    new Request(`https://control.service.dev${path}`, { method: "POST" }),
    state.service,
    allowed,
  );
  assert.equal(unauthorized.status, 401);

  const refreshed = await handleControlRequest(
    new Request(`https://control.service.dev${path}`, {
      method: "POST",
      headers: { authorization: `Bearer ${"x".repeat(43)}` },
    }),
    state.service,
    allowed,
  );
  assert.equal(refreshed.status, 200);
  assert.equal((await refreshed.json() as { state: string }).state, "ready_for_owner");

  const extraField = await handleControlRequest(
    new Request("https://control.service.dev/v1/registrations", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ domain: "blog.writer.com", plan: "unlimited" }),
    }),
    state.service,
    allowed,
  );
  assert.equal(extraField.status, 422);
});

test("registration abuse control runs before parsing or provisioning", async () => {
  const state = control();
  let checks = 0;
  const denied: RegistrationAbuseGuard = {
    async allow() {
      checks += 1;
      return false;
    },
  };

  const response = await handleControlRequest(
    new Request("https://control.service.dev/v1/registrations", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "not JSON and must never be read",
    }),
    state.service,
    denied,
  );

  assert.equal(response.status, 429);
  assert.equal(response.headers.get("retry-after"), "60");
  assert.equal(checks, 1);
  assert.deepEqual(state.calls, []);
});
