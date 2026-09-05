import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runStep, sanitizeText, writeJson } from './runner.mjs';

async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'verification-test-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

test('a failed child preserves prior output, exit code and exact invocation', async t => {
  const outputDirectory = await fixture(t);
  const args = ['-e', 'console.log("evidence-before-failure");process.exit(7)'];
  const result = await runStep({ name: 'failure', command: process.execPath, args, outputDirectory });
  assert.equal(result.status, 'failed');
  assert.equal(result.exitCode, 7);
  assert.deepEqual(result.args, args);
  assert.match(await readFile(join(outputDirectory, 'failure.stdout.log'), 'utf8'), /evidence-before-failure/);
  assert.equal(JSON.parse(await readFile(join(outputDirectory, '_steps', 'failure.json'))).status, 'failed');
});

test('timeout is distinct from success and retains output', async t => {
  const outputDirectory = await fixture(t);
  const result = await runStep({ name: 'timeout', command: process.execPath,
    args: ['-e', 'console.log("started");setInterval(()=>{},100)'], timeoutMs: 1500, outputDirectory });
  assert.equal(result.status, 'timeout');
  assert.match(await readFile(join(outputDirectory, 'timeout.stdout.log'), 'utf8'), /started/);
});

test('an unavailable executable is recorded as not run, never successful', async t => {
  const result = await runStep({ name: 'missing', command: 'nonexistent-simple-blog-tool',
    args: [], outputDirectory: await fixture(t) });
  assert.equal(result.status, 'not_run');
  assert.equal(result.exitCode, null);
});

test('evidence storage failure is fatal before executing the child', async t => {
  const directory = await fixture(t);
  const outputDirectory = join(directory, 'not-a-directory');
  await writeFile(outputDirectory, 'occupied');
  await assert.rejects(runStep({ name: 'evidence', command: process.execPath,
    args: ['-e', 'process.exit(0)'], outputDirectory }));
});

test('evidence redacts capability URLs and sensitive structured fields', () => {
  const source = JSON.stringify({ url: 'https://example.test/admin/share/private-capability/?token=query-secret',
    cookie: 'session-secret', authorization: 'Bearer auth-secret', body_markdown: 'private writing' });
  const redacted = sanitizeText(source);
  for (const value of ['private-capability', 'query-secret', 'session-secret', 'auth-secret', 'private writing']) {
    assert.equal(redacted.includes(value), false);
  }
});

test('a write failure after child startup is evidence_failed even when the child exits zero', async t => {
  let fired = 0;
  const result = await runStep({ name: 'mid-write', command: process.execPath,
    args: ['-e', 'console.log("must be retained");'], outputDirectory: await fixture(t),
    writeEvidence() { fired++; throw new Error('synthetic disk full'); } });
  assert.ok(fired > 0, 'the injected failure must fire');
  assert.equal(result.status, 'evidence_failed');
  assert.equal(result.errorCode, 'evidence.write_failed');
});

test('a report failure after spawn terminates the child and cannot become success', async t => {
  let writes = 0;
  const result = await runStep({ name: 'pid-write', command: process.execPath,
    args: ['-e', 'setInterval(()=>{},100)'], outputDirectory: await fixture(t), timeoutMs: 2000,
    writeReport(path, value) { if (++writes === 2) throw new Error('synthetic PID report failure'); writeJson(path, value); } });
  assert.equal(writes, 3);
  assert.equal(result.status, 'evidence_failed');
  assert.throws(() => process.kill(result.pid, 0), { code: 'ESRCH' });
});

test('a child artifact named after its step survives the final status report', async t => {
  const outputDirectory = await fixture(t);
  const result = await runStep({ name: 'measurement', command: process.execPath,
    args: ['-e', 'require("node:fs").writeFileSync(process.argv[1],JSON.stringify({measured:42}))', join(outputDirectory, 'measurement.json')], outputDirectory });
  assert.equal(result.status, 'passed');
  assert.deepEqual(JSON.parse(await readFile(join(outputDirectory, 'measurement.json'))), { measured: 42 });
  assert.equal(JSON.parse(await readFile(join(outputDirectory, '_steps', 'measurement.json'))).status, 'passed');
});
