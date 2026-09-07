import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, copyFileSync, rmSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fingerprintInputs } from './source-inputs.mjs';

const root = process.cwd();
const listing = spawnSync('git', ['ls-files', '-co', '--exclude-standard', '-z'], { encoding: 'utf8', windowsHide: true });
if (listing.status !== 0) throw new Error('Cannot enumerate source for secret inspection');
const snapshot = mkdtempSync(join(tmpdir(), 'simple-blog-source-scan-'));
try {
  for (const input of fingerprintInputs(root, listing.stdout.split('\0').filter(Boolean))) {
    if (input.status === 'deleted') continue;
    if (input.status !== 'file') throw new Error('Source secret scan refuses symlinks');
    const destination = join(snapshot, input.path);
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(join(root, input.path), destination);
  }
  for (const args of [['git', '--no-banner', '--redact', root], ['dir', '--no-banner', '--redact', snapshot]]) {
    const result = spawnSync('gitleaks', args, { stdio: 'inherit', windowsHide: true, timeout: 120_000 });
    if (result.status !== 0) process.exitCode = 1;
  }
} finally { rmSync(snapshot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); }
