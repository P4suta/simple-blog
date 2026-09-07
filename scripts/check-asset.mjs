import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
const directory = mkdtempSync(join(tmpdir(), 'simple-blog-asset-'));
try {
  const output = join(directory, 'admin.js');
  const result = spawnSync(process.platform === 'win32' ? 'bun.exe' : 'bun',
    ['build', 'frontend/admin.ts', '--format=iife', '--minify', '--target=browser', `--outfile=${output}`],
    { stdio: 'inherit', windowsHide: true, timeout: 60_000 });
  if (result.status !== 0) throw new Error('Asset rebuild failed');
  if (!readFileSync(output).equals(readFileSync('static/admin.js'))) throw new Error('static/admin.js is stale; run bun run build:admin');
} finally { rmSync(directory, { recursive: true, force: true }); }
