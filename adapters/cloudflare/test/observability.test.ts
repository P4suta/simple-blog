import assert from "node:assert/strict";
import test from "node:test";
import worker from "../src/index.ts";
import type { WorkerEnv } from "../src/bindings.ts";

test("Worker failures retain correlation without leaking paths or exception messages", async () => {
  const records: string[] = [];
  const saved = { log: console.log, warn: console.warn, error: console.error };
  let response: Response;
  try {
    console.log = console.warn = console.error = (...args) => { records.push(args.join(" ")); };
    const env = {
      get CONTROL_HOSTNAME(): string { throw new Error("synthetic-private-exception"); },
    } as WorkerEnv;
    response = await worker.fetch(new Request("https://site.example/admin/share/synthetic-capability/?token=synthetic-query", {
      method: "synthetic-private-method", body: "synthetic-private-body",
      headers: { Cookie: "synthetic-cookie", "X-Request-Id": "synthetic-client-id" },
    }), env, { waitUntil() {} });
  } finally {
    Object.assign(console, saved);
  }
  assert.equal(response.status, 500);
  const output = records.join("\n");
  for (const secret of ["synthetic-private-exception", "synthetic-capability", "synthetic-query", "synthetic-cookie", "synthetic-client-id", "synthetic-private-method", "synthetic-private-body"]) {
    assert.equal(output.includes(secret), false, `trace leaked ${secret}`);
  }
  const events = records.map((record) => JSON.parse(record));
  assert.ok(events.every((event) => event.request_id === response.headers.get("x-request-id")));
  assert.ok(events.some((event) => event.error_code === "worker.internal"));
});
