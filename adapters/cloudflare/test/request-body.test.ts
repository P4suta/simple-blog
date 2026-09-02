import assert from "node:assert/strict";
import test from "node:test";

import { boundedBytes } from "../src/request-body.ts";

function streamedRequest(chunks: number[], headers?: HeadersInit, cancel?: () => void): Request {
  let index = 0;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      const length = chunks[index];
      index += 1;
      if (length === undefined) {
        controller.close();
      } else {
        controller.enqueue(new Uint8Array(length));
      }
    },
    cancel,
  });
  return new Request("https://control.service.dev/internal", {
    method: "POST",
    headers,
    body,
    duplex: "half",
  } as RequestInit & { duplex: "half" });
}

test("bounded request reads reject mismatched declared lengths", async () => {
  await assert.rejects(
    boundedBytes(
      streamedRequest([2], { "content-length": "1" }),
      10,
      "invalid_length",
      "too_large",
    ),
    { message: "invalid_length" },
  );
  await assert.rejects(
    boundedBytes(
      streamedRequest([1], { "content-length": "2" }),
      10,
      "invalid_length",
      "too_large",
    ),
    { message: "invalid_length" },
  );
});

test("an absent request body must agree with its declared length", async () => {
  const absent = new Request("https://control.service.dev/internal", {
    method: "POST",
    headers: { "content-length": "1" },
  });
  await assert.rejects(
    boundedBytes(absent, 10, "invalid_length", "too_large"),
    { message: "invalid_length" },
  );

  const empty = new Request("https://control.service.dev/internal", {
    method: "POST",
    headers: { "content-length": "0" },
  });
  assert.deepEqual(
    await boundedBytes(empty, 10, "invalid_length", "too_large"),
    new Uint8Array(),
  );
});

test("stream cancellation failures cannot mask the stable oversized-body error", async () => {
  let cancelled = false;
  const request = streamedRequest([700, 700], undefined, () => {
    cancelled = true;
    throw new Error("adapter cancellation failed");
  });

  await assert.rejects(
    boundedBytes(request, 1024, "invalid_length", "too_large"),
    { message: "too_large" },
  );
  assert.equal(cancelled, true);
});
