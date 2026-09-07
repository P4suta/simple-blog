import { test, expect } from './fixture';

test.use({ unowned: true });
test('a real WebAuthn registration and assertion authenticate the owner', async ({ page, context, browserName, site }) => {
  test.skip(browserName !== 'chromium', 'CDP virtual authenticators are Chromium-only; recovery authentication is tested on all three engines');
  const cdp = await context.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', { options: {
    protocol: 'ctap2', transport: 'internal', hasResidentKey: true,
    hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true,
  } });
  try {
    await page.goto(`${site.origin}/admin/setup/?token=${site.setupToken}`);
    await page.locator('[data-passkey-name]').fill('Verification authenticator');
    await page.locator('[data-passkey-action]').click();
    await expect(page.locator('[data-codes]')).toBeVisible();
    await page.locator('[data-confirm-saved]').check();
    await page.locator('[data-recovery-continue]').click();
    await expect(page).toHaveURL(`${site.origin}/admin/`);
    await context.clearCookies();
    await page.goto(`${site.origin}/admin/login/`);
    await page.locator('[data-passkey-action]').click();
    await expect(page).toHaveURL(`${site.origin}/admin/`);
    expect((await cdp.send('WebAuthn.getCredentials', { authenticatorId })).credentials.length).toBe(1);
  } finally { await cdp.send('WebAuthn.removeVirtualAuthenticator', { authenticatorId }); }
});
