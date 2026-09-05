import { unzipSync, zipSync, strFromU8, strToU8 } from 'fflate';
import { sanitizeText } from './runner.mjs';

const confidential = /^(?:cookie|set-cookie|authorization|csrf|csrf_token|token|setup_token|recovery_codes?|password|secret|postData|requestBody|responseBody|storageState)$/i;
export function sanitizeTrace(bytes, secrets = []) {
  const clean = value => {
    if (Array.isArray(value)) return value.map(clean);
    if (value && typeof value === 'object') {
      if (typeof value.name === 'string' && confidential.test(value.name)) return { ...value, value: '[redacted]' };
      return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, confidential.test(key) ? '[redacted]' : clean(item)]));
    }
    if (typeof value !== 'string') return value;
    let text = sanitizeText(value);
    for (const secret of secrets) if (secret) text = text.replaceAll(secret, '[redacted]');
    return text;
  };
  const safe = {};
  for (const [name, content] of Object.entries(unzipSync(bytes))) {
    if (name.endsWith('.trace') || name.endsWith('.network') || name.endsWith('.stacks')) {
      const lines = strFromU8(content).split('\n').filter(line => line.trim());
      safe[name] = strToU8(lines.map(line => JSON.stringify(clean(JSON.parse(line)))).join('\n') + '\n');
    } else if (name.startsWith('resources/')) {
      // Response bodies and screenshots may contain credentials even without a
      // recognizable key. Keep JSON after sanitization; omit opaque resources.
      try { safe[name] = strToU8(JSON.stringify(clean(JSON.parse(strFromU8(content))))); } catch { /* omitted */ }
    }
  }
  for (const [name, content] of Object.entries(safe)) {
    let text = strFromU8(content);
    // Snapshots can use DOM values as object keys. Also sanitize the encoded
    // representation so keys cannot bypass value-oriented redaction.
    for (const secret of secrets) if (secret) text = text.replaceAll(JSON.stringify(secret).slice(1, -1), '[redacted]');
    if (secrets.some(secret => secret && text.includes(secret))) throw new Error('trace contains a fixture secret');
    safe[name] = strToU8(text);
  }
  return zipSync(safe);
}
