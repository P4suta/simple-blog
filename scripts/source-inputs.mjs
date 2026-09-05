import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, readlinkSync } from 'node:fs';
import { resolve, sep } from 'node:path';

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
