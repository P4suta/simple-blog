import { defineConfig, devices } from '@playwright/test';
import path from 'node:path';

process.env.PLAYWRIGHT_BROWSERS_PATH ??= path.resolve('target/playwright-browsers');
const evidence = process.env.VERIFY_RUN_DIRECTORY ?? 'target';

export default defineConfig({
  testDir: './tests/browser', outputDir: path.join(evidence, 'browser-results'),
  timeout: 60_000, expect: { timeout: 10_000 }, workers: 1, retries: 0,
  forbidOnly: !!process.env.CI,
  reporter: [['list'], ['json', { outputFile: path.join(evidence, 'browser-report/results.json') }],
    ['html', { outputFolder: path.join(evidence, 'browser-report/html'), open: 'never' }]],
  // Custom fixtures export sanitized traces; raw authentication state stays private.
  use: { trace: 'off', screenshot: 'off', video: 'off', locale: 'en-US', timezoneId: 'Asia/Tokyo' },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'firefox', testIgnore: ['**/passkey.spec.ts', '**/ime.spec.ts'], use: { ...devices['Desktop Firefox'] } },
    { name: 'webkit', testIgnore: ['**/passkey.spec.ts', '**/ime.spec.ts'], use: { ...devices['Desktop Safari'] } },
  ],
});
