export function pdbIdentity(bytes) {
  if (!bytes.subarray(0, 24).toString().startsWith('Microsoft C/C++ MSF 7.00')) throw new Error('Unsupported PDB format');
  const blockSize = bytes.readUInt32LE(32), directorySize = bytes.readUInt32LE(44), map = bytes.readUInt32LE(52);
  if (![512, 1024, 2048, 4096, 8192].includes(blockSize) || directorySize > bytes.length) throw new Error('Invalid PDB directory');
  const blocks = Math.ceil(directorySize / blockSize);
  const pieces = [];
  for (let index = 0; index < blocks; index++) {
    const block = bytes.readUInt32LE(map * blockSize + index * 4);
    if ((block + 1) * blockSize > bytes.length) throw new Error('Invalid PDB block');
    pieces.push(bytes.subarray(block * blockSize, (block + 1) * blockSize));
  }
  const directory = Buffer.concat(pieces).subarray(0, directorySize);
  const count = directory.readUInt32LE(0);
  if (count < 2 || count > directory.length / 4) throw new Error('Missing PDB information stream');
  const firstSize = directory.readUInt32LE(4), infoSize = directory.readUInt32LE(8);
  const firstBlocks = firstSize === 0xffffffff ? 0 : Math.ceil(firstSize / blockSize);
  const block = directory.readUInt32LE(4 + count * 4 + firstBlocks * 4);
  if (infoSize < 28 || (block + 1) * blockSize > bytes.length) throw new Error('Invalid PDB information stream');
  const info = bytes.subarray(block * blockSize, (block + 1) * blockSize);
  return { guid: info.subarray(12, 28).toString('hex'), age: info.readUInt32LE(8) };
}

export function verifyPeSymbols(executable, pdb) {
  if (executable.subarray(0, 2).toString() !== 'MZ') throw new Error('Expected a PE executable');
  const identity = pdbIdentity(pdb);
  // RSDS records contain the compiler's exact GUID and generation. Requiring a
  // matching PDB suffix prevents a bare string constant from matching a record.
  for (let position = executable.indexOf('RSDS'); position >= 0; position = executable.indexOf('RSDS', position + 4)) {
    if (position + 24 >= executable.length) continue;
    const end = executable.indexOf(0, position + 24);
    if (end < 0 || end - position > 1024 || !executable.subarray(position + 24, end).toString().toLowerCase().endsWith('.pdb')) continue;
    if (executable.subarray(position + 4, position + 20).toString('hex') === identity.guid && executable.readUInt32LE(position + 20) === identity.age) return identity;
  }
  throw new Error('The executable and PDB identities do not match');
}
