import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, renameSync, rmSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { publishEvidence } from './publish-evidence.mjs';

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'evidence-publish-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const staging = join(root, 'staging'), destination = join(root, 'published');
  for (const [path, value] of [[staging, 'new'], [destination, 'old']]) {
    mkdirSync(path); writeFileSync(join(path, 'evidence.json'), value);
  }
  return { root, staging, destination };
}
test('failed publication restores the old export and preserves the new evidence', t => {
  const { staging, destination } = fixture(t);
  let calls = 0;
  assert.throws(() => publishEvidence(staging, destination, (from, to) => {
    if (++calls === 2) throw new Error('synthetic rename failure');
    renameSync(from, to);
  }));
  assert.equal(calls, 3, 'failure and rollback must execute');
  assert.equal(readFileSync(join(destination, 'evidence.json'), 'utf8'), 'old');
  assert.equal(readFileSync(join(staging, 'evidence.json'), 'utf8'), 'new');
});
test('cleanup failure leaves the new export and previous evidence available', t => {
  const { root, staging, destination } = fixture(t);
  assert.throws(() => publishEvidence(staging, destination, renameSync, () => { throw new Error('synthetic cleanup failure'); }));
  assert.equal(readFileSync(join(destination, 'evidence.json'), 'utf8'), 'new');
  const previous = readdirSync(root).find(name => name.startsWith('.shareable-previous-'));
  assert.equal(readFileSync(join(root, previous, 'evidence.json'), 'utf8'), 'old');
});
