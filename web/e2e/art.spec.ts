import type { Page } from '@playwright/test';
import type { Week } from '@kanade/api-types';
import { ADMIN, PUBLIC, REAL_ART, expect, signInPublic, test, unconditional } from './support';

// Synthetic fixtures (e2e/fixtures/boss): Carling, MaleficStar, Kalos, BM, FA
// have entry art, MaleficStar also a clip (artwork/animated); Limbo has a
// portrait but no art; Baldrix, Bellona, Jupiter, Seren have nothing. Skipped
// under the real-art capture mode.
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

test('admin board: a lead boss with a clip plays it over the still poster; reduced motion keeps the still', async ({ page }) => {
  // Lead r-carling with HStar, the fixture boss that has a clip.
  await page.route(`${ADMIN}/api/admin/week*`, async (route) => {
    const response = await route.fetch(unconditional(route));
    const week = (await response.json()) as Week;
    const runs = week.runs.map((r) => (r.id === 'r-carling' ? { ...r, bosses: [...r.bosses].reverse() } : r));
    await route.fulfill({ response, json: { ...week, runs } });
  });
  await page.goto(`${ADMIN}/?sw=off`);
  const card = page.locator('[data-run="r-carling"]');
  const video = card.locator('video.runcard__art');
  await expect(video).toHaveAttribute('src', '/art/animated/MaleficStar');
  await expect(video).toHaveAttribute('poster', '/art/entry/MaleficStar');
  await expect(video).toHaveAttribute('aria-hidden', 'true');
  await expect(video).toHaveAttribute('preload', 'metadata');
  await expect(card.locator('img.runcard__art')).toHaveCount(0);
  expect(await video.evaluate((v: HTMLVideoElement) => ({ muted: v.muted, loop: v.loop, playsInline: v.playsInline, controls: v.controls }))).toEqual({ muted: true, loop: true, playsInline: true, controls: false });
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => !v.paused && v.currentTime > 0)).toBe(true);
  // The same veil as the still, and no style attribute (strict CSP).
  await expect(video).toHaveCSS('opacity', '0.38');
  expect(await video.getAttribute('style')).toBeNull();
  // A lead boss without a clip keeps its still.
  await expect(page.locator('[data-run="r-kalos"] img.runcard__art')).toHaveAttribute('src', '/art/entry/Kalos');

  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect(card.locator('img.runcard__art')).toHaveAttribute('src', '/art/entry/MaleficStar');
  await expect(card.locator('video')).toHaveCount(0);
});

test('admin run pane: portraits, levels, split artwork, monogram fallback', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const sheet = page.getByRole('complementary', { name: 'HCarling + HStar' });
  // Two bosses: two angled slices in the identity card, the lead first; HStar has a clip.
  await expect(sheet.locator('.week-pane__art .run__slice:nth-child(1) img.run__art')).toHaveAttribute('src', '/art/entry/Carling');
  const clip = sheet.locator('.week-pane__art .run__slice:nth-child(2) video.run__art');
  await expect(clip).toHaveAttribute('src', '/art/animated/MaleficStar');
  await expect(clip).toHaveAttribute('poster', '/art/entry/MaleficStar');
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

test('art routes refuse unknown keys; the public origin serves art only behind the member session, never stored', async ({ page }) => {
  for (const path of ['/art/entry/..%2F..%2Fetc%2Fpasswd', '/art/entry/Nope', '/art/secrets/Carling', '/art/entry/Jupiter']) {
    expect((await page.request.get(`${ADMIN}${path}`)).status(), path).toBe(404);
  }
  const ok = await page.request.get(`${ADMIN}/art/entry/Carling`);
  expect(ok.headers()['content-type']).toBe('image/png');
  expect(ok.headers()['cache-control']).toBe('public, max-age=3600');
  // Signed out, the public origin refuses art (and the refusal is never stored).
  const anonymous = await page.request.get(`${PUBLIC}/art/entry/Carling`);
  expect(anonymous.status()).toBe(401);
  expect(anonymous.headers()['cache-control']).toBe('no-store');
  expect(await anonymous.json()).toMatchObject({ error: 'unauthenticated' });
  // Signed in: the image, per-user and revalidated on every use; unknown keys still 404.
  await signInPublic(page);
  const member = await page.request.get(`${PUBLIC}/art/entry/Carling`);
  expect(member.status()).toBe(200);
  expect(member.headers()['content-type']).toBe('image/png');
  expect(member.headers()['cache-control']).toBe('private, no-cache');
  expect((await page.request.get(`${PUBLIC}/art/entry/Nope`)).status()).toBe(404);
});

// Regression: the batch-3 stylesheet split dropped `runs` from public.scss and
// the entry art rendered raw at full size. Every veil must be an absolute
// layer inside its own card, wide and narrow.
for (const width of [1280, 390]) {
  test(`admin board at ${width}px: every entry-art veil sits inside its card`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${ADMIN}/?sw=off`);
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
