import type { Page } from '@playwright/test';
import { ADMIN, PUBLIC, REAL_ART, expect, test } from './support';

// Synthetic fixtures (e2e/fixtures/boss): Carling, MaleficStar, Kalos, BM, FA
// have entry art; Limbo has a portrait but no art; Baldrix, Bellona, Jupiter,
// Seren have nothing. Skipped under the real-art capture mode.
test.skip(REAL_ART, 'fixture-specific assertions');

async function noBrokenImages(page: Page) {
  const broken = await page.locator('img').evaluateAll((imgs) =>
    (imgs as HTMLImageElement[]).filter((i) => i.complete && i.naturalWidth === 0).map((i) => i.src),
  );
  expect(broken).toEqual([]);
}

test('admin board: entry art under the veil only where the deployment has it', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  const art = (id: string) => page.locator(`[data-run="${id}"] img.runcard__art`);
  await expect(art('r-carling')).toHaveAttribute('src', '/art/entry/Carling');
  await expect(art('r-kalos')).toHaveAttribute('src', '/art/entry/Kalos');
  await expect(art('r-limbo')).toHaveCount(0);
  await expect(art('r-jupiter')).toHaveCount(0);
  await expect(art('r-carling')).toHaveCSS('opacity', '0.38');
  await expect(art('r-carling')).toHaveAttribute('alt', '');
  await page.evaluate(() => Promise.all([...document.images].map((i) => i.decode().catch(() => {}))));
  await noBrokenImages(page);
});

test('admin run pane: portraits, levels, split artwork, monogram fallback', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const sheet = page.getByRole('complementary', { name: 'HCarling + HStar' });
  await expect(sheet.locator('img.run__art--lead')).toHaveAttribute('src', '/art/entry/Carling');
  await expect(sheet.locator('img.run__art--second')).toHaveAttribute('src', '/art/entry/MaleficStar');
  await expect(sheet.locator('img.portrait')).toHaveCount(2);
  await expect(sheet.locator('img.portrait').first()).toHaveAttribute('src', '/art/icons/Carling');
  await expect(sheet.getByText('Lv. 275')).toBeVisible();
  await expect(sheet.getByText('Lv. 280')).toBeVisible();
  await expect(sheet.locator('.run__cards')).toContainText('morning');
  await noBrokenImages(page);
  await page.keyboard.press('Escape');

  await page.locator('[data-run="r-limbo"] .plan-card__open').click();
  const limbo = page.getByRole('complementary', { name: 'HLimbo' });
  await expect(limbo.locator('img.portrait')).toHaveAttribute('src', '/art/portraits/Limbo');
  await expect(limbo.locator('img.run__art')).toHaveCount(0);
  await page.keyboard.press('Escape');

  await page.locator('[data-run="r-jupiter"] .plan-card__open').click();
  const jupiter = page.getByRole('complementary', { name: 'HJupiter' });
  const mono = jupiter.locator('.portrait--mono');
  await expect(mono).toHaveText('Ju');
  // The hue arrives through CSSOM, not a style attribute.
  expect(await mono.evaluate((el) => (el as HTMLElement).style.getPropertyValue('--mono-hue'))).toBe('170');
  await expect(jupiter.locator('img')).toHaveCount(0);
});

test('public board shows the same art; art routes refuse unknown keys', async ({ page }) => {
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.locator('.runcard img.runcard__art').first()).toBeVisible();
  await noBrokenImages(page);
  for (const path of ['/art/entry/..%2F..%2Fetc%2Fpasswd', '/art/entry/Nope', '/art/secrets/Carling', '/art/entry/Jupiter']) {
    expect((await page.request.get(`${PUBLIC}${path}`)).status(), path).toBe(404);
  }
  const ok = await page.request.get(`${PUBLIC}/art/entry/Carling`);
  expect(ok.headers()['content-type']).toBe('image/png');
  expect(ok.headers()['cache-control']).toBe('public, max-age=3600');
});

// Regression: the batch-3 stylesheet split dropped `runs` from public.scss and
// the entry art rendered raw at full size. Every veil must be an absolute
// layer inside its own card, on both boards, wide and narrow.
for (const [name, origin] of [
  ['public', PUBLIC],
  ['admin', ADMIN],
] as const) {
  for (const width of [1280, 390]) {
    test(`${name} board at ${width}px: every entry-art veil sits inside its card`, async ({ page }) => {
      await page.setViewportSize({ width, height: 844 });
      await page.goto(`${origin}/?sw=off`);
      await expect(page.locator('.runcard__art').first()).toBeAttached();
      const boxes = await page.locator('.runcard__art').evaluateAll((imgs) =>
        imgs.map((img) => {
          const card = img.closest('.runcard')!;
          const a = img.getBoundingClientRect();
          const c = card.getBoundingClientRect();
          return {
            src: (img as HTMLImageElement).getAttribute('src'),
            position: getComputedStyle(img).position,
            inside: a.left >= c.left - 0.5 && a.top >= c.top - 0.5 && a.right <= c.right + 0.5 && a.bottom <= c.bottom + 0.5,
            size: [Math.round(a.width), Math.round(a.height), Math.round(c.width), Math.round(c.height)],
          };
        }),
      );
      expect(boxes.length).toBeGreaterThan(0);
      for (const box of boxes) {
        expect(box.position, `${box.src}`).toBe('absolute');
        expect(box.inside, `${box.src} ${box.size.join('×')}`).toBe(true);
      }
    });
  }
}
