/** Reads at most `maximum` payload bytes and cancels at the first excess chunk. */
export async function boundedBytes(
  request: Request,
  maximum: number,
  invalidLengthCode: string,
  tooLargeCode: string,
): Promise<Uint8Array> {
  const declared = request.headers.get("content-length");
  let declaredLength: number | null = null;
  if (declared !== null) {
    if (!/^[0-9]+$/.test(declared)) throw new Error(invalidLengthCode);
    const value = Number(declared);
    if (!Number.isSafeInteger(value) || value < 0) throw new Error(invalidLengthCode);
    if (value > maximum) throw new Error(tooLargeCode);
    declaredLength = value;
  }
  if (request.body === null) return new Uint8Array();

  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value.byteLength > maximum - total) {
        await cancelWithoutMasking(reader, tooLargeCode);
        throw new Error(tooLargeCode);
      }
      if (declaredLength !== null && value.byteLength > declaredLength - total) {
        await cancelWithoutMasking(reader, invalidLengthCode);
        throw new Error(invalidLengthCode);
      }
      const copy = new Uint8Array(value.byteLength);
      copy.set(value);
      chunks.push(copy);
      total += copy.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  if (declaredLength !== null && total !== declaredLength) throw new Error(invalidLengthCode);

  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

async function cancelWithoutMasking(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  reason: string,
): Promise<void> {
  try {
    await reader.cancel(reason);
  } catch {
    // The stable payload error remains more useful than an adapter-specific
    // cancellation failure after the body has already been rejected.
  }
}
