import { test } from 'node:test';
import assert from 'node:assert/strict';
import { zipSync, unzipSync, strToU8, strFromU8 } from 'fflate';
import { sanitizeTrace } from './trace-artifacts.mjs';

test('trace export removes header pairs, credentials in DOM snapshots and bodies', () => {
  const zip = zipSync({ 'test.trace': strToU8(JSON.stringify({headers:[{name:'cookie',value:'session-secret'}],
    postData:{text:'code=recovery-secret'},snapshot:'<input value="recovery-secret">'})+'\n'),
    'resources/body':strToU8('{"recovery_codes":["another-secret"]}'),
    'resources/image.png':new Uint8Array([1,2,3]) });
  const files = unzipSync(sanitizeTrace(zip, ['recovery-secret']));
  const output = Object.values(files).map(strFromU8).join('\n');
  for (const value of ['session-secret','recovery-secret','another-secret']) assert.equal(output.includes(value), false);
  assert.equal(files['resources/image.png'], undefined, 'opaque resources are not shareable evidence');
});

test('malformed trace metadata fails closed instead of exporting unknown material', () => {
  assert.throws(() => sanitizeTrace(zipSync({'bad.trace':strToU8('not json')}), []));
});

test('secrets used as snapshot object keys are redacted too', () => {
  const files = unzipSync(sanitizeTrace(zipSync({'keys.trace':strToU8('{"fixture-secret":"value"}\n')}), ['fixture-secret']));
  assert.equal(strFromU8(files['keys.trace']).includes('fixture-secret'), false);
});
