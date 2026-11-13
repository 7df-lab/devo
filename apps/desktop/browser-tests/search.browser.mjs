import { test, expect } from '@playwright/test';
for (const picker of ['branch', 'model']) {
 test(`${picker}: clear is keyboard accessible without selecting or closing`, async ({page}) => {
  await page.goto('/?picker=' + picker);
  await page.getByRole('button', {name: 'Choose ' + picker, exact:true}).click();
  const input = page.getByRole('textbox');
  const clear = page.getByRole('button', {name:'Clear search', exact:true});
  await expect(input).toBeFocused();
  await expect(input).toHaveAccessibleName(picker === 'branch' ? 'Search branches...' : 'Search models or providers…');
  await expect(clear).toHaveCount(0);
  for (const key of ['Enter', 'Space']) {
   await input.fill(key === 'Enter' ? 'alpha' : 'missing');
   await page.keyboard.press('Tab'); await expect(clear).toBeFocused();
   await page.keyboard.press(key);
   await expect(input).toHaveValue(''); await expect(input).toBeFocused();
   await expect(clear).toHaveCount(0);
   await expect(page.getByLabel('Selection')).toHaveText('');
   await expect(page.locator('[data-slot="searchable-list-popover-content"]')).toBeVisible();
  }
  await input.fill('beta');
  await page.getByRole('button', {name: picker === 'model' ? 'beta, Local' : 'beta', exact:true}).click();
  await expect(page.getByLabel('Selection')).toHaveText('beta');
  await expect(input).toHaveCount(0);
 });
 test(`${picker}: close/reopen resets search and focus`, async ({page}) => {
  await page.goto('/?picker=' + picker);
  const trigger=page.getByRole('button',{name:'Choose ' + picker,exact:true});
  await trigger.click(); await page.getByRole('textbox').fill('alpha');
  await page.keyboard.press('Escape'); await expect(trigger).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('textbox')).toBeFocused(); await expect(page.getByRole('textbox')).toHaveValue('');
  await page.getByRole('textbox').fill('missing');
  await page.getByRole('button',{name:'Outside',exact:true}).click();
  await expect(page.getByRole('textbox')).toHaveCount(0);
  await trigger.click(); await expect(page.getByRole('textbox')).toHaveValue('');
 });
 for (const width of [360,1000]) test(`${picker}: popup and clear fit ${width}px`, async ({page}, testInfo) => {
  await page.setViewportSize({width,height:700}); await page.goto('/?picker=' + picker);
  await page.getByRole('button',{name:'Choose ' + picker,exact:true}).click();
  const input=page.getByRole('textbox');
  const popupElement=page.locator('[data-slot="searchable-list-popover-content"]');
  await expect(popupElement).toBeVisible();
  // Bounding boxes include the opening zoom transform. Measure only after
  // actual animations settle; retain animations and the strict width assertion.
  await popupElement.evaluate(async element => {
   await Promise.all(element.getAnimations({subtree:true}).map(animation => animation.finished));
  });
  const before=await input.boundingBox();
  await input.fill('a'.repeat(100));
  const clear=page.getByRole('button',{name:'Clear search',exact:true});
  await expect(clear).toBeVisible();
  const after=await input.boundingBox(); const popup=await page.locator('[data-slot="searchable-list-popover-content"]').boundingBox();
  expect(Math.abs(before.width-after.width)).toBeLessThan(1);
  expect(popup.x).toBeGreaterThanOrEqual(0); expect(popup.x+popup.width).toBeLessThanOrEqual(width);
  await page.keyboard.press('Tab'); await expect(clear).toBeFocused();
  await page.screenshot({path:testInfo.outputPath(`search-${picker}-${width}.png`)});
  await clear.click(); await expect(input).toBeFocused(); await expect(input).toHaveValue('');
 });
}
