import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  nextDomainState,
  normalizeDomain,
  type DomainRegistrationState,
  type ProvisioningStatus,
} from "../src/domain.ts";

interface ContractCase {
  name: string;
  owner_registered: boolean;
  hostname: ProvisioningStatus;
  certificate: ProvisioningStatus;
  dns_routed: boolean;
  expected: DomainRegistrationState;
}

interface Contract {
  format_version: number;
  cases: ContractCase[];
}

test("Cloudflare adapter satisfies the Rust Core domain contract", () => {
  const contract = JSON.parse(
    readFileSync(new URL("../../../contracts/domain-registration-v1.json", import.meta.url), "utf8"),
  ) as Contract;
  assert.equal(contract.format_version, 1);
  for (const scenario of contract.cases) {
    assert.equal(
      nextDomainState(scenario.owner_registered, scenario),
      scenario.expected,
      scenario.name,
    );
  }
});

test("domain normalization matches the Core identity rules", () => {
  assert.equal(normalizeDomain("BLOG.Writer.Co.Jp"), "blog.writer.co.jp");
  for (const invalid of [
    "localhost",
    "127.0.0.1",
    "https://blog.example.com",
    "*.example.com",
    "blog.example.com.",
    "ブログ.example.com",
  ]) {
    assert.throws(() => normalizeDomain(invalid), /invalid_domain/, invalid);
  }
});
