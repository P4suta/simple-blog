import { copyFileSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

export function exportLinuxSymbols(executable, output, invoke = (args) => spawnSync('objcopy', args, { stdio: 'inherit' })) {
  // Keep Cargo's original intact. Every export starts with the same complete
  // binary, including on retries after a partial objcopy failure.
  const binary = join(output, 'simple-blog');
  const debug = join(output, 'simple-blog.debug');
  copyFileSync(executable, binary);
  for (const args of [['--only-keep-debug', executable, debug], ['--strip-debug', binary], [`--add-gnu-debuglink=${debug}`, binary]]) {
    const result = invoke(args);
    if (result.status !== 0) throw new Error('Could not preserve release debug symbols');
  }
  return [binary, debug];
}
