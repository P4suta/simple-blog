import assert from "node:assert/strict";
import test from "node:test";

import { registrationRateLimitKey } from "../src/index.ts";

test("registration rate limits isolate trusted Cloudflare source addresses", () => {
  const from = (address?: string) => registrationRateLimitKey(new Request(
    "https://control.service.dev/v1/registrations",
    address === undefined ? undefined : { headers: { "cf-connecting-ip": address } },
  ));

  assert.equal(from("203.0.113.7"), "source:203.0.113.7");
  assert.equal(from("2001:DB8::7"), "source:2001:db8::7");
  assert.notEqual(from("203.0.113.7"), from("203.0.113.8"));
  for (const untrusted of [undefined, "", "not-an-ip", "x".repeat(65)]) {
    assert.equal(from(untrusted), "source:unknown");
  }
});
