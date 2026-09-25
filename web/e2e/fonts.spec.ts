import { ADMIN, expect, test } from './support';

test('code text, the palette and the chart use Maple Mono, self-hosted', async ({ page }) => {
  const fonts: string[] = [];
  page.on('request', (r) => {
    if (r.resourceType() === 'font') fonts.push(new URL(r.url()).pathname);
  });
  await page.goto(`${ADMIN}/?sw=off`);
  expect(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--mono').trim())).toMatch(/^"Maple Mono",/);
  await page.getByRole('tab', { name: 'Answers' }).click();
  await page.locator('.chart__canvas canvas').waitFor();
  // Canvas text requests no font itself; the chart loads it and redraws.
  await expect.poll(() => page.evaluate(() => document.fonts.check('12px "Maple Mono"'))).toBe(true);
  await page.keyboard.press('ControlOrMeta+k');
  const input = page.getByRole('dialog', { name: 'Command palette' }).getByRole('combobox');
  await expect(input).toBeVisible();
  expect(await page.locator('.palette__group').first().evaluate((el) => getComputedStyle(el).fontFamily)).toMatch(/^"Maple Mono"/);
  expect(fonts.some((f) => f.includes('maple-mono'))).toBe(true);
  expect(fonts.some((f) => f.includes('sometype'))).toBe(false);
});
