import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, copyFileSync } from 'node:fs';
import { join } from 'node:path';
import { resolve } from 'node:path';
import { writeJson } from './runner.mjs';
import { verifyPeSymbols } from './symbol-identity.mjs';
import { exportLinuxSymbols } from './linux-symbols.mjs';

const profile = process.argv[2] ?? 'release';
if (!['debug', 'release'].includes(profile)) throw new Error('Expected debug or release profile');
const directory = `target/${profile}`;
const executable = join(directory, `simple-blog${process.platform === 'win32' ? '.exe' : ''}`);
const output = `target/verification/symbols-${profile}`;
mkdirSync(output, { recursive: true });
let files = [executable];
let identity;
if (process.platform === 'win32') {
  for (const file of [join(directory, 'simple_blog.pdb')]) {
    if (!existsSync(file)) throw new Error(`Required symbol artifact is missing: ${file}`);
    files.push(file);
    identity = verifyPeSymbols(readFileSync(executable), readFileSync(file));
  }
} else if (process.platform === 'linux') {
  files = exportLinuxSymbols(executable, output);
} else {
  throw new Error('Release symbol export currently requires Windows or Linux');
}
writeJson(join(output, 'symbols.json'), { schema: 1, profile, identity, files: files.map(path => {
  const bytes = readFileSync(path);
  const name = path.split(/[\\/]/).pop();
  if (resolve(path) !== resolve(output, name)) copyFileSync(path, join(output, name));
  return { name, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
}) });
