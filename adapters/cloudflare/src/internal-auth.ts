/** Constant-work bearer comparison suitable for internal capability tokens. */
export async function authorizedBearer(request: Request, expected: string): Promise<boolean> {
  const header = request.headers.get("authorization");
  if (header === null || !header.startsWith("Bearer ")) return false;
  const presented = new TextEncoder().encode(header.slice("Bearer ".length));
  const secret = new TextEncoder().encode(expected);
  const [presentedHash, secretHash] = await Promise.all([
    crypto.subtle.digest("SHA-256", presented),
    crypto.subtle.digest("SHA-256", secret),
  ]);
  const left = new Uint8Array(presentedHash);
  const right = new Uint8Array(secretHash);
  let difference = presented.length ^ secret.length;
  for (let index = 0; index < left.length; index += 1) difference |= left[index]! ^ right[index]!;
  return difference === 0;
}
