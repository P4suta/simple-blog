import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fingerprintInputs, snapshotInputs, assertInputsUnchanged } from './source-inputs.mjs';
test('reproducibility covers untracked inputs and legitimate tracked deletions', t => {
  const root = mkdtempSync(join(tmpdir(), 'simple-blog-inputs-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, 'new.rs'), 'first');
  const before = fingerprintInputs(root, ['new.rs', 'deleted.rs']);
  assert.equal(before[0].status, 'deleted');
  writeFileSync(join(root, 'new.rs'), 'changed');
  assert.notEqual(before[1].sha256, fingerprintInputs(root, ['new.rs'])[0].sha256);
  assert.throws(() => fingerprintInputs(root, ['../outside']));
});

test('the original source remains reproducible and input drift cannot be reported as a clean run', t => {
  const root = mkdtempSync(join(tmpdir(), 'simple-blog-snapshot-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, 'source.rs'), 'original source');
  const before = fingerprintInputs(root, ['source.rs']);
  snapshotInputs(root, before, join(root, 'snapshot'));
  assertInputsUnchanged(before, fingerprintInputs(root, ['source.rs']));
  writeFileSync(join(root, 'source.rs'), 'changed during test');
  assert.equal(readFileSync(join(root, 'snapshot/source.rs'), 'utf8'), 'original source');
  assert.throws(() => assertInputsUnchanged(before, fingerprintInputs(root, ['source.rs'])));
  assert.throws(() => snapshotInputs(root, before, join(root, 'late-snapshot')));
});
