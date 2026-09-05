import { test } from 'node:test';
import assert from 'node:assert/strict';
import { safeEvidenceText, assertSafeEvidence } from './evidence-security.mjs';
test('the share gate rejects private values, including an unknown synthetic canary', () => {
  for (const value of ['sb_session=private', 'sb_csrf=private', 'Bearer private', '/admin/share/private/', '?token=private', 'PRIVATE_FIXTURE_CANARY']) {
    assert.throws(() => assertSafeEvidence(value));
  }
  assert.doesNotThrow(() => assertSafeEvidence('/admin/share/{token}/ ?token=[redacted] Bearer [redacted]'));
});
test('export sanitizes classified secrets and refuses unclassified canaries', () => {
  assert.doesNotThrow(() => safeEvidenceText(JSON.stringify({ cookie: 'secret', url: '/admin/share/secret/?token=secret' })));
  assert.throws(() => safeEvidenceText('PRIVATE_FIXTURE_CANARY'));
});

test('pretty printed nested JSON cannot bypass classified field redaction', () => {
  const result = safeEvidenceText(JSON.stringify({ nested: { cookie: 'private-cookie', body_markdown: 'private-draft' } }, null, 2));
  assert.equal(result.includes('private-cookie'), false);
  assert.equal(result.includes('private-draft'), false);
});
