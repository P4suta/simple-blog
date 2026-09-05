import { spawn } from 'node:child_process';
import { mkdirSync, openSync, closeSync, writeSync, writeFileSync, renameSync } from 'node:fs';
import { join } from 'node:path';
import { StringDecoder } from 'node:string_decoder';

const sensitive = /^(?:cookie|set-cookie|authorization|csrf|csrf_token|token|setup_token|recovery_codes?|password|secret|body_markdown|body_html)$/i;
export function sanitizeText(text) {
  const clean = value => {
    if (Array.isArray(value)) return value.map(clean);
    if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, sensitive.test(key) ? '[redacted]' : clean(item)]));
    if (typeof value !== 'string') return value;
    return value.replace(/(\/admin\/share\/)[^/\s?"<>]+/g, '$1[redacted]')
      .replace(/([?&](?:token|csrf|code|claim|secret|key)=)[^\s&"<>]+/gi, '$1[redacted]')
      .replace(/\bBearer\s+[A-Za-z0-9._~+\/-]+/gi, 'Bearer [redacted]');
  };
  try { return JSON.stringify(clean(JSON.parse(text))); } catch { return clean(text); }
}

export function writeJson(path, value) {
  writeFileSync(`${path}.tmp`, JSON.stringify(value, null, 2) + '\n', { mode: 0o600 });
  renameSync(`${path}.tmp`, path);
}

function terminate(child) {
  if (!child.pid) return;
  if (process.platform === 'win32') {
    // Numeric PID only; no shell interpolation. Include grandchildren holding log pipes.
    const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
    killer.on('error', () => child.kill('SIGKILL'));
    killer.on('close', () => child.kill('SIGKILL'));
  } else {
    try { process.kill(-child.pid, 'SIGKILL'); } catch { child.kill('SIGKILL'); }
  }
}

export async function runStep({ name, command, args = [], cwd = process.cwd(), env = {},
  outputDirectory, timeoutMs = 30 * 60_000, writeEvidence = writeSync, writeReport = writeJson }) {
  if (!/^[a-z0-9][a-z0-9_.-]*$/.test(name) || !Number.isFinite(timeoutMs) || timeoutMs <= 0) throw new Error('Invalid verification step');
  mkdirSync(outputDirectory, { recursive: true, mode: 0o700 });
  // Reserve a namespace for runner state. A child may legitimately produce
  // e.g. performance.json; completing the step must not overwrite its evidence.
  const reportsDirectory = join(outputDirectory, '_steps');
  mkdirSync(reportsDirectory, { recursive: true, mode: 0o700 });
  const reportPath = join(reportsDirectory, `${name}.json`);
  const report = { schema: 1, name, command, args, cwd, startedAt: new Date().toISOString(),
    timeoutMs, status: 'running', exitCode: null, signal: null, errorCode: null, durationMs: 0 };
  writeReport(reportPath, report);
  const streams = [];
  try {
    for (const kind of ['stdout', 'stderr']) {
      streams.push({ fd: openSync(join(outputDirectory, `${name}.${kind}.log`), 'w', 0o600),
        decoder: new StringDecoder('utf8'), pending: '' });
    }
  } catch (error) {
    for (const stream of streams) closeSync(stream.fd);
    throw error;
  }
  const started = performance.now();
  let evidenceError;
  let timedOut = false;
  let timer;
  let spawnError;
  const child = spawn(command, args, { cwd, env: { ...process.env, ...env },
    shell: false, windowsHide: true, detached: process.platform !== 'win32', stdio: ['ignore', 'pipe', 'pipe'] });
  report.pid = child.pid ?? null;
  // Keep the child under the same bounded cleanup even if publishing its PID
  // fails. Throwing here would orphan a process with open evidence pipes.
  try { writeReport(reportPath, report); }
  catch (error) { evidenceError = error; terminate(child); }
  function consume(index, bytes, final = false) {
    const stream = streams[index];
    try {
      stream.pending += final ? stream.decoder.end() : stream.decoder.write(bytes);
      if (stream.pending.length > 8 * 1024 * 1024) throw new Error('Evidence line exceeds 8 MiB; preserve a minimal reproduction');
      const lines = stream.pending.split('\n');
      stream.pending = final ? '' : lines.pop();
      for (const line of lines) writeEvidence(stream.fd, sanitizeText(line) + '\n');
    } catch (error) {
      evidenceError ??= error;
      terminate(child);
    }
  }
  child.stdout.on('data', bytes => consume(0, bytes));
  child.stderr.on('data', bytes => consume(1, bytes));
  child.on('error', error => { spawnError = error; });
  let forced;
  const finished = new Promise(resolve => {
    child.on('close', (code, signal) => resolve({ code, signal }));
    // A descendant retaining a pipe or a failed OS tree termination must not
    // turn a bounded check into an infinite wait. Preserve an explicit failure.
    forced = setTimeout(() => {
      timedOut = true;
      terminate(child);
      child.stdout.destroy(); child.stderr.destroy(); child.unref();
      report.cleanupIncomplete = true;
      resolve({ code: null, signal: null });
    }, timeoutMs + 5000);
  });
  timer = setTimeout(() => { timedOut = true; terminate(child); }, timeoutMs);
  const { code, signal } = await finished;
  clearTimeout(timer);
  clearTimeout(forced);
  consume(0, null, true);
  consume(1, null, true);
  for (const stream of streams) closeSync(stream.fd);
  Object.assign(report, { exitCode: spawnError ? null : code, signal, durationMs: Math.round(performance.now() - started),
    errorCode: evidenceError ? 'evidence.write_failed' : spawnError?.code ?? null,
    status: evidenceError ? 'evidence_failed' : spawnError ? 'not_run' : timedOut ? 'timeout' : code === 0 ? 'passed' : 'failed' });
  writeReport(reportPath, report);
  return report;
}
