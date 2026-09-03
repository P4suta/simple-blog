import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

/**
 * Wrangler reads JSON with comments and trailing commas. `JSON.parse` would
 * accept the template only for as long as nobody writes a comment into a file
 * whose extension invites them, so the test strips both before parsing while
 * leaving string contents untouched.
 */
function parseJsonc(source: string): Record<string, unknown> {
  let output = "";
  let index = 0;
  while (index < source.length) {
    const character = source[index];
    if (character === '"') {
      let end = index + 1;
      while (end < source.length && source[end] !== '"') {
        if (source[end] === "\\") end += 1;
        end += 1;
      }
      output += source.slice(index, end + 1);
      index = end + 1;
      continue;
    }
    if (source.startsWith("//", index)) {
      const end = source.indexOf("\n", index);
      index = end === -1 ? source.length : end;
      continue;
    }
    if (source.startsWith("/*", index)) {
      const end = source.indexOf("*/", index + 2);
      index = end === -1 ? source.length : end + 2;
      continue;
    }
    if (character === ",") {
      let ahead = index + 1;
      // A comment may sit between the final comma and the closing bracket.
      while (ahead < source.length) {
        if (/\s/.test(source[ahead])) {
          ahead += 1;
        } else if (source.startsWith("//", ahead)) {
          const end = source.indexOf("\n", ahead);
          ahead = end === -1 ? source.length : end;
        } else if (source.startsWith("/*", ahead)) {
          const end = source.indexOf("*/", ahead + 2);
          ahead = end === -1 ? source.length : end + 2;
        } else {
          break;
        }
      }
      if (source[ahead] === "}" || source[ahead] === "]") {
        index += 1;
        continue;
      }
    }
    output += character;
    index += 1;
  }
  const parsed: unknown = JSON.parse(output);
  assert.equal(typeof parsed === "object" && parsed !== null && !Array.isArray(parsed), true);
  return parsed as Record<string, unknown>;
}

test("the JSONC reader keeps string contents and drops comments and trailing commas", () => {
  const parsed = parseJsonc(`{
    // line comment
    "url": "https://example.com/path?a=1,b=2", /* block */
    "list": [1, 2, 3,],
    "nested": { "keep": "// not a comment", },
  }`);

  assert.deepEqual(parsed, {
    url: "https://example.com/path?a=1,b=2",
    list: [1, 2, 3],
    nested: { keep: "// not a comment" },
  });
  assert.throws(() => parseJsonc("[1, 2]"));
});

test("deployment template denies service subdomains and declares every durable boundary", () => {
  const source = readFileSync(
    new URL("../wrangler.example.jsonc", import.meta.url),
    "utf8",
  );
  const config = parseJsonc(source);

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
