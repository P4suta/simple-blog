import { test } from 'node:test';
import assert from 'node:assert/strict';
import { summarizeBrowser } from './browser-evidence.mjs';

test('browser summaries preserve failures but omit private diagnostic arguments', () => {
  const report = { stats: { unexpected: 1, error: 'private-secret' }, errors: ['private-secret'], suites: [{ specs: [{ title: 'synthetic workflow',
    tests: [{ status: 'unexpected', expectedStatus: 'passed', results: [{ status: 'failed', error: 'fill(private-secret)',
      attachments: [{ name: 'error context', path: 'private-secret' }, { name: 'sanitized trace', path: 'trace.zip' }] }] }] }] }] };
  const result = summarizeBrowser(report);
  assert.equal(JSON.stringify(result).includes('private-secret'), false);
  assert.equal(result.summary.tests[0].results[0].status, 'failed');
  assert.deepEqual(result.attachments, [{ name: 'sanitized trace', path: 'trace.zip' }]);
});
