import { ADMIN, expect, test } from './support';

// The shared "Filters (n)" popover on the Chat and Extractions title bars:
// Escape returns to its button; a press outside closes it; Clear does not.

for (const [path, query, search] of [
  ['/chat', 'outcome=timeout,error', 'Search interactions'],
  ['/extractions', 'outcome=proposed', 'Search calls'],
] as const) {
  test(`log filters on ${path}: Escape and an outside press close the panel`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${ADMIN}${path}?${query}&sw=off`);
    const toggle = page.getByRole('button', { name: 'Filters (1)' });
    const panel = page.getByRole('group', { name: 'Filters' });
    await expect(toggle.locator('svg')).toHaveCount(1);

    await toggle.click();
    // The Dates trigger sits in the filter row beside "Filters (n)" (P_Dates), in the title bar's ink.
    const ink = (el: Element) => getComputedStyle(el).color;
    await expect(panel.getByRole('button', { name: /^Dates/ })).toHaveCount(0);
    expect(await page.getByRole('button', { name: /^Dates/ }).evaluate(ink)).toEqual(await toggle.evaluate(ink));
    await panel.getByLabel('Model').focus();
    await page.keyboard.press('Escape');
    await expect(panel).toBeHidden();
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await expect(toggle).toBeFocused();

    // Clear sits in the bar, so the panel stays; the search box is outside.
    await toggle.click();
    await page.getByRole('button', { name: 'Clear' }).click();
    const cleared = page.getByRole('button', { name: 'Filters (0)' });
    await expect(cleared).toHaveAttribute('aria-expanded', 'true');
    await page.getByRole('searchbox', { name: search }).click();
    await expect(panel).toBeHidden();
    await expect(cleared).toHaveAttribute('aria-expanded', 'false');
    await expect(page.getByRole('searchbox', { name: search })).toBeFocused();
  });
}

// Phones: the panel hangs from the title bar inside the window and the viewport, with no sideways scroll.
for (const [path, query, windowName] of [
  ['/chat', 'outcome=timeout,error', 'Interactions'],
  ['/extractions', 'outcome=proposed', 'Calls'],
] as const) {
  for (const [width, height] of [
    [390, 844],
    [360, 640],
  ] as const) {
    test(`log filters on ${path} at ${width} px: the panel stays inside the window`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      await page.goto(`${ADMIN}${path}?${query}&sw=off`);
      await page.getByRole('button', { name: 'Filters (1)' }).click();
      const panel = (await page.getByRole('group', { name: 'Filters' }).boundingBox())!;
      const win = (await page.getByRole('region', { name: windowName, exact: true }).boundingBox())!;
      expect(panel.x).toBeGreaterThanOrEqual(Math.max(0, win.x));
      expect(panel.x + panel.width).toBeLessThanOrEqual(Math.min(width, win.x + win.width));
      expect(panel.y).toBeGreaterThanOrEqual(win.y);
      expect(panel.y + panel.height).toBeLessThanOrEqual(Math.min(height, win.y + win.height));
      expect(await page.evaluate(() => document.scrollingElement!.scrollWidth <= innerWidth)).toBe(true);
    });
  }
}

// Phones: the filter row wraps inside the title bar; no chip or Clear is cut or clipped.
for (const [path, windowName, queries] of [
  ['/chat', 'Interactions', ['outcome=timeout,error', 'outcome=timeout,error&model=kanata/chat']],
  ['/extractions', 'Calls', ['outcome=proposed', 'outcome=proposed,failed,self_service_link&model=kanata/legacy']],
] as const) {
  for (const width of [390, 360]) {
    test(`log filters on ${path} at ${width} px: every chip and Clear sit inside the window`, async ({ page }) => {
      await page.setViewportSize({ width, height: 760 });
      for (const [i, query] of queries.entries()) {
        await page.goto(`${ADMIN}${path}?${query}&sw=off`);
        const chips = page.getByRole('button', { name: / — remove$/ });
        await expect(chips).toHaveCount(i + 1);
        const win = (await page.getByRole('region', { name: windowName, exact: true }).boundingBox())!;
        for (const button of [...(await chips.all()), page.getByRole('button', { name: 'Clear', exact: true })]) {
          const box = (await button.boundingBox())!;
          expect(box.x).toBeGreaterThanOrEqual(Math.max(0, win.x));
          expect(box.x + box.width).toBeLessThanOrEqual(Math.min(width, win.x + win.width));
          expect(box.y + box.height).toBeLessThanOrEqual(win.y + win.height);
          expect(await button.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
        }
        expect(await page.evaluate(() => document.scrollingElement!.scrollWidth <= innerWidth)).toBe(true);
      }
    });
  }
}
