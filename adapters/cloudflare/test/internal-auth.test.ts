import assert from "node:assert/strict";
import test from "node:test";

import { authorizedBearer } from "../src/internal-auth.ts";

function request(token: string): Request {
  return new Request("https://site.internal/diagnostics", {
    headers: { Authorization: `Bearer ${token}` },
  });
}

test("internal capabilities fail closed when the configured secret is missing or short", async () => {
  assert.equal(await authorizedBearer(request(""), ""), false);
  assert.equal(await authorizedBearer(request("x".repeat(31)), "x".repeat(31)), false);
  assert.equal(await authorizedBearer(request("x".repeat(32)), "x".repeat(32)), true);
  assert.equal(await authorizedBearer(request("y".repeat(32)), "x".repeat(32)), false);
});
