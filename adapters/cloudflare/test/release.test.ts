import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  parseManifest,
  resolveRelease,
  serveRelease,
  type ReleaseReader,
  type ResolvedRoute,
} from "../src/release.ts";

interface Contract {
  format_version: number;
  active_release: string;
  manifest: unknown;
  objects: Record<string, string>;
  cases: Array<Record<string, unknown> & { path: string; kind: string }>;
}

function contract(): Contract {
  return JSON.parse(
    readFileSync(new URL("../../../contracts/release-resolution-v1.json", import.meta.url), "utf8"),
  ) as Contract;
}

function reader(data = contract()): ReleaseReader {
  return {
    async activeRelease() {
      return data.active_release;
    },
    async manifest() {
      return data.manifest;
    },
    async object(objectId) {
      const encoded = data.objects[objectId];
      return encoded === undefined ? null : Uint8Array.from(Buffer.from(encoded, "base64"));
    },
  };
}

test("Cloudflare resolver satisfies the versioned Core release cases", async () => {
  const data = contract();
  assert.equal(data.format_version, 1);
  for (const scenario of data.cases) {
    const actual = await resolveRelease(reader(data), scenario.path);
    assert.equal(actual.kind, scenario.kind, scenario.path);
    assert.equal(actual.status, scenario.status, scenario.path);
    if (actual.kind === "asset") {
      assert.equal(actual.object_id, scenario.object_id);
      assert.equal(actual.fallback, scenario.fallback);
    } else {
      assert.equal(actual.location, scenario.location);
    }
  }
});

test("HTTP adapter preserves Core metadata, HEAD, conditional, redirect, and 404 behavior", async () => {
  const essay = await serveRelease(
    new Request("https://writing.example.com/essay/", { method: "HEAD" }),
    reader(),
  );
  assert.equal(essay.status, 200);
  assert.equal(await essay.text(), "");
  assert.equal(essay.headers.get("etag"), `"blake3-${"c".repeat(64)}"`);
  assert.equal(essay.headers.get("x-simple-blog-release"), "a".repeat(64));
  assert.equal(essay.headers.get("last-modified"), "Wed, 02 Sep 2026 01:02:03 GMT");

  const fresh = await serveRelease(
    new Request("https://writing.example.com/essay/", {
      headers: { "If-None-Match": `W/"blake3-${"c".repeat(64)}"` },
    }),
    reader(),
  );
  assert.equal(fresh.status, 304);
  assert.equal(fresh.headers.get("content-type"), null);

  const redirect = await serveRelease(
    new Request("https://writing.example.com/essay"),
    reader(),
  );
  assert.equal(redirect.status, 308);
  assert.equal(redirect.headers.get("location"), "/essay/");

  const missing = await serveRelease(
    new Request("https://writing.example.com/not-here"),
    reader(),
  );
  assert.equal(missing.status, 404);
  assert.equal(await missing.text(), "missing");
});

test("unknown manifest fields and missing immutable objects fail closed", async () => {
  const unknown = contract();
  (unknown.manifest as Record<string, unknown>).future = true;
  assert.throws(() => parseManifest(unknown.manifest), /release_manifest_fields/);

  const missing = contract();
  missing.objects = {};
  await assert.rejects(
    resolveRelease(reader(missing), "/essay/"),
    /release_object_missing/,
  );
});

void ({} as ResolvedRoute);
