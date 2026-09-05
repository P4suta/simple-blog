import { sanitizeText } from './runner.mjs';

export function safeEvidenceText(text) {
  // Parse complete JSON before treating output as JSONL. Pretty printed objects
  // otherwise bypass field redaction when each line is parsed separately.
  let sanitized;
  try { JSON.parse(text); sanitized = sanitizeText(text); }
  catch { sanitized = text.split('\n').map(sanitizeText).join('\n'); }
  assertSafeEvidence(sanitized);
  return sanitized;
}

export function assertSafeEvidence(text) {
  for (const pattern of [
    /PRIVATE_FIXTURE_CANARY/,
    /\bsb_(?:session|csrf)=(?!\[redacted\])[^;\s"<>]+/i,
    /\bBearer\s+(?!\[redacted\])[A-Za-z0-9._~+/-]+/i,
    /\/admin\/share\/(?!\[redacted\]|\{token\})[^/\s?"<>]+/,
    /[?&](?:token|csrf|code|claim|secret)=(?!\[redacted\])[^&\s"<>]+/i,
  ]) if (pattern.test(text)) throw new Error('Evidence contains a private value; export refused');
}
