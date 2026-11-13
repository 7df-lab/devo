import { test, expect } from '@playwright/test';
const rowName = /Daily review/;
test('keyboard discovers copy and copies without opening the run', async ({ page, context }) => {
 await context.grantPermissions(['clipboard-read', 'clipboard-write']);
 await page.goto('/');
 const row = page.getByRole('button', { name: rowName });
 const copy = page.getByRole('button', { name: 'Copy summary', exact: true });
 await page.keyboard.press('Tab'); await expect(row).toBeFocused();
 await page.keyboard.press('Tab'); await expect(copy).toBeFocused();
 await expect(copy).toHaveCSS('opacity', '1');
 for (const key of ['Enter', 'Space']) {
  await page.evaluate(key => navigator.clipboard.writeText('before-' + key), key);
  await page.keyboard.press(key);
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain('A long automation summary');
  await expect(page.getByLabel('Opened')).toHaveText('0');
 }
 await page.keyboard.press('Shift+Tab'); await expect(row).toBeFocused();
 await page.keyboard.press('Enter'); await expect(page.getByLabel('Opened')).toHaveText('1');
 await expect(page.locator('button button')).toHaveCount(0);
});
test('menu supports keyboard, dismissal focus and actions', async ({ page }) => {
 await page.goto('/'); const row = page.getByRole('button', { name: rowName });
 await row.focus(); await page.keyboard.press('Shift+F10');
 await expect(page.getByRole('menuitem', { name: 'Mark as read' })).toBeVisible();
 await page.keyboard.press('Escape'); await expect(row).toBeFocused();
 await row.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Mark as read' }).click();
 await expect(page.getByLabel('Marked')).toHaveText('1'); await expect(page.getByLabel('Opened')).toHaveText('0');
 await row.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Archive', exact: true }).click();
 await expect(page.getByLabel('Archived')).toHaveText('1');
 await row.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Open thread' }).click();
 await expect(page.getByLabel('Opened')).toHaveText('1');
 await page.getByRole('button', { name: 'Copy summary' }).focus(); await page.keyboard.press('Shift+F10');
 await expect(page.getByRole('menuitem', { name: 'Open thread' })).toBeVisible();
 await page.keyboard.press('Escape'); await expect(page.getByRole('button', { name: 'Copy summary' })).toBeFocused();
});
for (const mode of ['rejected', 'missing']) test(`clipboard ${mode} is reported`, async ({ page }) => {
 await page.addInitScript(mode => Object.defineProperty(navigator, 'clipboard', { configurable: true,
  value: mode === 'missing' ? undefined : { writeText: async () => { throw new Error('Denied'); } } }), mode);
 await page.goto('/'); await page.getByRole('button', { name: 'Copy summary' }).focus();
 await page.keyboard.press('Enter'); await expect(page.getByText('Failed to copy summary')).toBeVisible();
 await expect(page.getByLabel('Opened')).toHaveText('0');
});
for (const variant of ['empty', 'archived']) test(`${variant} omits copy`, async ({ page }) => {
 await page.goto(`/?variant=${variant}`); await expect(page.getByRole('button', { name: rowName })).toBeVisible();
 await expect(page.getByRole('button', { name: 'Copy summary' })).toHaveCount(0);
});
for (const width of [360, 1000]) test(`stable layout at ${width}px`, async ({ page }) => {
 await page.setViewportSize({ width, height: 600 }); await page.goto('/');
 const row = page.getByRole('button', { name: rowName }); const copy = page.getByRole('button', { name: 'Copy summary' });
 const before = await row.boundingBox(); await row.hover(); await expect(copy).toHaveCSS('opacity', '1');
 expect(await row.boundingBox()).toEqual(before);
 await copy.focus(); const bounds = await copy.boundingBox(); expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
 expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
 await page.screenshot({ path: `test-results/inbox-${width}.png` });
});
