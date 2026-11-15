import { test, expect } from '@playwright/test';
const message = 'Copy this exact message: 项目 review';
test('user message keyboard copy writes exact text and reports success', async ({ page, context }) => {
 await context.grantPermissions(['clipboard-read', 'clipboard-write']);
 await page.goto('/?message-copy');
 await expect(page.getByText(message, { exact: true })).toBeVisible();
 const copy = page.getByRole('button', { name: 'Copy message', exact: true });
 await page.keyboard.press('Tab'); await expect(copy).toBeFocused();
 await page.keyboard.press('Enter');
 await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(message);
 await expect(page.getByRole('button', { name: 'Copied', exact: true })).toBeVisible();
 await expect(copy).toBeVisible({ timeout: 5000 });
});
for (const mode of ['rejected', 'missing']) test(`user message clipboard ${mode} reports failure without false success`, async ({ page }) => {
 await page.addInitScript(mode => Object.defineProperty(navigator, 'clipboard', { configurable: true,
  value: mode === 'missing' ? undefined : { writeText: async () => { throw new Error('Denied'); } } }), mode);
 await page.goto('/?message-copy');
 await expect(page.getByText(message, { exact: true })).toBeVisible();
 const copy = page.getByRole('button', { name: 'Copy message', exact: true });
 await copy.focus(); await page.keyboard.press('Space');
 await expect(page.getByText('Failed to copy message', { exact: true })).toBeVisible();
 await expect(copy).toBeVisible();
 await expect(page.getByRole('button', { name: 'Copied', exact: true })).toHaveCount(0);
});
