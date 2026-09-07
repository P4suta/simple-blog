import fs from 'node:fs';

// Inspect and read the same descriptor. Nonblocking/no-follow flags prevent an
// input replaced by a FIFO or symlink from hanging or following a different file.
export function readRegularFile(path, maxBytes = 64 * 1024 * 1024, io = fs) {
  const expected = io.lstatSync(path, { bigint: true });
  if (!expected.isFile()) throw new Error('Evidence input is not a regular file');
  const flags = fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0) | (fs.constants.O_NONBLOCK ?? 0);
  const fd = io.openSync(path, flags);
  try {
    const before = io.fstatSync(fd, { bigint: true });
    if (!before.isFile()) throw new Error('Evidence input is not a regular file');
    if (before.dev !== expected.dev || before.ino !== expected.ino) throw new Error('Evidence input changed before opening');
    if (before.size > maxBytes) throw new Error('Evidence input exceeds its size limit');
    const size = Number(before.size);
    const chunks = [];
    let total = 0;
    while (total <= size) {
      const chunk = Buffer.alloc(Math.min(64 * 1024, size + 1 - total));
      const count = io.readSync(fd, chunk, 0, chunk.length, null);
      if (!count) break;
      total += count;
      chunks.push(chunk.subarray(0, count));
    }
    const after = io.fstatSync(fd, { bigint: true });
    if (total !== size || after.size !== before.size || after.mtimeNs !== before.mtimeNs || after.ctimeNs !== before.ctimeNs) {
      throw new Error('Evidence input changed while reading');
    }
    return Buffer.concat(chunks, total);
  } finally {
    io.closeSync(fd);
  }
}
