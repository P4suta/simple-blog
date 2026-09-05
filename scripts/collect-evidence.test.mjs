import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, existsSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
const collector = fileURLToPath(new URL('./collect-evidence.mjs', import.meta.url));
test('failed secret inspection cannot publish partial evidence', t => {
  const root = mkdtempSync(join(tmpdir(), 'simple-blog-evidence-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'target/verification'), { recursive: true });
  writeFileSync(join(root, 'target/verification/a.json'), '{"status":"failed"}');
  writeFileSync(join(root, 'target/verification/z.log'), 'PRIVATE_FIXTURE_CANARY');
  const result = spawnSync(process.execPath, [collector], { cwd: root, windowsHide: true });
  assert.notEqual(result.status, 0);
  assert.equal(existsSync(join(root, 'target/shareable')), false);
});
test('validated failure evidence is exported with integrity hashes', t => {
  const root = mkdtempSync(join(tmpdir(), 'simple-blog-evidence-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'target/verification'), { recursive: true });
  writeFileSync(join(root, 'target/verification/recovery.json'), '{"status":"timeout","cookie":"private"}');
  const result = spawnSync(process.execPath, [collector], { cwd: root, windowsHide: true });
  assert.equal(result.status, 0, result.stderr.toString());
  const index = JSON.parse(readFileSync(join(root, 'target/shareable/evidence.json')));
  assert.equal(index.files.length, 1);
  assert.equal(index.files[0].sha256.length, 64);
  assert.equal(JSON.parse(readFileSync(join(root, 'target/shareable/verification/recovery.json'))).cookie, '[redacted]');
});

test('private source snapshots and unclassified files beside symbols are never exported', t => {
  const root = mkdtempSync(join(tmpdir(), 'simple-blog-evidence-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const snapshots = join(root, 'target/verification/run/_source/browser-report');
  mkdirSync(snapshots, { recursive: true });
  writeFileSync(join(snapshots, 'results.json'), 'PRIVATE_FIXTURE_CANARY');
  const symbols = join(root, 'target/verification/symbols-release');
  mkdirSync(symbols, { recursive: true });
  const files = ['simple-blog', 'simple-blog.debug'].map(name => {
    const bytes = Buffer.from(name);
    writeFileSync(join(symbols, name), bytes);
    return { name, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
  });
  writeFileSync(join(symbols, 'symbols.json'), JSON.stringify({ schema: 1, files }));
  writeFileSync(join(symbols, 'private.txt'), 'PRIVATE_FIXTURE_CANARY');
  const result = spawnSync(process.execPath, [collector], { cwd: root, windowsHide: true });
  assert.equal(result.status, 0, result.stderr.toString());
  const index = JSON.parse(readFileSync(join(root, 'target/shareable/evidence.json')));
  assert.equal(index.files.length, 3);
  assert.equal(index.files.some(file => /private|_source/.test(file.path)), false);
});
