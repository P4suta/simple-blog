import { statSync } from 'node:fs';
import { win32 } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export function discoverGitBash(env = process.env, isFile = path => {
  try { return statSync(path).isFile(); } catch { return false; }
}) {
  const directories = (env.PATH ?? '').split(';').map(path => path.replace(/^"|"$/g, '')).filter(Boolean);
  const candidates = env.VERIFY_BASH ? [env.VERIFY_BASH] : [
    ...directories.map(directory => win32.join(directory, 'bash.exe')),
    ...directories.filter(directory => isFile(win32.join(directory, 'git.exe'))).map(directory => win32.resolve(directory, '../bin/bash.exe')),
    win32.join(env.ProgramFiles ?? 'C:/Program Files', 'Git/bin/bash.exe'),
  ];
  return candidates.map(path => win32.normalize(path)).find(path =>
    win32.isAbsolute(path) && isFile(path) && isFile(win32.join(win32.dirname(path), 'sh.exe')));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const bash = discoverGitBash();
  if (!bash) {
    console.error('policy.bash_unavailable: Install Git for Windows or set VERIFY_BASH to its bash.exe.');
    process.exitCode = 1;
  } else {
    const directory = win32.dirname(bash);
    const result = spawnSync(bash, ['tests/repository_policy.sh'], {
      stdio: 'inherit', windowsHide: true,
      env: { ...process.env, PATH: `${directory};${win32.resolve(directory, '../usr/bin')};${process.env.PATH}` },
    });
    if (result.error) console.error(`policy.shell_failed: ${result.error.code ?? 'unknown'}`);
    process.exitCode = result.status ?? 1;
  }
}
