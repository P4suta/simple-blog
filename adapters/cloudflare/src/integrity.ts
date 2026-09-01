import type { R2ObjectBody } from "./bindings.ts";

const DIGEST = /^[0-9a-f]{64}$/;

export async function verifiedReleaseBytes(
  object: R2ObjectBody,
  kind: "manifest" | "object",
  blake3: string,
): Promise<Uint8Array> {
  const expectedSha256 = object.customMetadata?.["sha256"] ?? "";
  if (
    object.customMetadata?.["simple-blog-kind"] !== kind ||
    object.customMetadata?.["blake3"] !== blake3 ||
    !DIGEST.test(expectedSha256)
  ) {
    throw new Error(`release_${kind}_metadata_invalid`);
  }

  const bytes = new Uint8Array(await object.arrayBuffer());
  if (await sha256Hex(bytes) !== expectedSha256) {
    throw new Error(`release_${kind}_integrity_invalid`);
  }
  return bytes;
}

export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  const digest = await crypto.subtle.digest("SHA-256", copy.buffer);
  return [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}
