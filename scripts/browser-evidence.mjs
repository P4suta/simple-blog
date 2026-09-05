export function summarizeBrowser(report) {
  const tests = [], attachments = [];
  function visit(suite) {
    for (const spec of suite.specs ?? []) for (const test of spec.tests ?? []) {
      tests.push({ title: spec.title, file: spec.file, line: spec.line, project: test.projectName,
        expectedStatus: test.expectedStatus, status: test.status,
        results: test.results.map(result => ({ status: result.status, duration: result.duration, retry: result.retry })) });
      for (const result of test.results) for (const item of result.attachments ?? []) {
        if (['sanitized trace', 'server events'].includes(item.name) && item.path) attachments.push(item);
      }
    }
    for (const child of suite.suites ?? []) visit(child);
  }
  visit(report);
  // Errors, steps, commands, raw attachments and configuration may quote private
  // authentication values. Select fields; never spread an external report.
  const { startTime, duration, expected, skipped, unexpected, flaky } = report.stats ?? {};
  return { summary: { stats: { startTime, duration, expected, skipped, unexpected, flaky }, tests }, attachments };
}
