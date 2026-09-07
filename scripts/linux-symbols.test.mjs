import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { exportLinuxSymbols } from './linux-symbols.mjs';

test('symbol export retries always read the untouched original', t => {
  const directory = mkdtempSync(join(tmpdir(), 'symbols-export-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const original = join(directory, 'original');
  const output = join(directory, 'output');
  mkdirSync(output);
  writeFileSync(original, 'code+debug');
  let calls = 0;
  const invoke = args => {
    calls++;
    if (args[0] === '--only-keep-debug') {
      assert.equal(readFileSync(args[1], 'utf8'), 'code+debug');
      writeFileSync(args[2], 'debug');
    } else if (args[0] === '--strip-debug') writeFileSync(args[1], 'code');
    return { status: 0 };
  };
  for (let attempt = 0; attempt < 2; attempt++) {
    exportLinuxSymbols(original, output, invoke);
    assert.equal(readFileSync(original, 'utf8'), 'code+debug');
    assert.equal(readFileSync(join(output, 'simple-blog.debug'), 'utf8'), 'debug');
  }
  assert.equal(calls, 6);
  assert.throws(() => exportLinuxSymbols(original, output, () => ({ status: 1 })));
  assert.equal(readFileSync(original, 'utf8'), 'code+debug');
});
