import { test } from 'node:test';
import assert from 'node:assert/strict';
import { summarizeSamples, comparePerformance } from './performance-summary.mjs';
const sample = { dataset: 'synthetic-100-v1', dataset_time: '2026-09-05T00:00:00Z', render_store_ms: 3,
  publish_ms: 4, search_100_ms: 5, restore_ms: 6, peak_resident_bytes: 100,
  db_operations: { render_store: 1000, publish: 107, search_100: 100 } };
test('measurement summarizes comparable work and rejects inactive instrumentation', () => {
  const summary = summarizeSamples([sample, { ...sample, publish_ms: 100 }, { ...sample, publish_ms: 5 }]);
  assert.equal(summary.median_ms.publish_ms, 5);
  assert.equal(summarizeSamples([{ ...sample, peak_resident_bytes: null }]).peak_resident_bytes, null);
  assert.throws(() => summarizeSamples([{ ...sample, db_operations: { publish: 0 } }]));
  assert.throws(() => summarizeSamples([sample, { ...sample, dataset: 'other' }]));
  const baseline = { environment: { platform: 'synthetic' }, summary };
  assert.equal(comparePerformance(baseline, baseline).time_ratios.publish_ms, 1);
  assert.equal(comparePerformance(baseline, { ...baseline, environment: {} }).status, 'incomparable');
});
