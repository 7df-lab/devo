import { test, expect } from '@playwright/test';
test('successful empty skills are not reported as a load failure', async ({ page }) => {
 await page.goto('/?skill-error&mode=empty');
 await page.getByRole('button', { name: 'Choose skill', exact: true }).click();
 await expect(page.getByText('No skills available', { exact: true })).toBeVisible();
 await expect(page.getByText('Failed to load skills', { exact: true })).toHaveCount(0);
 await expect(page.getByRole('button', { name: 'Retry', exact: true })).toHaveCount(0);
});
test('failed skill request is distinct from empty and retries successfully', async ({ page }) => {
 await page.goto('/?skill-error&mode=failure');
 await page.getByRole('button', { name: 'Choose skill', exact: true }).click();
 // Production QueryClient retries the initial transport failure once before showing an error.
 await expect(page.getByText('Failed to load skills', { exact: true })).toBeVisible();
 await expect(page.getByText('No skills available', { exact: true })).toHaveCount(0);
 const retry = page.getByRole('button', { name: 'Retry', exact: true });
 await retry.focus(); await page.keyboard.press('Enter');
 await expect(page.getByRole('button', { name: 'alpha Recovered skill' })).toBeVisible();
 await expect(page.getByText('Failed to load skills', { exact: true })).toHaveCount(0);
 const search = page.getByPlaceholder('Search skills...');
 await search.focus(); await search.press('Enter');
 await expect(page.getByLabel('Selected skill')).toHaveText('alpha');
 await expect(page.getByRole('dialog')).toHaveCount(0);
});
