import { test } from 'node:test';
import assert from 'node:assert/strict';
import { selectChecks, mutationVerdict, critical, shardChecks } from './deep-checks.mjs';
test('unknown comparison cannot silently skip the deep checks', () => {
  assert.equal(selectChecks(null).fuzz.length, 3);
  assert.deepEqual(selectChecks(['Cargo.lock']).mutation, critical);
  assert.deepEqual(selectChecks(['README.md']), { fuzz: [], mutation: [] });
  assert.deepEqual(selectChecks(['src/portable.rs']).fuzz, ['portable_site']);
});
test('mutation timeouts are inconclusive and never counted as detections', () => {
  const outcomes = summary => ({ outcomes: [{ scenario: 'Baseline', summary: 'Success' }, { scenario: { Mutant: {} }, summary }] });
  assert.equal(mutationVerdict(outcomes('Caught')), 'passed');
  assert.equal(mutationVerdict(outcomes('Missed')), 'failed');
  assert.equal(mutationVerdict(outcomes('Timeout')), 'timeout');
  assert.equal(mutationVerdict(outcomes('Unviable')), 'not_run');
  assert.equal(mutationVerdict({ outcomes: [] }), 'not_run');
  assert.equal(mutationVerdict(outcomes('new-tool-status')), 'evidence_failed');
});

test('CI shards cover every applicable check exactly once without silently dropping work', () => {
  const selected = selectChecks(null);
  const shards = Array.from({ length: 8 }, (_, index) => shardChecks(selected, index, 8));
  for (const kind of ['fuzz', 'mutation']) assert.deepEqual(shards.flatMap(shard => shard[kind]).sort(), selected[kind].toSorted());
  for (const [index, count] of [[-1, 8], [8, 8], [0, 0], [0, 1.5]]) assert.throws(() => shardChecks(selected, index, count));
});
