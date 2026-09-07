import { test } from 'node:test';
import assert from 'node:assert/strict';
import { verifyPeSymbols, pdbIdentity } from './symbol-identity.mjs';
function fixture() {
  const pdb = Buffer.alloc(512 * 5);
  pdb.write('Microsoft C/C++ MSF 7.00');
  pdb.writeUInt32LE(512, 32); pdb.writeUInt32LE(16, 44); pdb.writeUInt32LE(1, 52);
  pdb.writeUInt32LE(2, 512);
  pdb.writeUInt32LE(2, 1024); pdb.writeUInt32LE(0xffffffff, 1028); pdb.writeUInt32LE(28, 1032); pdb.writeUInt32LE(3, 1036);
  pdb.writeUInt32LE(7, 1536 + 8);
  const guid = Buffer.from('0123456789abcdef0123456789abcdef', 'hex'); guid.copy(pdb, 1536 + 12);
  const exe = Buffer.alloc(0x500); exe.write('MZ'); exe.writeUInt32LE(0x80, 0x3c); exe.write('PE\0\0', 0x80);
  exe.writeUInt16LE(0x8664, 0x84); exe.writeUInt16LE(1, 0x86); exe.writeUInt16LE(240, 0x94);
  const optional = 0x98, section = optional + 240;
  exe.writeUInt16LE(0x20b, optional); exe.writeUInt32LE(0x200, optional + 60); exe.writeUInt32LE(16, optional + 108);
  exe.writeUInt32LE(0x1000, optional + 112 + 6 * 8); exe.writeUInt32LE(28, optional + 112 + 6 * 8 + 4);
  exe.write('.rdata', section); exe.writeUInt32LE(0x200, section + 8); exe.writeUInt32LE(0x1000, section + 12);
  exe.writeUInt32LE(0x200, section + 16); exe.writeUInt32LE(0x200, section + 20);
  exe.writeUInt32LE(2, 0x200 + 12); exe.writeUInt32LE(40, 0x200 + 16);
  exe.writeUInt32LE(0x1040, 0x200 + 20); exe.writeUInt32LE(0x240, 0x200 + 24);
  exe.write('RSDS', 0x240); guid.copy(exe, 0x244);
  exe.writeUInt32LE(7, 0x254); exe.write('simple_blog.pdb', 0x258);
  return { pdb, exe };
}
test('symbol pairing checks compiler GUID and generation, not filenames', () => {
  const { pdb, exe } = fixture();
  assert.equal(verifyPeSymbols(exe, pdb).age, 7);
  exe.writeUInt32LE(8, 0x254);
  assert.throws(() => verifyPeSymbols(exe, pdb), /do not match/);
});
test('corrupt PDB directories and missing CodeView records fail closed', () => {
  const { pdb, exe } = fixture();
  pdb.writeUInt32LE(0xffffffff, 512);
  assert.throws(() => pdbIdentity(pdb));
  exe.fill(0, 0x240);
  assert.throws(() => verifyPeSymbols(exe, fixture().pdb));
});

test('a matching RSDS string outside the PE debug directory is never an identity', () => {
  const { pdb, exe } = fixture();
  exe.copy(exe, 0x300, 0x240, 0x268);
  exe.writeUInt32LE(8, 0x254);
  assert.throws(() => verifyPeSymbols(exe, pdb));
  const missingPe = fixture();
  missingPe.exe.fill(0, 0x80, 0x84);
  assert.throws(() => verifyPeSymbols(missingPe.exe, missingPe.pdb));
});
