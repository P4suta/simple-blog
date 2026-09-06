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
  const range = (offset, size) => {
    if (!Number.isSafeInteger(offset) || offset < 0 || size < 0 || offset + size > executable.length) throw new Error('Invalid PE range');
  };
  range(0, 64);
  if (executable.subarray(0, 2).toString() !== 'MZ') throw new Error('Expected a PE executable');
  const pe = executable.readUInt32LE(0x3c);
  range(pe, 24);
  if (executable.subarray(pe, pe + 4).toString() !== 'PE\0\0') throw new Error('Invalid PE signature');
  const sections = executable.readUInt16LE(pe + 6), optionalSize = executable.readUInt16LE(pe + 20), optional = pe + 24;
  range(optional, optionalSize);
  const magic = executable.readUInt16LE(optional), directories = magic === 0x20b ? 112 : magic === 0x10b ? 96 : 0;
  if (!directories || optionalSize < directories + 7 * 8 || sections < 1 || sections > 96) throw new Error('Invalid PE optional header');
  if (executable.readUInt32LE(optional + directories - 4) < 7) throw new Error('Missing PE debug directory');
  const table = optional + optionalSize;
  range(table, sections * 40);
  const fileOffset = (rva, size) => {
    for (let index = 0; index < sections; index++) {
      const section = table + index * 40, address = executable.readUInt32LE(section + 12), rawSize = executable.readUInt32LE(section + 16);
      if (rva >= address && rva - address + size <= rawSize) {
        const offset = executable.readUInt32LE(section + 20) + rva - address;
        range(offset, size);
        return offset;
      }
    }
    throw new Error('PE debug address is not backed by a section');
  };
  const entry = optional + directories + 6 * 8, debugSize = executable.readUInt32LE(entry + 4);
  if (!debugSize || debugSize % 28 !== 0) throw new Error('Invalid PE debug directory size');
  const debug = fileOffset(executable.readUInt32LE(entry), debugSize);
  const records = [];
  for (let offset = debug; offset < debug + debugSize; offset += 28) {
    if (executable.readUInt32LE(offset + 12) !== 2) continue;
    const size = executable.readUInt32LE(offset + 16), position = executable.readUInt32LE(offset + 24);
    if (size < 25 || size > 4096 || fileOffset(executable.readUInt32LE(offset + 20), size) !== position) throw new Error('Invalid CodeView location');
    if (executable.subarray(position, position + 4).toString() !== 'RSDS') throw new Error('Unsupported CodeView record');
    const record = executable.subarray(position, position + size), end = record.indexOf(0, 24);
    if (end < 0 || !record.subarray(24, end).toString().toLowerCase().endsWith('.pdb')) throw new Error('Invalid CodeView PDB name');
    records.push({ guid: record.subarray(4, 20).toString('hex'), age: record.readUInt32LE(20) });
  }
  const identity = pdbIdentity(pdb);
  if (records.length === 1 && records[0].guid === identity.guid && records[0].age === identity.age) return identity;
  throw new Error('The executable and PDB identities do not match');
}
