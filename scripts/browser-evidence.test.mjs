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

test('existing inline server logs can be vetted and recovered without accepting raw error contexts', () => {
  const safe = { name: 'server events', body: Buffer.from('{"event":"synthetic"}').toString('base64') };
  const report = { specs: [{ tests: [{ results: [{ status: 'failed', attachments: [safe, { name: 'error context', body: 'private' }] }] }] }] };
  const [attachment] = summarizeBrowser(report).attachments;
  assert.deepEqual(JSON.parse(Buffer.from(attachment.body, 'base64').toString('utf8')), { event: 'synthetic' });
});

test('untrusted attachment fields and server payloads cannot carry arbitrary private values', () => {
  const id = 'a4134e5e-7044-4229-8e14-d7cf529b766c';
  const body = Buffer.from(JSON.stringify({ cookie: 'private-cookie', message: 'private-exception',
    fields: { event: 'http.request.failed', error_code: 'repository.storage', body_markdown: 'private-writing' },
    span: { name: 'http.request', request_id: id, path: '/private-path' } })).toString('base64');
  const report = { specs: [{ tests: [{ results: [{ attachments: [
    { name: 'server events', body, private: 'private-attachment-field' },
    { name: 'server events', path: 'events.log', private: 'private-path-field' },
  ] }] }] }] };
  const attachments = summarizeBrowser(report).attachments;
  assert.deepEqual(attachments[1], { name: 'server events', path: 'events.log' });
  const log = Buffer.from(attachments[0].body, 'base64').toString('utf8');
  assert.equal(log.includes('private-'), false);
  assert.equal(JSON.parse(log).span.request_id, id);
  assert.equal(JSON.parse(log).fields.error_code, 'repository.storage');
});
