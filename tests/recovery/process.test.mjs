import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, readFile, writeFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { createHash } from 'node:crypto';

const executable = path.resolve(`target/debug/simple-blog${process.platform === 'win32' ? '.exe' : ''}`);
const fixture = path.resolve(`target/debug/examples/verification_fixture${process.platform === 'win32' ? '.exe' : ''}`);
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
function command(data, ...args) {
  const result = spawnSync(executable, ['--data-dir', data, ...args], { windowsHide: true, encoding: 'utf8', timeout: 30_000 });
  assert.equal(result.status, 0, `Command ${args[0]} failed with ${result.status}`);
  return result.stdout;
}
function seed(data) {
  const result = spawnSync(fixture, ['seed', data, 'http://localhost:8080'], { windowsHide: true, encoding: 'utf8', timeout: 30_000 });
  assert.equal(result.status, 0, 'Synthetic fixture must be created');
}
function database(data, action) {
  const connection = new DatabaseSync(path.join(data, 'simple-blog.sqlite3'));
  try { return action(connection); } finally { connection.close(); }
}
function change(data) {
  database(data, db => db.exec("UPDATE contents SET title='Changed before interruption', version=version+1 WHERE id=1; UPDATE publication_state SET public_revision=public_revision+1"));
}
function title(data) { return database(data, db => db.prepare('SELECT title FROM contents WHERE id=1').get().title); }
async function directory(t) {
  const root = await mkdtemp(path.join(tmpdir(), 'simple-blog-recovery-'));
  t.after(() => rm(root, { recursive: true, force: true, maxRetries: 8, retryDelay: 150 }));
  return root;
}
async function interrupt(root, data, event, ...args) {
  const marker = path.join(root, 'fault-reached.json');
  await rm(marker, { force: true });
  await writeFile(path.join(root, 'fault.json'), JSON.stringify({ event, mode: 'pause' }));
  const child = spawn(fixture, ['run', root, '--data-dir', data, ...args], { windowsHide: true, stdio: ['ignore', 'ignore', 'ignore'] });
  const exited = new Promise(resolve => { child.once('exit', resolve); child.once('error', resolve); });
  let reached;
  try {
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline && child.exitCode === null) {
      try { reached = JSON.parse(await readFile(marker, 'utf8')); break; } catch { await wait(20); }
    }
  } finally { child.kill('SIGKILL'); await exited; }
  assert.equal(reached?.event, event, 'the durable boundary must be reached before killing the process');
}

for (const operation of ['restore', 'portable']) for (const phase of ['prepared', 'previous_retained', 'installed']) {
  test(`${operation}: process death after ${phase} recovers a complete installation and permits another replacement`, { timeout: 90_000 }, async t => {
    const root = await directory(t), source = path.join(root, 'source'), destination = path.join(root, 'site');
    seed(source); seed(destination); change(source);
    const archive = path.join(root, operation === 'restore' ? 'backup.tar.zst' : 'site.simple-blog');
    if (operation === 'restore') command(source, 'backup', '--output', archive);
    else command(source, 'migrate', 'export', '--output', archive);
    const args = operation === 'restore' ? ['restore', archive, '--force'] : ['migrate', 'import', archive, '--force'];
    await interrupt(root, destination, `installation.activation.${phase}`, ...args);
    const before = await readdir(root);
    const diagnosis = spawnSync(executable, ['--data-dir', destination, 'doctor', '--json'], { windowsHide: true, encoding: 'utf8', timeout: 30_000 });
    assert.notEqual(diagnosis.status, 0);
    assert.equal(JSON.parse(diagnosis.stdout).healthy, false);
    assert.deepEqual(await readdir(root), before, 'doctor must not recover the interrupted installation');
    command(destination, 'build');
    assert.equal(title(destination), phase === 'installed' ? 'Changed before interruption' : 'Synthetic article 0');
    command(destination, 'build');
    command(destination, ...args);
    assert.equal(title(destination), 'Changed before interruption');
    assert.equal(JSON.parse(command(destination, 'doctor', '--json')).healthy, true);
    assert.ok((await readdir(root)).some(name => name.startsWith('.simple-blog-previous-')), 'previous data remains recoverable');
  });
}

test('interrupted backup preserves the old archive and can be repeated', { timeout: 90_000 }, async t => {
  const root = await directory(t), data = path.join(root, 'site'), archive = path.join(root, 'backup.tar.zst');
  seed(data); command(data, 'backup', '--output', archive);
  const digest = bytes => createHash('sha256').update(bytes).digest('hex');
  const previous = digest(await readFile(archive));
  change(data);
  const nextArchive = path.join(root, 'next-backup.tar.zst');
  await interrupt(root, data, 'backup.archive.synchronized', 'backup', '--output', nextArchive);
  assert.equal(digest(await readFile(archive)), previous);
  assert.ok((await readdir(root)).some(name => name.includes('.partial-')));
  command(data, 'backup', '--output', nextArchive);
  const restored = path.join(root, 'restored');
  command(restored, 'restore', nextArchive);
  assert.equal(title(restored), 'Changed before interruption');
});

test('interrupted migration retains a verified safety backup and reopens safely', { timeout: 90_000 }, async t => {
  const root = await directory(t), data = path.join(root, 'site');
  seed(data);
  database(data, db => db.exec('DROP TABLE media_variants; DELETE FROM _sqlx_migrations WHERE version=2'));
  await interrupt(root, data, 'database.migration.backup_created', 'build');
  assert.ok((await readdir(path.join(data, 'backups'))).some(name => name.startsWith('simple-blog-pre-migration-')));
  assert.equal(database(data, db => db.prepare('SELECT COUNT(*) AS n FROM _sqlx_migrations WHERE version=2').get().n), 0);
  command(data, 'build'); command(data, 'build');
  assert.equal(JSON.parse(command(data, 'doctor', '--json')).healthy, true);
});

for (const boundary of ['objects_stored', 'manifest_stored']) test(`interrupted publication after ${boundary} keeps the active release and repeats safely`, { timeout: 90_000 }, async t => {
  const root = await directory(t), data = path.join(root, 'site');
  seed(data);
  const previous = await readFile(path.join(data, 'releases/active'), 'utf8');
  change(data);
  await interrupt(root, data, `release.publish.${boundary}`, 'build');
  assert.equal(await readFile(path.join(data, 'releases/active'), 'utf8'), previous);
  command(data, 'build');
  assert.notEqual(await readFile(path.join(data, 'releases/active'), 'utf8'), previous);
  assert.equal(JSON.parse(command(data, 'doctor', '--json')).healthy, true);
});
