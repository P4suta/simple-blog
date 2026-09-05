import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { selectChecks, mutationVerdict, critical, shardChecks } from './deep-checks.mjs';

test('the recorded cargo-mutants 27.1.0 output is accepted without guessing its status names', () => {
  const recorded = JSON.parse(readFileSync(new URL('./fixtures/cargo-mutants-27.1.0.json', import.meta.url)));
  assert.equal(mutationVerdict(recorded), 'passed');
  const missingBaseline = structuredClone(recorded);
  missingBaseline.outcomes.shift();
  assert.equal(mutationVerdict(missingBaseline), 'evidence_failed');
  for (const [summary, expected] of [['MissedMutant', 'failed'], ['Timeout', 'timeout'], ['UnknownToolStatus', 'evidence_failed']]) {
    const altered = structuredClone(recorded);
    altered.outcomes[1].summary = summary;
    assert.equal(mutationVerdict(altered), expected);
  }
});
test('unknown comparison cannot silently skip the deep checks', () => {
  assert.equal(selectChecks(null).fuzz.length, 3);
  assert.deepEqual(selectChecks(['Cargo.lock']).mutation, critical);
  assert.deepEqual(selectChecks(['README.md']), { fuzz: [], mutation: [] });
  assert.deepEqual(selectChecks(['src/portable.rs']).fuzz, ['portable_site']);
});
test('mutation timeouts are inconclusive and never counted as detections', () => {
  const outcomes = summary => ({ outcomes: [{ scenario: 'Baseline', summary: 'Success' }, { scenario: { Mutant: {} }, summary }] });
  assert.equal(mutationVerdict(outcomes('CaughtMutant')), 'passed');
  assert.equal(mutationVerdict(outcomes('MissedMutant')), 'failed');
  assert.equal(mutationVerdict(outcomes('Timeout')), 'timeout');
  assert.equal(mutationVerdict(outcomes('Unviable')), 'not_run');
  assert.equal(mutationVerdict({ outcomes: [] }), 'not_run');
  assert.equal(mutationVerdict(outcomes('new-tool-status')), 'evidence_failed');
  assert.equal(mutationVerdict({ outcomes: [{ scenario: { Mutant: {} }, summary: 'CaughtMutant' }] }), 'evidence_failed');
  assert.equal(mutationVerdict(outcomes('Success')), 'evidence_failed');
  assert.equal(mutationVerdict({ outcomes: [...outcomes('CaughtMutant').outcomes, { scenario: 'Baseline', summary: 'Success' }] }), 'evidence_failed');
});

test('CI shards cover every applicable check exactly once without silently dropping work', () => {
  const selected = selectChecks(null);
  const shards = Array.from({ length: 8 }, (_, index) => shardChecks(selected, index, 8));
  for (const kind of ['fuzz', 'mutation']) assert.deepEqual(shards.flatMap(shard => shard[kind]).sort(), selected[kind].toSorted());
  for (const [index, count] of [[-1, 8], [8, 8], [0, 0], [0, 1.5]]) assert.throws(() => shardChecks(selected, index, count));
});
