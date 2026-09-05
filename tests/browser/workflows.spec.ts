import { test, expect } from './fixture';
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

async function navigateByForm(page: Page, submit: () => Promise<void>) {
  const committed = page.waitForEvent('framenavigated', { predicate: frame => frame === page.mainFrame() });
  await submit();
  await committed;
  await page.waitForLoadState('load');
}

test('reader search, navigation, narrow layout and keyboard access', async ({ page, site }) => {
  await page.goto(site.origin);
  await expect(page.getByRole('link', { name: 'Synthetic article 0' })).toBeVisible();
  await page.goto(`${site.origin}/search/`);
  await page.getByRole('searchbox').fill('日本語 Rust');
  await expect(page.locator('[data-search-results]')).toContainText('Synthetic article 0');
  await page.getByRole('link', { name: 'Synthetic article 0' }).click();
  await expect(page.locator('article')).toContainText('日本語 Rust');
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const accessibility = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa']).analyze();
  expect(accessibility.violations).toEqual([]);
});

test('author can write Japanese, publish, share, revoke and recover from trash', async ({ page, playwright, site }) => {
  await site.login(page);
  await page.goto(`${site.origin}/admin/content/new/`);
  await page.locator('[name=title]').fill('日本語 verification');
  await page.locator('.cm-content').fill('保存される日本語の本文');
  await page.locator('[data-publish]').click();
  await expect(page.locator('[data-save-state]')).toContainText(/Saved/i);
  await expect(page).toHaveURL(/\/admin\/content\/\d+\/edit\//);
  const editUrl = page.url();
  await page.locator('[data-preview-toggle]').click();
  await expect(page.locator('iframe')).toBeVisible();
  await page.goto(editUrl);
  await expect(page.locator('.cm-content')).toContainText('保存される日本語の本文');
  const id = editUrl.match(/content\/(\d+)/)![1];
  await page.locator('[data-drawer-toggle]').click();
  await page.locator('[data-share] > summary').click();
  await page.locator('button[form=share-form]').click();
  await expect(page.locator('[data-share-url]')).toContainText('/admin/share/');
  const url = await page.locator('[data-share-url]').innerText();
  const anonymous = await playwright.request.newContext();
  const preview = await anonymous.get(url);
  expect(await preview.text()).toContain('保存される日本語の本文');
  const revoked = page.waitForResponse(response => response.request().method() === 'POST' && response.url().includes('/share/revoke/'));
  await navigateByForm(page, () => page.locator('button[form=share-revoke-form]').click());
  expect((await revoked).status()).toBe(303);
  expect((await anonymous.get(url)).status()).toBe(404);
  await anonymous.dispose();
  await page.goto(editUrl);
  await page.locator('[data-drawer-toggle]').click();
  await page.locator('.danger-zone > summary').click();
  await navigateByForm(page, () => page.locator('button[form=trash-form]').click());
  await page.goto(editUrl);
  await expect(page.locator('.trash-banner')).toBeVisible();
  await navigateByForm(page, () => page.locator(`form[action="/admin/content/${id}/restore/"] button`).click());
  await expect(page.locator('[name=title]')).toHaveValue('日本語 verification');
});

test('offline autosave preserves input and succeeds after connectivity returns', async ({ page, context, site }) => {
  await site.login(page);
  await page.goto(`${site.origin}/admin/content/1/edit/`);
  await context.setOffline(true);
  await page.locator('.cm-content').fill('Network failure must preserve this writing');
  await expect(page.locator('[data-save-state]')).toHaveAttribute('data-error', 'true');
  await expect(page.locator('.cm-content')).toContainText('Network failure must preserve this writing');
  await context.setOffline(false);
  await page.locator('.cm-content').press('ControlOrMeta+s');
  await expect(page.locator('[data-save-state]')).toContainText(/Saved/i);
  await page.reload();
  await expect(page.locator('.cm-content')).toContainText('Network failure must preserve this writing');
});

test('expired authentication leaves unsaved writing recoverable', async ({ page, context, site }) => {
  await site.login(page);
  await page.goto(`${site.origin}/admin/content/1/edit/`);
  await context.clearCookies();
  await page.locator('.cm-content').fill('Writing after authentication expired');
  await expect(page.locator('[data-save-state]')).toHaveAttribute('data-error', 'true');
  await expect(page.locator('.cm-content')).toContainText('Writing after authentication expired');
  await expect(page.locator('[data-save-state]')).toContainText(/Inquiry ID: [0-9a-f-]{36}/i);
});

test('concurrent edits show a conflict and preserve both versions', async ({ page, context, site }) => {
  await site.login(page);
  await page.goto(`${site.origin}/admin/content/1/edit/`);
  const second = await context.newPage();
  await second.goto(page.url());
  const saved = page.waitForResponse(response => response.request().method() === 'POST' && response.url().includes('/admin/content/1/'));
  await page.locator('.cm-content').fill('The first committed version');
  expect((await saved).status()).toBe(200);
  await second.locator('.cm-content').fill('The second unsaved version');
  await expect(second.locator('body')).toContainText('The first committed version');
  await expect(second.locator('body')).toContainText('The second unsaved version');
  await page.reload();
  await expect(page.locator('.cm-content')).toContainText('The first committed version');
  await second.close();
});

test('details dialog supports keyboard focus and timezone round trips', async ({ page, browser, site }) => {
  await site.login(page);
  await page.goto(`${site.origin}/admin/content/1/edit/`);
  const toggle = page.locator('[data-drawer-toggle]');
  await toggle.focus();
  await toggle.press('Enter');
  await expect(page.locator('[data-drawer]')).toBeVisible();
  await expect(page.locator('[data-publish-at-hint]')).toContainText('Asia/Tokyo');
  await page.locator('[data-publish-at]').fill('2099-01-01T09:30');
  const saved = page.waitForResponse(response => response.request().method() === 'POST' && response.url().includes('/admin/content/1/'));
  await page.keyboard.press('ControlOrMeta+s');
  expect((await saved).status()).toBe(200);
  await page.keyboard.press('Escape');
  await expect(page.locator('[data-drawer]')).not.toBeVisible();
  await expect(toggle).toBeFocused();
  await page.reload();
  await page.locator('[data-drawer-toggle]').click();
  await expect(page.locator('[data-publish-at]')).toHaveValue('2099-01-01T09:30');
  await expect(page.locator('[data-status-time]')).toHaveAttribute('datetime', /^2099-01-01T00:30:00/);
  const otherZone = await browser.newContext({ timezoneId: 'America/New_York' });
  try {
    const otherPage = await otherZone.newPage();
    await site.login(otherPage);
    await otherPage.goto(`${site.origin}/admin/content/1/edit/`);
    await otherPage.locator('[data-drawer-toggle]').click();
    await expect(otherPage.locator('[data-publish-at]')).toHaveValue('2098-12-31T19:30');
  } finally { await otherZone.close(); }
});

test('blocked browser storage does not prevent a server save', async ({ page, context, site }) => {
  await context.addInitScript(() => {
    Object.assign(window, { storageFaultCount: 0 });
    Storage.prototype.setItem = () => {
      (window as Window & { storageFaultCount: number }).storageFaultCount++;
      throw new DOMException('Synthetic quota failure', 'QuotaExceededError');
    };
  });
  await site.login(page);
  await page.goto(`${site.origin}/admin/content/1/edit/`);
  const saved = page.waitForResponse(response => response.request().method() === 'POST' && response.url().includes('/admin/content/1/'));
  await page.locator('.cm-content').fill('Server save survives storage failure');
  expect((await saved).ok()).toBe(true);
  expect(await page.evaluate(() => (window as Window & { storageFaultCount: number }).storageFaultCount)).toBeGreaterThan(0);
  await expect(page.locator('[data-save-state]')).toContainText(/Saved/i);
  await page.reload();
  await expect(page.locator('.cm-content')).toContainText('Server save survives storage failure');
});

test('basic reading and editing work with JavaScript disabled', async ({ browser, site }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  try {
    const page = await context.newPage();
    await page.goto(`${site.origin}/synthetic-0/`);
    await expect(page.locator('article')).toContainText('日本語 Rust');
    await site.login(page);
    await page.goto(`${site.origin}/admin/content/1/edit/`);
    await page.locator('textarea[name=body_markdown]').fill('Plain HTML form writing');
    await page.locator('[name=title]').fill('Plain HTML title');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.locator('textarea[name=body_markdown]')).toHaveValue('Plain HTML form writing');
  } finally { await context.close(); }
});
