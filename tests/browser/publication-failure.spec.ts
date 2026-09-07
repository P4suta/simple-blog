import { test, expect } from './fixture';
import { writeFile, readFile, rm } from 'node:fs/promises';
import path from 'node:path';

test.use({ injectFaults: true });
test('a committed save survives publication failure while the old public release remains available', async ({ page, site }) => {
  test.setTimeout(90_000);
  await site.login(page);
  await page.goto(`${site.origin}/admin/content/1/edit/`);
  const previous = await (await page.request.get(`${site.origin}/synthetic-0/`)).text();
  const control = path.dirname(site.data);
  await writeFile(path.join(control, 'fault.json'), JSON.stringify({ event: 'release.publish.objects_stored', mode: 'block_manifest' }));
  const saved = page.waitForResponse(response => response.request().method() === 'POST' && response.url().includes('/admin/content/1/'));
  await page.locator('.cm-content').fill('Committed writing survives failed publication');
  const response = await saved;
  expect(response.status()).toBe(200);
  expect((await response.json()).site).toBe('pending');
  const reached = JSON.parse(await readFile(path.join(control, 'fault-reached.json'), 'utf8'));
  expect(reached.event).toBe('release.publish.objects_stored');
  await expect(page.locator('[data-save-state]')).toHaveAttribute('data-pending', 'true');
  await expect(page.locator('[data-save-state]')).toContainText(/Inquiry ID: [0-9a-f-]{36}/i);
  expect(await (await page.request.get(`${site.origin}/synthetic-0/`)).text()).toBe(previous);
  await page.reload();
  await expect(page.locator('.cm-content')).toContainText('Committed writing survives failed publication');
  // This path came from the local fault controller; validate confinement before removal.
  const blocker = path.resolve(reached.blocker);
  expect(path.dirname(blocker)).toBe(path.resolve(site.data, 'releases/manifests'));
  await rm(blocker, { recursive: true });
  await expect.poll(async () => (await page.request.get(`${site.origin}/synthetic-0/`)).text(), { timeout: 45_000 }).toContain('Committed writing survives failed publication');
});
