import { test, expect } from '@playwright/test';
async function openPicker(page) {
 await page.goto('/?skill-ime');
 await page.getByRole('button', { name: 'Choose skill', exact: true }).click();
 const search = page.getByPlaceholder('Search skills...');
 await expect(search).toBeFocused();
 await expect(page.getByRole('button', { name: 'alpha First skill' })).toBeVisible();
 return search;
}
test('skill picker ordinary keyboard selection remains reachable', async ({ page }) => {
 const search = await openPicker(page);
 await search.press('ArrowDown');
 await search.press('Enter');
 await expect(page.getByLabel('Selections')).toHaveText('["beta"]');
 await expect(page.getByRole('dialog')).toHaveCount(0);
});
for (const mode of ['composing', 'legacy229']) test(`skill picker ignores ${mode} Enter then accepts ordinary Enter`, async ({ page }) => {
 const search = await openPicker(page);
 // Browser component event regression, not native OS input-method automation.
 await search.evaluate((input, mode) => {
  input.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true, data: '' }));
  input.dispatchEvent(new KeyboardEvent('keydown', {
   bubbles: true, cancelable: true, key: 'Enter', code: 'Enter',
   isComposing: mode === 'composing', keyCode: mode === 'legacy229' ? 229 : 13,
  }));
 }, mode);
 await expect(page.getByLabel('Selections')).toHaveText('[]');
 await expect(page.getByRole('dialog')).toBeVisible();
 await expect(search).toBeFocused();
 await search.evaluate(input => input.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true, data: '' })));
 await search.press('Enter');
 await expect(page.getByLabel('Selections')).toHaveText('["alpha"]');
 await expect(page.getByRole('dialog')).toHaveCount(0);
});
