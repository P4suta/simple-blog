import { test as base, expect, type Page } from '@playwright/test';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import net from 'node:net';
import { sanitizeText } from '../../scripts/runner.mjs';
import { sanitizeTrace } from '../../scripts/trace-artifacts.mjs';

type Site = { origin: string; data: string; setupToken?: string; recoveryCodes: string[]; login(page: Page): Promise<void> };
const binary = path.resolve(`target/debug/simple-blog${process.platform === 'win32' ? '.exe' : ''}`);
const fixture = path.resolve(`target/debug/examples/verification_fixture${process.platform === 'win32' ? '.exe' : ''}`);

async function port(): Promise<number> {
  const server = net.createServer();
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const address = server.address() as net.AddressInfo;
  await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return address.port;
}

export const test = base.extend<{ site: Site; unowned: boolean; injectFaults: boolean }>({
  unowned: [false, { option: true }],
  injectFaults: [false, { option: true }],
  site: async ({ context, unowned, injectFaults }, use, info) => {
    const temporary = await mkdtemp(path.join(tmpdir(), 'simple-blog-browser-'));
    const data = path.join(temporary, 'site');
    const selectedPort = await port();
    const origin = `http://localhost:${selectedPort}`;
    const seeded = spawnSync(fixture, [unowned ? 'seed-unowned' : 'seed', data, origin], { encoding: 'utf8', windowsHide: true, timeout: 30_000 });
    if (seeded.status !== 0) {
      await rm(temporary, { recursive: true, force: true });
      throw new Error(`Disposable fixture failed: ${sanitizeText(seeded.stderr ?? String(seeded.error))}`);
    }
    const { recovery_codes: recoveryCodes, setup_token: setupToken }: { recovery_codes: string[]; setup_token?: string } = JSON.parse(seeded.stdout);
    const secrets = [...recoveryCodes, ...(setupToken ? [setupToken] : [])];
    const secretReads: Promise<void>[] = [];
    let secretCaptureFailed = false;
    context.on('response', response => {
      secretReads.push((async () => {
        const headers = await response.allHeaders();
        for (const match of (headers['set-cookie'] ?? '').matchAll(/sb_(?:session|csrf)=([^;\n,]+)/g)) secrets.push(match[1]);
        if (response.url().endsWith('/admin/auth/setup/finish') && response.ok()) {
          const payload = await response.json();
          secrets.push(...payload.recovery_codes);
        }
      })().catch(() => { secretCaptureFailed = true; }));
    });
    const server = spawn(injectFaults ? fixture : binary, [...(injectFaults ? ['run', temporary] : []), '--data-dir', data, '--bind', `127.0.0.1:${selectedPort}`, 'serve'], {
      windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, SIMPLE_BLOG_LOG_FORMAT: 'json', RUST_LOG: 'simple_blog=debug' },
    });
    const logs: string[] = [];
    server.stderr.on('data', bytes => logs.push(bytes.toString()));
    server.stdout.resume();
    const exited = new Promise<void>(resolve => { server.once('exit', () => resolve()); server.once('error', () => resolve()); });
    let traceStarted = false;
    try {
      await expect.poll(async () => {
        if (server.exitCode !== null) throw new Error('Fixture server exited before readiness');
        try { return (await fetch(`${origin}/healthz`)).status; } catch { return 0; }
      }, { timeout: 20_000 }).toBe(200);
      await context.tracing.start({ snapshots: true, screenshots: false, sources: false });
      traceStarted = true;
      await use({ origin, data, recoveryCodes, setupToken, async login(page) {
        const code = recoveryCodes.shift();
        if (!code) throw new Error('Fixture recovery codes exhausted');
        await page.goto(`${origin}/admin/login/`);
        await page.locator('details > summary').click();
        await page.locator('input[name=code]').fill(code);
        await page.locator('form[action="/admin/auth/recovery"] button').click();
        await expect(page).toHaveURL(`${origin}/admin/settings/`);
        await page.waitForLoadState('load');
      } });
    } finally {
      try {
      // All contexts belong to this disposable fixture; no live user sessions.
      await Promise.all(secretReads);
      for (const cookie of await context.cookies().catch(() => { secretCaptureFailed = true; return []; })) {
        if (/^sb_(session|csrf)$/.test(cookie.name)) secrets.push(cookie.value);
      }
      const privateTrace = path.join(temporary, 'trace.zip');
      if (traceStarted) await context.tracing.stop({ path: privateTrace });
      if (traceStarted && info.status !== info.expectedStatus) {
        if (secretCaptureFailed) throw new Error('Credential capture was incomplete; private trace export refused');
        const trace = info.outputPath('trace.zip');
        await writeFile(trace, sanitizeTrace(await readFile(privateTrace), secrets));
        await info.attach('sanitized trace', { path: trace, contentType: 'application/zip' });
        for (const page of context.pages()) {
          if (!page.isClosed()) await info.attach('failure screen', { body: await page.screenshot({
            mask: [page.locator('[data-codes], [data-setup-token], [data-share-url], input[name=code], input[type=hidden]')],
          }), contentType: 'image/png' });
        }
      }
      const safeLog = logs.join('').split('\n').map(line => {
        let value = sanitizeText(line);
        for (const secret of secrets) if (secret) value = value.replaceAll(secret, '[redacted]');
        return value;
      }).join('\n');
      await info.attach('server events', { body: safeLog, contentType: 'text/plain' });
      } finally {
      server.kill('SIGKILL');
      await exited;
      await rm(temporary, { recursive: true, force: true, maxRetries: 8, retryDelay: 150 });
      }
    }
  },
});
export { expect };
