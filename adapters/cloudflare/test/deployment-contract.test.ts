import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { parseConfigFileTextToJson } from "typescript";

test("deployment template denies service subdomains and declares every durable boundary", () => {
  const source = readFileSync(
    new URL("../wrangler.example.jsonc", import.meta.url),
    "utf8",
  );
  const parsed = parseConfigFileTextToJson("wrangler.example.jsonc", source);
  assert.equal(parsed.error, undefined);
  const config = parsed.config as Record<string, unknown>;

  assert.equal(config.main, "src/index.ts");
  assert.equal(config.compatibility_date, "2026-09-02");
  assert.equal(config.workers_dev, false);
  assert.equal(config.preview_urls, false);
  assert.deepEqual(config.routes, [{ pattern: "*/*", zone_name: "replace-with-saas-zone" }]);
  assert.deepEqual(
    (config.kv_namespaces as Array<{ binding: string }>).map((value) => value.binding),
    ["HOSTS"],
  );
  assert.deepEqual(
    (config.r2_buckets as Array<{ binding: string }>).map((value) => value.binding),
    ["RELEASES"],
  );
  assert.deepEqual(config.ratelimits, [{
    name: "REGISTRATION_RATE_LIMITER",
    namespace_id: "1001",
    simple: { limit: 10, period: 60 },
  }]);
  assert.deepEqual(
    (config.d1_databases as Array<{ binding: string }>).map((value) => value.binding),
    ["REGISTRY"],
  );
  assert.deepEqual(config.durable_objects, {
    bindings: [{ name: "SITES", class_name: "SiteCoordinator" }],
  });
  assert.deepEqual(config.migrations, [{
    tag: "v1",
    new_sqlite_classes: ["SiteCoordinator"],
  }]);
  assert.deepEqual(config.services, [{
    binding: "CORE",
    service: "replace-with-simple-blog-core-service",
  }]);
  assert.deepEqual(config.observability, { enabled: true });

  const variables = config.vars as Record<string, unknown>;
  for (const secret of ["CF_API_TOKEN", "INTERNAL_DO_TOKEN", "DIAGNOSTIC_TOKEN"]) {
    assert.equal(secret in variables, false, `${secret} must be installed as a Worker secret`);
  }
  assert.equal("limits" in config, false, "the product contract has no arbitrary usage quota");
});
