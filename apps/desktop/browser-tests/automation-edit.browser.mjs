import { test, expect } from '@playwright/test';
for (const width of [360, 1000]) test(`scheduled edit keyboard access and stable countdown at ${width}px`, async ({ page }) => {
 await page.setViewportSize({ width, height: 600 });
 await page.goto('/?automation-edit');
 const row = page.getByRole('button', { name: /Scheduled review/ });
 const edit = page.getByRole('button', { name: 'Edit automation', exact: true });
 const countdown = page.getByText(/^Starts in /);
 await expect(countdown).toBeVisible();
 await page.keyboard.press('Tab'); await expect(row).toBeFocused();
 await page.keyboard.press('Tab'); await expect(edit).toBeFocused();
 await expect(edit).toHaveCSS('opacity', '1');
 for (const [i, key] of ['Enter', 'Space'].entries()) {
  await page.keyboard.press(key);
  await expect(page.getByLabel('Edit count')).toHaveText(String(i + 1));
  await expect(page.getByLabel('Navigation count')).toHaveText('0');
 }
 await expect(page.locator('button button')).toHaveCount(0);
 const bounds = await row.boundingBox();
 await row.hover(); await expect(countdown).toBeVisible();
 expect(await row.boundingBox()).toEqual(bounds);
 const editBounds = await edit.boundingBox();
 expect(editBounds.x + editBounds.width).toBeLessThanOrEqual(width);
 expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
 await edit.focus(); await page.keyboard.press('Shift+F10');
 await expect(page.getByRole('menuitem', { name: 'Edit', exact: true })).toBeVisible();
 await page.keyboard.press('Escape'); await expect(edit).toBeFocused();
 await row.focus(); await page.keyboard.press('Enter');
 await expect(page.getByLabel('Navigation count')).toHaveText('1');
 await page.screenshot({ path: `test-results/automation-edit-${width}.png` });
});
