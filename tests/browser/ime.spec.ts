import { test, expect } from './fixture';
test('Japanese composition commits before a keyboard save without losing text', async ({ page, context, site }) => {
  await site.login(page);
  await page.goto(`${site.origin}/admin/content/1/edit/`);
  const editor = page.locator('.cm-content');
  await editor.fill('');
  await editor.focus();
  const cdp = await context.newCDPSession(page);
  await cdp.send('Input.imeSetComposition', { text: 'にほんご', selectionStart: 4, selectionEnd: 4 });
  await cdp.send('Input.insertText', { text: '日本語を入力して保存' });
  await expect(editor).toContainText('日本語を入力して保存');
  const response = page.waitForResponse(response => response.request().method() === 'POST' && response.url().includes('/admin/content/1/'));
  await editor.press('ControlOrMeta+s');
  expect((await response).ok()).toBe(true);
  await page.reload();
  await expect(editor).toContainText('日本語を入力して保存');
  await cdp.detach();
});
