import { ADMIN, expect, test } from './support';

test('code text, the palette and the answers chart use Maple Mono, self-hosted', async ({ page }) => {
  const fonts: string[] = [];
  page.on('request', (r) => {
    if (r.resourceType() === 'font') fonts.push(new URL(r.url()).pathname);
  });
  await page.goto(`${ADMIN}/?sw=off`);
  expect(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--mono').trim())).toMatch(/^"Maple Mono",/);
  await page.getByRole('tab', { name: /^Answers/ }).click();
  const day = page.locator('.week-answers__dow').first();
  await expect(day).toBeVisible();
  expect(await day.evaluate((el) => getComputedStyle(el).fontFamily)).toMatch(/^"Maple Mono"/);
  await expect.poll(() => page.evaluate(() => document.fonts.check('12px "Maple Mono"'))).toBe(true);
  await page.keyboard.press('ControlOrMeta+k');
  const input = page.getByRole('dialog', { name: 'Command palette' }).getByRole('combobox');
  await expect(input).toBeVisible();
  expect(await page.locator('.palette__group').first().evaluate((el) => getComputedStyle(el).fontFamily)).toMatch(/^"Maple Mono"/);
  expect(fonts.some((f) => f.includes('maple-mono'))).toBe(true);
  expect(fonts.some((f) => f.includes('sometype'))).toBe(false);
});

test('bold Zilla Slab and Maple Mono are real 700 faces, never synthesised', async ({ page }) => {
  const fonts: string[] = [];
  page.on('request', (r) => {
    if (r.resourceType() === 'font') fonts.push(new URL(r.url()).pathname);
  });
  await page.goto(`${ADMIN}/?sw=off`);
  // The page line's count is bold mono, so the Maple Mono 700 face loads on its own.
  const count = page.locator('.pageline__num').first();
  await expect(count).toBeVisible();
  expect(await count.evaluate((el) => getComputedStyle(el).fontWeight)).toBe('700');
  const loaded700 = (family: string) =>
    page.evaluate(
      (name) => [...document.fonts].some((f) => f.family.replace(/"/g, '') === name && f.weight === '700' && f.style === 'normal' && f.status === 'loaded'),
      family,
    );
  await expect.poll(() => loaded700('Maple Mono')).toBe(true);
  // A 700 request resolves to a declared 700 face (with none, the browser would pick 600 and embolden it).
  const zilla = await page.evaluate(async () => (await document.fonts.load('700 16px "Zilla Slab"')).map((f) => f.weight));
  expect(zilla).toContain('700');
  await expect.poll(() => loaded700('Zilla Slab')).toBe(true);
  expect(fonts.some((f) => /maple-mono-latin-700-normal/.test(f))).toBe(true);
  expect(fonts.some((f) => /zilla-slab-latin-700-normal/.test(f))).toBe(true);
});
