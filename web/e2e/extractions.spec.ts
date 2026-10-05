import { ADMIN, expect, test } from './support';

// Extractions on a phone: the list, then the call; the browser's Back and the
// top bar's "‹ Extractions" both return to the list with focus on the row.

test('extractions on a phone: Back closes the call and focus returns to its row', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/extractions?sw=off`);
  const list = page.getByRole('listbox', { name: /Extraction calls/ });
  const option = list.getByRole('option').filter({ hasText: 'kalos-four' }).filter({ hasText: '1 change' });
  await expect(option).toBeVisible();
  const optionId = (await option.getAttribute('id'))!;

  await option.click();
  await expect(page).toHaveURL(`${ADMIN}/extractions?call=x-kalos`);
  await expect(list).toBeHidden();
  await expect(page.getByText('kalos 10pm instead?')).toBeVisible();

  // The browser's Back closes the call rather than leaving the page.
  await page.goBack();
  await expect(page).toHaveURL(`${ADMIN}/extractions?sw=off`);
  await expect(list).toBeVisible();
  await expect(list).toBeFocused();
  await expect(list).toHaveAttribute('aria-activedescendant', optionId);

  // The top bar's back step pops the same entry: one more Back leaves Extractions.
  await option.click();
  await page.getByRole('button', { name: 'Back to the list (Extractions)' }).click();
  await expect(page).toHaveURL(`${ADMIN}/extractions?sw=off`);
  await expect(list).toBeFocused();
  await page.goBack();
  await expect(page).not.toHaveURL(/\/extractions/);
});

test('extractions on a wide screen: picking a call replaces the entry', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/chat?sw=off`);
  await page.goto(`${ADMIN}/extractions?sw=off`);
  await page.getByRole('option').filter({ hasText: 'kalos-four' }).filter({ hasText: '1 change' }).click();
  await expect(page).toHaveURL(`${ADMIN}/extractions?call=x-kalos`);
  await page.goBack();
  await expect(page).toHaveURL(`${ADMIN}/chat?sw=off`);
});

test('extractions Raw tab: the response pretty-printed in the code viewer', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/extractions?call=x-kalos&sw=off`);
  await page.getByRole('tab', { name: 'Raw' }).click();
  const panel = page.getByRole('tabpanel', { name: 'Raw' });
  await expect(panel).toBeVisible();
  await expect(panel.getByRole('heading', { name: 'Raw response' })).toBeVisible();
  // Pretty-printed: a space after each key's colon, which the stored compact JSON lacks.
  await expect(panel).toContainText('"amendments": [');
  await expect(panel.getByRole('button', { name: 'Copy' })).toBeVisible();
});
