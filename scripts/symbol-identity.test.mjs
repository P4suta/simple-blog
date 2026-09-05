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
  const exe = Buffer.alloc(128); exe.write('MZ'); exe.write('RSDS', 40); guid.copy(exe, 44);
  exe.writeUInt32LE(7, 60); exe.write('simple_blog.pdb', 64);
  return { pdb, exe };
}
test('symbol pairing checks compiler GUID and generation, not filenames', () => {
  const { pdb, exe } = fixture();
  assert.equal(verifyPeSymbols(exe, pdb).age, 7);
  exe.writeUInt32LE(8, 60);
  assert.throws(() => verifyPeSymbols(exe, pdb), /do not match/);
});
test('corrupt PDB directories and missing CodeView records fail closed', () => {
  const { pdb, exe } = fixture();
  pdb.writeUInt32LE(0xffffffff, 512);
  assert.throws(() => pdbIdentity(pdb));
  exe.fill(0, 40);
  assert.throws(() => verifyPeSymbols(exe, fixture().pdb));
});
