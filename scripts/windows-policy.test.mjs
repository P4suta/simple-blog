import { test } from 'node:test';
import assert from 'node:assert/strict';
import { win32 } from 'node:path';
import { discoverGitBash } from './windows-policy.mjs';

test('policy discovers custom Git installations and refuses a WSL-only launcher', () => {
  const files = new Set(['D:/Tools/Git/bin/bash.exe', 'D:/Tools/Git/bin/sh.exe', 'D:/Tools/Git/cmd/git.exe', 'C:/Windows/System32/bash.exe'].map(path => win32.normalize(path)));
  const exists = path => files.has(win32.normalize(path));
  const expected = win32.normalize('D:/Tools/Git/bin/bash.exe');
  assert.equal(discoverGitBash({ PATH: 'C:/Windows/System32;D:/Tools/Git/bin' }, exists), expected);
  assert.equal(discoverGitBash({ PATH: 'D:/Tools/Git/cmd' }, exists), expected);
  assert.equal(discoverGitBash({ PATH: 'C:/Windows/System32' }, exists), undefined);
  assert.equal(discoverGitBash({ VERIFY_BASH: expected }, exists), expected);
  assert.equal(discoverGitBash({ VERIFY_BASH: 'missing', PATH: 'D:/Tools/Git/bin' }, exists), undefined);
});
