import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { verificationBudget, stepTimeout, EVIDENCE_RESERVE_MS } from './verification-budget.mjs';
import { runStep } from './runner.mjs';

test('the CI deadline includes setup time and retains five minutes for evidence', () => {
  const start = 1_800_000_000_000;
  const budget = verificationBudget({ VERIFY_JOB_STARTED_AT: String(start / 1000), VERIFY_JOB_TIMEOUT_MINUTES: '30' }, start + 10 * 60_000);
  assert.equal(budget.stopAt, start + 25 * 60_000);
  assert.equal(stepTimeout(budget, 60 * 60_000, start + 10 * 60_000), 15 * 60_000 - 5000);
  assert.equal(stepTimeout(budget, 1000, budget.stopAt), 0);
  assert.equal(verificationBudget({}), null);
  for (const env of [{ VERIFY_JOB_STARTED_AT: '0' }, { VERIFY_JOB_STARTED_AT: '0', VERIFY_JOB_TIMEOUT_MINUTES: '5' }, { VERIFY_JOB_STARTED_AT: 'secret', VERIFY_JOB_TIMEOUT_MINUTES: '30' }]) {
    assert.throws(() => verificationBudget(env));
  }
});

test('a real child is stopped before the job deadline and retains partial output', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'verification-budget-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const now = Date.now(), budget = { stopAt: now + 6500, evidenceReserveMs: EVIDENCE_RESERVE_MS };
  const result = await runStep({ name: 'budget', command: process.execPath,
    args: ['-e', 'console.log("partial evidence");setInterval(()=>{},100)'], outputDirectory: directory,
    timeoutMs: stepTimeout(budget, 30 * 60_000, now) });
  assert.equal(result.status, 'timeout');
  assert.match(readFileSync(join(directory, 'budget.stdout.log'), 'utf8'), /partial evidence/);
  assert.ok(Date.now() < budget.stopAt + budget.evidenceReserveMs);
});

test('the common entry records an exhausted job budget as not run and exits unsuccessfully', () => {
  const child = spawnSync(process.execPath, [fileURLToPath(new URL('./verify.mjs', import.meta.url)), 'format'], {
    env: { ...process.env, VERIFY_JOB_STARTED_AT: '0', VERIFY_JOB_TIMEOUT_MINUTES: '30' },
    encoding: 'utf8', windowsHide: true, timeout: 60_000,
  });
  assert.equal(child.status, 1, child.stderr);
  const directory = /^Evidence: (.+)$/m.exec(child.stdout)?.[1].trim();
  assert.ok(directory, 'the failed run must still identify its evidence');
  const report = JSON.parse(readFileSync(join(directory, 'run.json')));
  assert.equal(report.status, 'failed');
  assert.equal(report.steps.length, 1);
  assert.equal(report.steps[0].status, 'not_run');
  assert.equal(report.steps[0].errorCode, 'verification.budget_exhausted');
});

test('every evidence-producing CI job starts a deadline matching its job limit', () => {
  let checked = 0;
  for (const file of ['ci.yml', 'deep-verification.yml']) {
    const yaml = readFileSync(new URL(`../.github/workflows/${file}`, import.meta.url), 'utf8').replaceAll('\r\n', '\n').split('\njobs:\n')[1];
    assert.ok(yaml);
    for (const job of yaml.split(/^  [\w-]+:\r?$/m)) {
      if (!job.includes('uses: ./.github/actions/verification')) continue;
      const limit = /^    timeout-minutes: (\d+)\r?$/m.exec(job)?.[1];
      assert.ok(limit, 'job has an explicit numeric timeout');
      assert.equal(/^      VERIFY_JOB_TIMEOUT_MINUTES: "(\d+)"\r?$/m.exec(job)?.[1], limit);
      assert.ok(job.indexOf('VERIFY_JOB_STARTED_AT=$(date +%s)') >= 0);
      assert.ok(job.indexOf('VERIFY_JOB_STARTED_AT=$(date +%s)') < job.indexOf('uses: actions/checkout@'));
      checked++;
    }
  }
  assert.equal(checked, 11, 'new jobs must explicitly adopt the evidence deadline');
});
