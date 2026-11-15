import { test, expect } from '@playwright/test';
test('task list exposes each status in its accessible content', async ({ page }) => {
 await page.goto('/?task-list');
 const items = page.getByRole('listitem');
 await expect(items).toHaveCount(4);
 console.log('Expanded task accessibility snapshot:', await page.getByRole('list').ariaSnapshot());
 for (const [index, status] of ['Pending', 'In progress', 'Completed', 'Cancelled'].entries()) {
  await expect.poll(() => items.nth(index).ariaSnapshot()).toContain(status);
 }
});
test('collapsed tasks leave the accessibility tree and return on keyboard expansion', async ({ page }) => {
 await page.goto('/?task-list');
 const toggle = page.getByRole('button', { name: 'Tasks', exact: true });
 await toggle.focus(); await page.keyboard.press('Enter');
 await expect(page.getByRole('button', { name: 'Second task', exact: true })).toHaveAttribute('aria-expanded', 'false');
 console.log('Collapsed task accessibility snapshot:', await page.locator('main').ariaSnapshot());
 await expect(page.getByRole('listitem')).toHaveCount(0);
 await page.keyboard.press('Space');
 await expect(page.getByRole('button', { name: 'Tasks', exact: true })).toHaveAttribute('aria-expanded', 'true');
 await expect(page.getByRole('listitem')).toHaveCount(4);
});
