import { spawnSync } from 'node:child_process';
import { readFileSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { platform, arch, release, cpus } from 'node:os';
import { writeJson } from './runner.mjs';
import { comparePerformance, summarizeSamples } from './performance-summary.mjs';

const directory = process.env.VERIFY_RUN_DIRECTORY ?? 'target/verification/performance';
mkdirSync(directory, { recursive: true });
const fixture = resolve(`target/debug/examples/verification_fixture${platform() === 'win32' ? '.exe' : ''}`);
const samples = [];
for (let index = 0; index < 3; index++) {
  const result = spawnSync(fixture, ['benchmark'], { encoding: 'utf8', windowsHide: true, timeout: 120_000 });
  if (result.status !== 0) throw new Error(`Performance sample ${index} failed (${result.status}); no successful measurement claimed`);
  samples.push(JSON.parse(result.stdout));
}
const rust = spawnSync('rustc', ['--version'], { encoding: 'utf8', windowsHide: true });
if (rust.status !== 0) throw new Error('Performance toolchain identity is unavailable');
const report = { schema: 1, environment: { platform: platform(), arch: arch(), os: release(), cpu: cpus()[0]?.model ?? null, profile: 'debug', rust: rust.stdout.trim() },
  summary: summarizeSamples(samples), samples };
if (process.env.PERFORMANCE_BASELINE) report.comparison = comparePerformance(JSON.parse(readFileSync(process.env.PERFORMANCE_BASELINE)), report);
writeJson(join(directory, 'performance.json'), report);
console.log(JSON.stringify(report));
