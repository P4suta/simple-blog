const timings = ['render_store_ms', 'publish_ms', 'search_100_ms', 'restore_ms'];
const median = values => values.toSorted((a, b) => a - b)[Math.floor(values.length / 2)];
export function summarizeSamples(samples) {
  if (!samples.length) throw new Error('No performance samples');
  const first = samples[0];
  for (const sample of samples) {
    if (sample.dataset !== first.dataset || sample.dataset_time !== first.dataset_time) throw new Error('Performance dataset changed');
    for (const key of timings) if (!Number.isFinite(sample[key]) || sample[key] < 0) throw new Error('Performance timing unavailable');
    if (!sample.db_operations || Object.values(sample.db_operations).some(value => !Number.isInteger(value) || value <= 0)) throw new Error('Database instrumentation did not observe the workload');
  }
  return { dataset: first.dataset, dataset_time: first.dataset_time, samples: samples.length,
    median_ms: Object.fromEntries(timings.map(key => [key, median(samples.map(sample => sample[key]))])),
    peak_resident_bytes: samples.every(sample => Number.isFinite(sample.peak_resident_bytes) && sample.peak_resident_bytes > 0)
      ? median(samples.map(sample => sample.peak_resident_bytes)) : null,
    db_operations: first.db_operations };
}
export function comparePerformance(baseline, current) {
  if (JSON.stringify(baseline.environment) !== JSON.stringify(current.environment) ||
      baseline.summary.dataset !== current.summary.dataset || baseline.summary.dataset_time !== current.summary.dataset_time) {
    return { status: 'incomparable', reason: 'environment_or_dataset_changed' };
  }
  return { status: 'compared', time_ratios: Object.fromEntries(timings.map(key =>
    [key, baseline.summary.median_ms[key] > 0 ? current.summary.median_ms[key] / baseline.summary.median_ms[key] : null])),
    db_operation_delta: Object.fromEntries(Object.entries(current.summary.db_operations).map(([key, value]) => [key, value - baseline.summary.db_operations[key]])) };
}
