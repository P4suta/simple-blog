import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, readlinkSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve, sep, dirname } from 'node:path';

export function fingerprintInputs(root, names) {
  const base = resolve(root);
  return [...new Set(names)].sort().map(path => {
    const file = resolve(base, path);
    if (!file.startsWith(base + sep)) throw new Error('Source input escaped the repository');
    let metadata;
    try { metadata = lstatSync(file); } catch (error) {
      if (error.code === 'ENOENT') return { path, status: 'deleted', sha256: null };
      throw error;
    }
    if (!metadata.isFile() && !metadata.isSymbolicLink()) throw new Error('Source input is not a file');
    const bytes = metadata.isSymbolicLink() ? readlinkSync(file) : readFileSync(file);
    return { path, status: metadata.isSymbolicLink() ? 'symlink' : 'file', sha256: createHash('sha256').update(bytes).digest('hex') };
  });
}

export function snapshotInputs(root, inputs, destination) {
  const base = resolve(root), output = resolve(destination);
  for (const input of inputs) {
    if (input.status === 'deleted') continue;
    if (input.status !== 'file') throw new Error('Reproducible source snapshots require regular files');
    const source = resolve(base, input.path), target = resolve(output, input.path);
    if (!source.startsWith(base + sep) || !target.startsWith(output + sep)) throw new Error('Source snapshot escaped its directory');
    if (!lstatSync(source).isFile()) throw new Error('Source input changed type during capture');
    const bytes = readFileSync(source);
    if (createHash('sha256').update(bytes).digest('hex') !== input.sha256) throw new Error('Source changed during capture');
    mkdirSync(dirname(target), { recursive: true, mode: 0o700 });
    writeFileSync(target, bytes, { mode: 0o600, flag: 'wx' });
  }
}

export function assertInputsUnchanged(before, after) {
  if (JSON.stringify(before) !== JSON.stringify(after)) throw new Error('Verification inputs changed during execution');
}
