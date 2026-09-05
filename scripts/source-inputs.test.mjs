import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fingerprintInputs } from './source-inputs.mjs';
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
