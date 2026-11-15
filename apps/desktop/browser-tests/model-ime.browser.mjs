import { test, expect } from '@playwright/test';
async function openPicker(page) {
 await page.goto('/?picker=model');
 await page.getByRole('button', { name: 'Choose model', exact: true }).click();
 const search = page.getByPlaceholder('Search models or providers…');
 await expect(search).toBeFocused();
 await expect(page.getByRole('button', { name: 'alpha, Local', exact: true })).toBeVisible();
 return search;
}
test('model picker ordinary Enter selects the first result', async ({ page }) => {
 const search = await openPicker(page);
 await search.press('Enter');
 await expect(page.getByLabel('Selection', { exact: true })).toHaveText('alpha');
 await expect(search).toHaveCount(0);
});
for (const mode of ['composing', 'legacy229']) test(`model picker ignores ${mode} Enter then accepts ordinary Enter`, async ({ page }) => {
 const search = await openPicker(page);
 // Synthetic DOM composition events in real Chromium, not native OS IME automation.
 await search.evaluate((input, mode) => {
  input.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true, data: '' }));
  input.dispatchEvent(new KeyboardEvent('keydown', {
   bubbles: true, cancelable: true, key: 'Enter', code: 'Enter',
   isComposing: mode === 'composing', keyCode: mode === 'legacy229' ? 229 : 13,
  }));
 }, mode);
 await expect(page.getByLabel('Selection', { exact: true })).toHaveText('');
 await expect(search).toBeVisible();
 await expect(search).toBeFocused();
 await search.evaluate(input => input.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true, data: '' })));
 await search.press('Enter');
 await expect(page.getByLabel('Selection', { exact: true })).toHaveText('alpha');
 await expect(search).toHaveCount(0);
});
