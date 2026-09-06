import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync } from 'node:fs';
import { platform, arch, release } from 'node:os';
import { resolve, join, delimiter } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runStep, writeJson } from './runner.mjs';
import { checks, profiles } from './verification.config.mjs';
import { fingerprintInputs, snapshotInputs, assertInputsUnchanged } from './source-inputs.mjs';
import { verificationBudget, stepTimeout } from './verification-budget.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
process.chdir(root);
const localTools = ['target/dev-tools/node_modules/bun/bin', 'target/dev-tools/perl/perl/bin', 'target/dev-tools/repository'].map(path => join(root, path));
process.env.PATH = `${localTools.join(delimiter)}${delimiter}${process.env.PATH}`;
process.env.RUST_BACKTRACE ??= '1';
process.env.PROPTEST_RNG_SEED ??= '20260905';
process.env.PLAYWRIGHT_BROWSERS_PATH ??= join(root, 'target/playwright-browsers');
const requested = process.argv.length > 2 ? process.argv.slice(2) : (process.env.VERIFY_CHECKS ?? '').split(/\s+/).filter(Boolean);
const selected = [...new Set((requested.length ? requested : ['all']).flatMap(name => profiles[name] ?? [name]))];
if (selected.some(name => !checks[name])) throw new Error(`Unknown checks: ${selected.filter(name => !checks[name]).join(', ')}`);
const directory = resolve('target/verification', `${new Date().toISOString().replace(/[:.]/g, '-')}-${randomUUID().slice(0, 8)}`);
mkdirSync(directory, { recursive: true });
const capture = (command, args, trim = true) => {
  const result = spawnSync(command, args, { encoding: 'utf8', timeout: 10_000, windowsHide: true });
  return result.status === 0 ? trim ? result.stdout.trim() : result.stdout : null;
};
const diff = capture('git', ['diff', '--binary', 'HEAD']);
const sourceFiles = capture('git', ['ls-files', '-co', '--exclude-standard', '-z'], false);
const inputs = fingerprintInputs(root, (sourceFiles ?? '').split('\0').filter(Boolean));
const report = { schema: 1, revision: capture('git', ['rev-parse', 'HEAD']),
  dirty: (capture('git', ['status', '--porcelain']) ?? '') !== '',
  diffHash: diff === null ? null : createHash('sha256').update(diff).digest('hex'),
  os: { platform: platform(), arch: arch(), release: release() }, seed: process.env.PROPTEST_RNG_SEED,
  conditions: { ci: Boolean(process.env.CI), cargoBuildJobs: process.env.CARGO_BUILD_JOBS ?? null,
    rustupToolchain: process.env.RUSTUP_TOOLCHAIN ?? null, rustBacktrace: process.env.RUST_BACKTRACE,
    opensslSource: process.env.OPENSSL_NO_VENDOR === '1' ? 'explicit_prebuilt' : 'manifest_default',
    opensslPerl: process.env.OPENSSL_SRC_PERL ?? process.env.PERL ?? 'PATH',
    fuzzShard: { index: process.env.VERIFY_SHARD_INDEX ?? '0', count: process.env.VERIFY_SHARD_COUNT ?? '1' } },
  tools: { node: process.version, rust: capture('rustc', ['--version', '--verbose']),
    perl: capture(process.env.OPENSSL_SRC_PERL ?? process.env.PERL ?? 'perl', ['-MIPC::Cmd', '-e', 'print "$^V\n"']),
    cargo: capture('cargo', ['--version']), bun: capture(process.platform === 'win32' ? 'bun.exe' : 'bun', ['--version']),
    fuzz: capture('cargo', ['fuzz', '--version']), mutants: capture('cargo', ['mutants', '--version']),
    llvmCov: capture('cargo', ['llvm-cov', '--version']), actionlint: capture('actionlint', ['--version']),
    gitleaks: capture('gitleaks', ['version']) },
  inputs, sourceSnapshot: '_source (private, not exported)',
  rerun: { command: process.execPath, args: ['scripts/verify.mjs', ...requested], seed: process.env.PROPTEST_RNG_SEED },
  selected, steps: [], status: 'running' };
const manifest = join(directory, 'run.json');
writeJson(manifest, report);
console.log(`Evidence: ${directory}`);
try {
  if (!report.revision || sourceFiles === null || diff === null) throw new Error('Source revision metadata unavailable');
  report.budget = verificationBudget(process.env);
  snapshotInputs(root, inputs, join(directory, '_source'));
  for (const name of selected) {
    console.log(`Running ${name}`);
    const definition = { ...checks[name], args: [...checks[name].args] };
    if (name === 'coverage') definition.args[definition.args.indexOf('--output-path') + 1] = join(directory, 'lcov.info');
    const timeoutMs = stepTimeout(report.budget, definition.timeoutMs);
    if (timeoutMs === 0) {
      report.steps.push({ name, command: definition.command, args: definition.args, status: 'not_run',
        exitCode: null, errorCode: 'verification.budget_exhausted', durationMs: 0 });
      writeJson(manifest, report);
      console.log(`${name}: not_run (verification budget exhausted; preserving evidence)`);
      continue;
    }
    definition.timeoutMs = timeoutMs;
    const result = await runStep({ name, ...definition, env: { ...checks[name].env, VERIFY_RUN_DIRECTORY: directory }, cwd: root, outputDirectory: directory });
    report.steps.push(result);
    writeJson(manifest, report);
    console.log(`${name}: ${result.status} (${result.durationMs} ms)`);
    if (name === 'coverage' && result.status !== 'passed') {
      report.steps.push(await runStep({ name: 'coverage-partial', command: 'cargo', args: ['llvm-cov', 'report', '--profile', 'verification', '--lcov', '--output-path', join(directory, 'lcov.partial.info')], cwd: root, outputDirectory: directory, timeoutMs: 120_000 }));
      writeJson(manifest, report);
    }
  }
  const finalSources = capture('git', ['ls-files', '-co', '--exclude-standard', '-z'], false);
  if (finalSources === null) throw new Error('Final source enumeration unavailable');
  assertInputsUnchanged(inputs, fingerprintInputs(root, finalSources.split('\0').filter(Boolean)));
  report.inputStability = 'unchanged';
  report.status = report.steps.every(step => step.status === 'passed') ? 'passed' : 'failed';
} catch (error) {
  report.status = 'evidence_failed';
  report.errorCode = error.code ?? 'evidence.unavailable';
  console.error('Verification evidence could not be preserved; this run failed.');
} finally {
  writeJson(manifest, report);
  process.exitCode = report.status === 'passed' ? 0 : 1;
}
