import { mkdtempSync, statSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
const binary = resolve(`target/verification/symbols-release/simple-blog${process.platform === 'win32' ? '.exe' : ''}`);
if (statSync(binary).size > 41943040) throw new Error('Release binary exceeds the existing 40 MiB budget');
const data = mkdtempSync(join(tmpdir(), 'simple-blog-release-'));
try {
  // Child commands receive no development-time Node/Bun configuration.
  const env = process.platform === 'win32' ? { SystemRoot: process.env.SystemRoot, TEMP: process.env.TEMP, PATH: `${process.env.SystemRoot}/System32` }
    : { PATH: '/usr/bin:/bin' };
  for (const args of [['init'], ['doctor', '--json']]) {
    const result = spawnSync(binary, ['--data-dir', data, ...args], { env, encoding: 'utf8', windowsHide: true, timeout: 30_000 });
    if (result.status !== 0) throw new Error(`Release ${args[0]} failed (${result.status})`);
    if (args[0] === 'doctor' && JSON.parse(result.stdout).healthy !== true) throw new Error('Release doctor was not healthy');
  }
  console.log('Release size, initialization and read-only diagnosis passed');
} finally { rmSync(data, { recursive: true, force: true, maxRetries: 8, retryDelay: 150 }); }
