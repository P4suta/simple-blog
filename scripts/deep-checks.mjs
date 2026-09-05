import { spawnSync } from 'node:child_process';
import { readFileSync, mkdirSync, existsSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { randomUUID, createHash } from 'node:crypto';
import { runStep, writeJson } from './runner.mjs';
import { assertSafeEvidence } from './evidence-security.mjs';

export const parsers = {
  search: ['src/domain/search.rs', 'src/domain/content.rs'],
  release_manifest: ['src/release.rs'],
  portable_site: ['src/portable.rs'],
};
export const critical = ['src/domain/auth.rs', 'src/application/auth.rs', 'src/application/preview.rs',
  'src/application/publication.rs', 'src/domain/content.rs', 'src/domain/search.rs',
  'src/release.rs', 'src/portable.rs', 'src/operations/activation.rs', 'src/operations/restore.rs',
  'src/operations/doctor.rs', 'src/infrastructure/diagnostic_snapshot.rs'];
export function selectChecks(files) {
  if (files === null || files.some(file => /^(Cargo\.|fuzz\/|scripts\/deep-|tests\/support\/parser)/.test(file))) {
    return { fuzz: Object.keys(parsers), mutation: critical };
  }
  return { fuzz: Object.entries(parsers).filter(([, paths]) => paths.some(path => files.includes(path))).map(([name]) => name),
    mutation: critical.filter(path => files.includes(path)) };
}

export function mutationVerdict(value) {
  // A timeout is inconclusive. Build failures/unviable changes are reported
  // separately and never increase the caught count.
  if (!value || !Array.isArray(value.outcomes) || value.outcomes.length === 0) return 'not_run';
  const mutants = value.outcomes.filter(outcome => outcome.scenario !== 'Baseline');
  if (!mutants.length) return 'not_run';
  const statuses = value.outcomes.map(outcome => outcome.summary);
  if (statuses.some(status => /Timeout/i.test(status))) return 'timeout';
  if (statuses.some(status => status === 'Missed' || status === 'Failure')) return 'failed';
  if (statuses.some(status => !['Success', 'Caught', 'Unviable'].includes(status))) return 'evidence_failed';
  return mutants.some(outcome => outcome.summary === 'Caught') ? 'passed' : 'not_run';
}

export function shardChecks(selected, index = 0, count = 1) {
  if (!Number.isInteger(index) || !Number.isInteger(count) || count < 1 || count > 32 || index < 0 || index >= count) throw new Error('Invalid verification shard');
  return Object.fromEntries(Object.entries(selected).map(([kind, values]) => [kind, values.filter((_, position) => position % count === index)]));
}

async function main() {
  const mode = process.argv[2] ?? 'pr';
  if (!['pr', 'daily', 'weekly'].includes(mode)) throw new Error('Expected pr, daily or weekly');
  const base = process.env.VERIFY_BASE;
  const changed = base ? spawnSync('git', ['diff', '--name-only', `${base}...HEAD`], { encoding: 'utf8', windowsHide: true }) : null;
  const applicable = selectChecks(changed?.status === 0 ? changed.stdout.trim().split('\n') : null);
  if (mode === 'daily') { applicable.fuzz = Object.keys(parsers); applicable.mutation = []; }
  if (mode === 'weekly') { applicable.fuzz = []; applicable.mutation = critical; }
  const shard = { index: Number(process.env.VERIFY_SHARD_INDEX ?? 0), count: Number(process.env.VERIFY_SHARD_COUNT ?? 1) };
  const selected = shardChecks(applicable, shard.index, shard.count);
  const directory = join('target/verification', `deep-${mode}-${randomUUID()}`);
  mkdirSync(directory, { recursive: true });
  writeJson(join(directory, 'selection.json'), { mode, base: base ?? null, applicable, selected, shard, comparisonAvailable: changed?.status === 0 });
  const reports = [];
  if (mode !== 'weekly') for (const target of selected.fuzz) {
    const seconds = mode === 'daily' ? 600 : 60;
    const corpus = join('target/fuzz-corpus', target);
    mkdirSync(corpus, { recursive: true });
    reports.push(await runStep({ name: `fuzz-${target}`, command: 'cargo',
      args: ['+nightly', 'fuzz', 'run', target, corpus, `tests/corpus/${target}`, '--',
        `-max_total_time=${seconds}`, '-timeout=10', '-max_len=65536', '-rss_limit_mb=2048', `-seed=${process.env.PROPTEST_RNG_SEED ?? '20260905'}`],
      outputDirectory: directory, timeoutMs: (seconds + 1800) * 1000 }));
    const artifacts = join('fuzz/artifacts', target);
    if (existsSync(artifacts)) for (const entry of readdirSync(artifacts, { withFileTypes: true })) {
      if (!entry.isFile() || !/^(?:crash|timeout|oom|leak)-[a-z0-9]+$/.test(entry.name)) continue;
      const path = join(artifacts, entry.name);
      const bytes = statSync(path).size;
      if (bytes > 1_048_576) throw new Error('Unexpectedly large fuzz evidence; original retained locally');
      const input = readFileSync(path);
      const artifact = { target, bytes, sha256: createHash('sha256').update(input).digest('hex'),
        replay: { command: 'cargo', args: ['+nightly', 'fuzz', 'run', target, path] } };
      try { assertSafeEvidence(input.toString('utf8')); artifact.encoding = 'base64'; artifact.input = input.toString('base64'); }
      catch { artifact.status = 'private_input_retained_locally'; }
      writeJson(join(directory, `fuzz-${target}-${entry.name}.json`), artifact);
    }
    writeJson(join(directory, 'deep.json'), { mode, reports, status: 'running' });
  }
  if (mode !== 'daily') for (const [index, file] of selected.mutation.entries()) {
    const output = join(directory, `mutations-${index}`);
    const report = await runStep({ name: `mutation-${index}`, command: 'cargo',
      args: ['mutants', '--file', file, '--profile', 'mutation', '--output', output, '--timeout', '120', '--build-timeout', '1200', '--jobs', '1', '--no-shuffle', '--cargo-arg=--lib', '--cargo-arg=--tests'],
      outputDirectory: directory, timeoutMs: 4 * 60 * 60_000 });
    const outcomes = join(output, 'mutants.out', 'outcomes.json');
    try { report.verdict = existsSync(outcomes) ? mutationVerdict(JSON.parse(readFileSync(outcomes))) : 'evidence_failed'; }
    catch { report.verdict = 'evidence_failed'; }
    reports.push(report);
    writeJson(join(directory, 'deep.json'), { mode, reports, status: 'running' });
  }
  writeJson(join(directory, 'deep.json'), { mode, reports, status: reports.length === 0 ? 'not_applicable' : reports.every(report => report.status === 'passed' && (!report.verdict || report.verdict === 'passed')) ? 'passed' : 'failed' });
  process.exitCode = reports.every(report => report.status === 'passed' && (!report.verdict || report.verdict === 'passed')) ? 0 : 1;
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
