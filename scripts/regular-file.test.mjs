import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { readRegularFile } from './regular-file.mjs';

test('evidence reads the opened file and bounds growth after the size check', t => {
  const root = fs.mkdtempSync(join(tmpdir(), 'evidence-descriptor-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const file = join(root, 'input');
  fs.writeFileSync(file, 'original');
  assert.equal(readRegularFile(file).toString(), 'original');
  assert.throws(() => readRegularFile(file, 7), /size limit/);
  let injected = 0, bytesRead = 0;
  const io = { ...fs, readSync(fd, buffer, offset, length, position) {
    if (!injected++) fs.appendFileSync(file, 'growth');
    const read = fs.readSync(fd, buffer, offset, length, position);
    bytesRead += read;
    return read;
  } };
  assert.throws(() => readRegularFile(file, 8, io), /changed while reading/);
  assert.ok(injected > 0);
  assert.equal(bytesRead, 9, 'read at most the original size plus one byte');
});

test('evidence checks the actual opened descriptor and always closes it', t => {
  const root = fs.mkdtempSync(join(tmpdir(), 'evidence-descriptor-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const file = join(root, 'input');
  fs.writeFileSync(file, 'original');
  let opened, injected = false;
  const io = { ...fs, openSync(path, flags) {
    opened = fs.openSync(path, flags);
    return opened;
  }, fstatSync(fd) {
    injected = true;
    return { ...fs.fstatSync(fd), isFile: () => false };
  } };
  assert.throws(() => readRegularFile(file, 8, io), /not a regular file/);
  assert.equal(injected, true);
  assert.throws(() => fs.fstatSync(opened), { code: 'EBADF' });
});

test('replacement between inspection and open is detected before reading its contents', t => {
  const root = fs.mkdtempSync(join(tmpdir(), 'evidence-replacement-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const file = join(root, 'input');
  fs.writeFileSync(file, 'original');
  let injected = false, reads = 0;
  const io = { ...fs, openSync(path, flags) {
    fs.renameSync(path, join(root, 'retained'));
    fs.writeFileSync(path, 'PRIVATE_FIXTURE_CANARY');
    injected = true;
    return fs.openSync(path, flags);
  }, readSync(...args) { reads++; return fs.readSync(...args); } };
  assert.throws(() => readRegularFile(file, 64, io), /changed before opening/);
  assert.equal(injected, true);
  assert.equal(reads, 0);
});

test('Unix evidence refuses symlinks and real FIFOs without waiting for a writer', { skip: process.platform === 'win32' }, t => {
  const root = fs.mkdtempSync(join(tmpdir(), 'evidence-special-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(join(root, 'private'), 'PRIVATE_FIXTURE_CANARY');
  fs.symlinkSync('private', join(root, 'link'));
  assert.throws(() => readRegularFile(join(root, 'link')), /not a regular file/);
  assert.equal(spawnSync('mkfifo', [join(root, 'pipe')]).status, 0);
  const child = spawnSync(process.execPath, ['--input-type=module', '-e',
    `import { readRegularFile } from ${JSON.stringify(new URL('./regular-file.mjs', import.meta.url).href)}; try { readRegularFile(process.argv[1]); process.exit(2); } catch { process.exit(0); }`, join(root, 'pipe')], { timeout: 5000 });
  assert.equal(child.status, 0, child.stderr?.toString());
});
