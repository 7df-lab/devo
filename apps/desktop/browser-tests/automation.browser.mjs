import { test, expect } from '@playwright/test';
const windows = 'C:\\Users\\jack\\WindowsProject\\';
const unix = '/repo/UnixProject/';
const discovered = 'C:\\Users\\jack\\DiscoveredProject\\';
for (const width of [360, 1000]) test(`automation project labels render in real components at ${width}px`, async ({ page }) => {
 await page.setViewportSize({ width, height: 900 });
 await page.goto('/?automation');
 const inbox = page.getByRole('region', { name: 'Automation inbox' });
 await expect(inbox.getByText('WindowsProject, UnixProject', { exact: true })).toBeVisible();
 await expect(inbox.getByText('WindowsProject', { exact: true })).toBeVisible();
 await expect(inbox.getByText('UnixProject', { exact: true })).toBeVisible();
 const detail = page.getByRole('region', { name: 'Automation detail' });
 await expect(detail.getByText('WindowsProject', { exact: true })).toBeVisible();
 await expect(detail.getByText('UnixProject', { exact: true })).toBeVisible();
 await page.screenshot({ path: `test-results/automation-labels-${width}.png` });
 await page.getByRole('button', { name: 'Edit label fixture' }).click();
 const dialog = page.getByRole('dialog');
 await expect(dialog.getByRole('button', { name: 'Remove WindowsProject', exact: true })).toBeVisible();
 await expect(dialog.getByRole('button', { name: 'Remove UnixProject', exact: true })).toBeVisible();
 await dialog.getByPlaceholder('Search projects...').click();
 const option = page.getByRole('option').filter({ hasText: 'DiscoveredProject' });
 await expect(option.getByText('DiscoveredProject', { exact: true })).toBeVisible();
 await expect(option.getByText(discovered, { exact: true })).toBeVisible();
 await option.click();
 await expect(dialog.getByRole('button', { name: 'Remove DiscoveredProject', exact: true })).toBeVisible();
 await dialog.getByRole('button', { name: 'Save', exact: true }).click();
 await expect(dialog).not.toBeVisible();
 await expect.poll(async () => JSON.parse(await page.getByLabel('Saved payload').textContent()).workspaces).toEqual([windows, unix, discovered]);
 // Reopen from unchanged backing data: remove by compact label, preserve the
 // other exact path in the submitted payload (not just its display basename).
 await page.getByRole('button', { name: 'Edit label fixture' }).click();
 await dialog.getByRole('button', { name: 'Remove WindowsProject', exact: true }).click();
 await dialog.getByRole('button', { name: 'Save', exact: true }).click();
 await expect.poll(async () => JSON.parse(await page.getByLabel('Saved payload').textContent()).workspaces).toEqual([unix]);
});
