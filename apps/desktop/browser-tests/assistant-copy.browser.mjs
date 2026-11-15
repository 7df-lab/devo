import { test, expect } from '@playwright/test';
const response = 'Exact assistant response: 项目 review';
async function openResponse(page) {
 await page.goto('/?assistant-copy');
 await expect(page.getByText(response, { exact: true })).toBeVisible();
 // Reveal the existing hover toolbar; keyboard-only discoverability is separate.
 await page.locator('[data-turn-id="turn"]').hover();
 return page.getByRole('button', { name: 'Copy', exact: true });
}
test('assistant copy writes exact text and shows success', async ({ page, context }) => {
 await context.grantPermissions(['clipboard-read', 'clipboard-write']);
 const copy = await openResponse(page);
 await copy.focus(); await page.keyboard.press('Enter');
 await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(response);
 await expect(page.getByRole('button', { name: 'Copied', exact: true })).toBeVisible();
 await expect(copy).toBeVisible({ timeout: 5000 });
});
for (const mode of ['rejected', 'missing']) test(`assistant clipboard ${mode} reports failure without false success`, async ({ page }) => {
 await page.addInitScript(mode => Object.defineProperty(navigator, 'clipboard', { configurable: true,
  value: mode === 'missing' ? undefined : { writeText: async () => { throw new Error('Denied'); } } }), mode);
 const copy = await openResponse(page);
 await copy.focus(); await page.keyboard.press('Space');
 await expect(page.getByText('Failed to copy response', { exact: true })).toBeVisible();
 await expect(copy).toBeVisible();
 await expect(page.getByRole('button', { name: 'Copied', exact: true })).toHaveCount(0);
});
