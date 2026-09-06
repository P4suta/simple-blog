const identifiers = /^(?:request_id|operation_id|parent_operation_id|build_id|release_id)$/;
const labels = /^(?:name|event|operation|error_code|phase|disposition|method)$/;
const measurements = /^(?:status|elapsed_ms|public_revision|route_count|staged_object_count|content_count|redirect_count|media_count|bytes|line_number)$/;
function allowedFields(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  return Object.fromEntries(Object.entries(value).filter(([key, field]) =>
    (identifiers.test(key) && typeof field === 'string' && /^(?:[0-9a-f]{64}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})$/i.test(field)) ||
    (labels.test(key) && typeof field === 'string' && /^[a-z0-9_.<>]{1,120}$/i.test(field)) ||
    (measurements.test(key) && typeof field === 'number' && Number.isFinite(field) && field >= 0)));
}

export function sanitizeServerEvents(text) {
  return text.split('\n').filter(line => line.trim()).map((line, index) => {
    let source;
    try { source = JSON.parse(line); }
    catch { return JSON.stringify({ fields: { event: 'evidence.unclassified_server_line', line_number: index + 1 } }); }
    const result = allowedFields(source);
    if (typeof source?.timestamp === 'string' && /^\d{4}-\d\d-\d\dT[\d:.]+Z$/.test(source.timestamp)) result.timestamp = source.timestamp;
    if (['TRACE', 'DEBUG', 'INFO', 'WARN', 'ERROR'].includes(source?.level)) result.level = source.level;
    if (source?.fields) result.fields = allowedFields(source.fields);
    if (source?.span) result.span = allowedFields(source.span);
    if (Array.isArray(source?.spans)) result.spans = source.spans.map(allowedFields);
    return JSON.stringify(result);
  }).join('\n') + '\n';
}

export function summarizeBrowser(report) {
  const tests = [], attachments = [];
  function visit(suite) {
    for (const spec of suite.specs ?? []) for (const test of spec.tests ?? []) {
      tests.push({ title: spec.title, file: spec.file, line: spec.line, project: test.projectName,
        expectedStatus: test.expectedStatus, status: test.status,
        results: test.results.map(result => ({ status: result.status, duration: result.duration, retry: result.retry })) });
      for (const result of test.results) for (const item of result.attachments ?? []) {
        if ((item.name === 'sanitized trace' || item.name === 'server events') && typeof item.path === 'string') {
          attachments.push({ name: item.name, path: item.path });
        } else if (item.name === 'server events' && typeof item.body === 'string') {
          const bytes = Buffer.from(item.body, 'base64');
          if (bytes.toString('base64') !== item.body) throw new Error('Invalid inline log encoding');
          attachments.push({ name: item.name, body: Buffer.from(sanitizeServerEvents(bytes.toString('utf8'))).toString('base64') });
        }
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
