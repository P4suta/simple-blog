import assert from "node:assert/strict";
import test from "node:test";

import type {
  DurableObjectId,
  DurableObjectNamespace,
  DurableObjectStub,
  R2Bucket,
} from "../src/bindings.ts";
import {
  CloudflareInternalPublication,
  handleInternalPublicationRequest,
  type InternalPublication,
} from "../src/internal-publication.ts";
import type { StageManifestInput, StageObjectInput } from "../src/staging.ts";
import type { ReleaseCandidate } from "../src/publication.ts";

const siteId = "12345678-1234-4234-8234-123456789abc";
const objectId = "b".repeat(64);
const releaseId = "a".repeat(64);
const sha256 = "c".repeat(64);
const token = `test-only-publication-${"x".repeat(32)}`;

class Publication implements InternalPublication {
  calls: Array<[string, unknown]> = [];
  async stageObject(input: StageObjectInput) {
    this.calls.push(["object", input]);
    return { created: true };
  }
  async stageManifest(input: StageManifestInput) {
    this.calls.push(["manifest", input]);
    return { created: false };
  }
  async activate(input: ReleaseCandidate) {
    this.calls.push(["activate", input]);
    return { changed: true };
  }
}

function headers(extra: Record<string, string> = {}) {
  return {
    Authorization: `Bearer ${token}`,
    ...extra,
  };
}

test("Core stages objects and manifests without changing public visibility", async () => {
  const publication = new Publication();
  const object = await handleInternalPublicationRequest(new Request(
    `https://control.service.dev/internal/sites/${siteId}/release-objects/${objectId}`,
    {
      method: "PUT",
      headers: headers({ "X-Simple-Blog-SHA256": sha256 }),
      body: "immutable",
    },
  ), publication, token);
  assert.equal(object.status, 201);

  const manifest = await handleInternalPublicationRequest(new Request(
    `https://control.service.dev/internal/sites/${siteId}/release-manifests/${releaseId}`,
    {
      method: "PUT",
      headers: headers({
        "Content-Type": "application/json",
        "X-Simple-Blog-SHA256": sha256,
        "X-Simple-Blog-Domain": "writing.example.com",
      }),
      body: "{}",
    },
  ), publication, token);
  assert.equal(manifest.status, 200);

  assert.equal(publication.calls[0]?.[0], "object");
  assert.deepEqual((publication.calls[0]?.[1] as StageObjectInput).bytes, new TextEncoder().encode("immutable"));
  assert.deepEqual(publication.calls[1], ["manifest", {
    siteId,
    releaseId,
    domain: "writing.example.com",
    sha256,
    bytes: new TextEncoder().encode("{}"),
  }]);
});

test("activation is a separate CAS operation with an exact body", async () => {
  const publication = new Publication();
  const response = await handleInternalPublicationRequest(new Request(
    `https://control.service.dev/internal/sites/${siteId}/releases/${releaseId}/activate`,
    {
      method: "POST",
      headers: headers({ "Content-Type": "application/json" }),
      body: JSON.stringify({
        domain: "writing.example.com",
        expected_release: null,
        next_publish_at: "2026-09-03T12:00:00.000Z",
      }),
    },
  ), publication, token);

  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { changed: true });
  assert.deepEqual(publication.calls, [["activate", {
    siteId,
    domain: "writing.example.com",
    expectedRelease: null,
    replacementRelease: releaseId,
    nextPublishAt: "2026-09-03T12:00:00.000Z",
  }]]);
});

test("publication internals fail closed before reading bodies", async () => {
  const publication = new Publication();
  const unauthorized = await handleInternalPublicationRequest(new Request(
    `https://control.service.dev/internal/sites/${siteId}/release-objects/${objectId}`,
    { method: "PUT", body: "secret bytes" },
  ), publication, token);
  assert.equal(unauthorized.status, 401);
  assert.equal(publication.calls.length, 0);

  const extra = await handleInternalPublicationRequest(new Request(
    `https://control.service.dev/internal/sites/${siteId}/releases/${releaseId}/activate`,
    {
      method: "POST",
      headers: headers({ "Content-Type": "application/json" }),
      body: JSON.stringify({
        domain: "writing.example.com",
        expected_release: null,
        next_publish_at: null,
        force: true,
      }),
    },
  ), publication, token);
  assert.equal(extra.status, 422);
  assert.equal(publication.calls.length, 0);
});

test("Cloudflare activation client reaches the site coordinator with its internal capability", async () => {
  const requests: Request[] = [];
  const stub: DurableObjectStub = {
    async fetch(request) {
      requests.push(request);
      return Response.json({ changed: false });
    },
  };
  const sites: DurableObjectNamespace = {
    idFromName: () => ({} as DurableObjectId),
    get: () => stub,
  };
  const bucket: R2Bucket = {
    async get() { return null; },
    async put() { return {}; },
  };
  const service = new CloudflareInternalPublication(bucket, sites, token);
  const result = await service.activate({
    siteId,
    domain: "writing.example.com",
    expectedRelease: null,
    replacementRelease: releaseId,
    nextPublishAt: null,
  });

  assert.deepEqual(result, { changed: false });
  assert.equal(new URL(requests[0]!.url).pathname, "/publication/activate");
  assert.equal(requests[0]!.headers.get("authorization"), `Bearer ${token}`);
});

test("downstream activation failures are retryable host faults, not client validation errors", async () => {
  for (const code of [
    "release_activation_failed_503",
    "release_activation_response_invalid",
    "release_manifest_missing",
    `release_object_missing:${objectId}`,
  ]) {
    const publication = new Publication();
    publication.activate = async () => { throw new Error(code); };
    const response = await handleInternalPublicationRequest(new Request(
      `https://control.service.dev/internal/sites/${siteId}/releases/${releaseId}/activate`,
      {
        method: "POST",
        headers: headers({ "Content-Type": "application/json" }),
        body: JSON.stringify({
          domain: "writing.example.com",
          expected_release: null,
          next_publish_at: null,
        }),
      },
    ), publication, token);

    assert.equal(response.status, 502, code);
    assert.equal((await response.json() as { error: string }).error, code);
  }
});
