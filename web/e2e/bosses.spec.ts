import type { Page } from '@playwright/test';
import { ADMIN, REAL_ART, expect, test } from './support';

// Event bosses (knowledge documents with an `event`, outside the catalog: Kai,
// Meilin in the tracked boss/knowledge): art like catalog rows and the
// "Seasonal boss" label. The synthetic fixtures carry no event art, so the
// image path is driven through the events read with fixture art.

async function go(page: Page, path: string) {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

const SEASON_3 = 'Seasonal boss · CW3';
const SEASON_4 = 'Seasonal boss · CW4';

test.describe('event bosses', () => {
  test.skip(REAL_ART, 'fixture-specific assertions');

  test('the Bosses page labels event rows by season and only them', async ({ page }) => {
    await go(page, '/bosses');
    const events = page.getByRole('list', { name: 'Event bosses' });
    const kai = events.getByRole('listitem').filter({ has: page.getByRole('link', { name: 'Kai', exact: true }) });
    const meilin = events.getByRole('listitem').filter({ has: page.getByRole('link', { name: 'Meilin', exact: true }) });
    await expect(kai.locator('.status-chip')).toHaveText(SEASON_3);
    await expect(meilin.locator('.status-chip')).toHaveText(SEASON_4);
    // The short tag keeps the event's full name from the data.
    await expect(kai.locator('.status-chip abbr')).toHaveAttribute('title', 'Challengers World Season 3');
    // No fixture art for event keys: the monogram holds the same box.
    await expect(kai.locator('.portrait--mono')).toHaveText('Ka');
    await expect(page.getByRole('list', { name: 'Bosses', exact: true })).not.toContainText('Seasonal boss');
    // Event art resolves only through the declared, exact-case key.
    expect((await page.request.get(`${ADMIN}/art/portraits/kai`)).status()).toBe(404);
  });

  test('an event row renders its art as a catalog row does', async ({ page }) => {
    await page.route(/\/api\/admin\/bosses\/events$/, async (route) => {
      const response = await route.fetch();
      const rows = (await response.json()) as { key: string; portrait: string | null; portrait_sm: string | null }[];
      for (const row of rows) if (row.key === 'Kai') Object.assign(row, { portrait: null, portrait_sm: '/art/icons/Carling' });
      await route.fulfill({ response, json: rows });
    });
    await go(page, '/bosses');
    const kai = page
      .getByRole('list', { name: 'Event bosses' })
      .getByRole('listitem')
      .filter({ has: page.getByRole('link', { name: 'Kai', exact: true }) });
    // Without a portrait the icon stands in, at the catalog rows' size.
    const img = kai.locator('img.portrait.portrait--md');
    await expect(img).toHaveAttribute('src', '/art/icons/Carling');
    await expect(img).toHaveAttribute('alt', '');
    await img.scrollIntoViewIfNeeded();
    await expect.poll(() => img.evaluate((i) => (i as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  });

  test('a selected event row reveals its difficulties below the name, like catalog rows', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await go(page, '/bosses/Kai/knowledge');
    const kai = page
      .getByRole('list', { name: 'Event bosses' })
      .getByRole('listitem')
      .filter({ has: page.getByRole('link', { name: 'Kai', exact: true }) });
    const ticks = kai.locator('.row-content__full .boss-tick');
    await expect(ticks).toHaveText(['NORMAL', 'HARD']);
    await expect(kai.locator('.row-content__full .bossrow__difficulties > span')).toHaveText(['NORMAL Lv. 270', 'HARD Lv. 280']);
    await expect(kai.locator('.row-content__full .boss-tick--h')).toBeVisible();
    // Let the reveal transition settle before measuring.
    await expect.poll(async () => (await kai.locator('.row-content__clip').boundingBox())!.height, { intervals: [100, 100, 250] }).toBeGreaterThan(20);
    const row = (await kai.boundingBox())!;
    const link = (await kai.locator('a').boundingBox())!;
    for (const tick of await ticks.all()) {
      const part = (await tick.boundingBox())!;
      expect(part.y, 'ticks sit below the name line').toBeGreaterThanOrEqual(link.y + link.height - 1);
      expect(part.x + part.width, 'ticks stay inside the row').toBeLessThanOrEqual(row.x + row.width + 0.5);
    }
  });

  test('an event knowledge page carries the season label; a catalog one does not', async ({ page }) => {
    await go(page, '/bosses/Meilin/knowledge');
    await expect(page.locator('.knowledge-hero .status-chip')).toHaveText(SEASON_4);
    await go(page, '/bosses/MaleficStar/knowledge');
    await expect(page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' })).toBeVisible();
    await expect(page.locator('.knowledge-hero')).not.toContainText('Seasonal boss');
  });
});

test('bosses: phone opens a selected knowledge detail and returns to the catalog', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/bosses');
  await expect(page.getByRole('heading', { level: 1 })).toContainText('bosses');
  await page.getByRole('link', { name: 'Radiant Malefic Star' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' })).toBeVisible();
  await page.getByRole('button', { name: 'Back to the catalog (Bosses)' }).click();
  await expect(page.getByRole('list', { name: 'Bosses', exact: true })).toBeVisible();
});

test('bosses: selected rows reveal every difficulty and detail keeps timing and fact content', async ({ page }) => {
  await go(page, '/bosses/MaleficStar/knowledge');
  const star = page.locator('.bossrow', { hasText: 'Radiant Malefic Star' });
  await expect(star.locator('.row-content__full .boss-tick--h')).toHaveText('HARD');
  await expect(star.locator('.row-content__full .boss-tick--h')).toBeVisible();
  await expect(star.locator('.boss-tick--more')).toBeHidden();
  const catalog = await (await page.request.get(`${ADMIN}/api/admin/bosses`)).json() as { key: string; difficulties: unknown[] }[];
  await expect(star.locator('.row-content__full .boss-tick')).toHaveCount(catalog.find((boss) => boss.key === 'MaleficStar')!.difficulties.length);
  const facts = page.locator('.knowledge-facts');
  await expect(facts).toContainText('HP (total)');
  await expect(facts).not.toContainText('Recommended');
  await expect(page.locator('.knowledge-recommended .cap')).toHaveText('Recommended · hexa-converted stat');
  const timing = page.locator('.knowledge-detail aside li').first();
  await expect(timing.locator('strong')).toHaveText(/^(Mon|Tue|Wed|Thu|Fri|Sat|Sun) \d\d:\d\d$/);
  await expect(timing.locator('.pill')).toHaveText(/^(EASY|NORMAL|HARD|CHAOS|EXTREME)$/);
  await expect(page.locator('.knowledge-aside__count')).toContainText('next');
});

test('bosses: knowledge strategies render when the document has them and not otherwise', async ({ page }) => {
  const knowledge = await (await page.request.get(`${ADMIN}/api/admin/bosses/Lotus/knowledge`)).json() as { doc: { strategies?: { name: string; risk: string; damage: string; when: string; payoff: string; steps: string[] }[] } };
  const expected = knowledge.doc.strategies ?? [];
  expect(expected.length).toBeGreaterThan(0);
  await go(page, '/bosses/Lotus/knowledge');
  const section = page.getByRole('region', { name: 'Strategies' });
  await expect(section).toBeVisible();
  const cards = section.locator('.strategy');
  await expect(cards).toHaveCount(expected.length);
  for (const [index, strategy] of expected.entries()) {
    const card = cards.nth(index);
    await expect(card.getByRole('heading', { level: 3 })).toHaveText(strategy.name);
    await expect(card.locator('.status-chip')).toHaveText([`Risk: ${strategy.risk}`, `Damage needed: ${strategy.damage}`]);
    await expect(card.locator('dd')).toHaveText([strategy.when, strategy.payoff]);
    await expect(card.getByRole('list', { name: `Steps for ${strategy.name}` }).getByRole('listitem')).toHaveCount(strategy.steps.length);
  }
  // Strategies sit between Tips and Notes/Sources.
  const headings = await page.locator('.knowledge-detail__main h2.cap').allTextContents();
  expect(headings.indexOf('Strategies')).toBeGreaterThan(headings.indexOf('Tips'));
  expect(headings.indexOf('Strategies')).toBeLessThan(headings.indexOf('Sources'));

  // Kai's document declares no strategies.
  const kai = await (await page.request.get(`${ADMIN}/api/admin/bosses/Kai/knowledge`)).json() as { doc: { strategies?: unknown[] } };
  expect(kai.doc.strategies ?? []).toHaveLength(0);
  await go(page, '/bosses/Kai/knowledge');
  await expect(page.getByRole('heading', { level: 2, name: 'Sources' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Strategies' })).toHaveCount(0);
});

test('bosses: a whole catalog row selects its boss, not only the name', async ({ page }) => {
  await go(page, '/bosses/MaleficStar/knowledge');
  const other = page.locator('.bosses-list .bossrow').filter({ hasNotText: 'Radiant Malefic Star' }).first();
  const name = (await other.locator('a.bossrow__name').textContent())!.trim();
  // The row's right edge (the ticks), well away from the name link.
  const box = (await other.boundingBox())!;
  await page.mouse.click(box.x + box.width - 4, box.y + box.height / 2);
  await expect(page.getByRole('heading', { level: 2, name })).toBeVisible();
  await expect(other.getByRole('link')).toHaveAttribute('aria-current', 'true');
});

test('bosses: a weekly timing opens that timing in Fixed', async ({ page }) => {
  await go(page, '/bosses/MaleficStar/knowledge');
  const timing = page.locator('.knowledge-detail aside li a').first();
  const when = (await timing.locator('strong').textContent())!.trim();
  await timing.click();
  // Wide, the editor is the side pane beside the list (a sheet only on phones).
  const pane = page.getByRole('complementary', { name: 'Weekly timing details' });
  await expect(pane).toBeVisible();
  await expect(pane).toContainText(when.slice(-5));
  await expect(page).toHaveURL(/\/fixed$/);
});

test('bosses: real catalog portraits and detail art load from the declared asset paths', async ({ page }) => {
  test.skip(!REAL_ART, 'real art only');
  await go(page, '/bosses/MaleficStar/knowledge');
  const rows = page.locator('.bosses-list .bossrow');
  await expect(rows).toHaveCount(11);
  for (const row of await rows.all()) {
    const portrait = row.locator('img.portrait');
    await portrait.scrollIntoViewIfNeeded();
    await expect(portrait).toHaveAttribute('src', /\/art\/portraits\//);
    await expect.poll(() => portrait.evaluate((image) => (image as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  }
  const hero = page.locator('.knowledge-hero');
  const portrait = hero.locator('img.portrait');
  const art = hero.locator('video.knowledge-hero__art');
  await expect(portrait).toHaveAttribute('src', '/art/portraits/MaleficStar');
  await expect(art).toHaveAttribute('src', '/art/animated/MaleficStar');
  await expect(art).toHaveAttribute('poster', '/art/entry/MaleficStar');
  await expect.poll(() => portrait.evaluate((element) => (element as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  await expect.poll(() => art.evaluate((element) => (element as HTMLVideoElement).videoWidth)).toBeGreaterThan(0);
  const kai = page.getByRole('list', { name: 'Event bosses' }).getByRole('listitem').filter({ has: page.getByRole('link', { name: 'Kai', exact: true }) }).locator('img.portrait');
  await kai.scrollIntoViewIfNeeded();
  await expect(kai).toHaveAttribute('src', '/art/portraits/Kai');
  await expect.poll(() => kai.evaluate((image) => (image as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
});

// Review captures with the private art (KANADE_REAL_ART=1), git-ignored.
test('capture event bosses with real art', async ({ page }) => {
  test.skip(!REAL_ART, 'real art only');
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.addInitScript(() => {
    localStorage.setItem('colorway', 'blossom');
    localStorage.setItem('theme', 'light');
  });
  const settle = () =>
    page.evaluate(async () => {
      await document.fonts.ready;
      await Promise.all([...document.images].map((i) => i.decode().catch(() => {})));
    });
  await go(page, '/bosses');
  const events = page.getByRole('list', { name: 'Event bosses' });
  await expect(events.locator('.status-chip').first()).toBeVisible();
  await events.scrollIntoViewIfNeeded();
  await expect(events.locator('img.portrait')).toHaveCount(2);
  await events.locator('img.portrait').last().scrollIntoViewIfNeeded();
  await settle();
  await page.screenshot({ path: 'e2e/.captures/real/bosses-events-blossom-light.png', animations: 'disabled' });

  await go(page, '/bosses/Meilin/knowledge');
  const hero = page.locator('.knowledge-hero');
  await expect(hero.locator('.status-chip')).toHaveText(SEASON_4);
  await expect(hero.locator('img.portrait')).toBeVisible();
  await settle();
  await page.screenshot({ path: 'e2e/.captures/real/knowledge-meilin-blossom-light.png', animations: 'disabled' });
});

// Synthetic fixtures: MaleficStar has an invented 1-second solid-colour MP4
// (e2e/fixtures/boss/artwork/animated); Kalos has entry art only.
test.describe('animated knowledge hero', () => {
  test.skip(REAL_ART, 'fixture-specific assertions');

  test('a boss with animated art plays a muted, looping, decorative video over its still poster', async ({ page }) => {
    await go(page, '/bosses/MaleficStar/knowledge');
    const hero = page.locator('.knowledge-hero');
    const video = hero.locator('video.knowledge-hero__art');
    await expect(video).toHaveAttribute('src', '/art/animated/MaleficStar');
    await expect(video).toHaveAttribute('poster', '/art/entry/MaleficStar');
    await expect(video).toHaveAttribute('aria-hidden', 'true');
    await expect(video).toHaveAttribute('preload', 'metadata');
    await expect(hero.locator('img.knowledge-hero__art')).toHaveCount(0);
    expect(await video.evaluate((v: HTMLVideoElement) => ({ muted: v.muted, loop: v.loop, playsInline: v.playsInline, controls: v.controls }))).toEqual({ muted: true, loop: true, playsInline: true, controls: false });
    await expect.poll(() => video.evaluate((v: HTMLVideoElement) => !v.paused && v.currentTime > 0)).toBe(true);
    // The video takes the still's place exactly: the same box as the image.
    await page.route(/\/api\/admin\/bosses\/MaleficStar\/knowledge$/, async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, json: { ...(await response.json()), animated: null } });
    });
    // Layout boxes (offset*), so the pane's enter transform cannot skew them.
    const layout = (element: HTMLElement) => [element.offsetLeft, element.offsetTop, element.offsetWidth, element.offsetHeight];
    const box = await video.evaluate(layout);
    await go(page, '/bosses/MaleficStar/knowledge');
    const still = hero.locator('img.knowledge-hero__art');
    await expect(still).toHaveAttribute('src', '/art/entry/MaleficStar');
    expect(await still.evaluate(layout)).toEqual(box);
  });

  test('reduced motion shows the still and never an autoplaying video', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await go(page, '/bosses/MaleficStar/knowledge');
    const hero = page.locator('.knowledge-hero');
    const still = hero.locator('img.knowledge-hero__art');
    await expect(still).toHaveAttribute('src', '/art/entry/MaleficStar');
    await expect(still).toHaveAttribute('alt', '');
    await expect.poll(() => still.evaluate((i: HTMLImageElement) => i.naturalWidth)).toBeGreaterThan(0);
    await expect(hero.locator('video')).toHaveCount(0);
    // Asking for motion again brings the video back without a reload.
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await expect(hero.locator('video.knowledge-hero__art')).toHaveAttribute('src', '/art/animated/MaleficStar');
  });

  test('a boss without animated art keeps the still image, and switching drops the previous video', async ({ page }) => {
    await go(page, '/bosses/MaleficStar/knowledge');
    const hero = page.locator('.knowledge-hero');
    await expect(hero.locator('video.knowledge-hero__art')).toHaveCount(1);
    await page.locator('.bosses-list a.bossrow__name', { hasText: /Kalos/ }).click();
    await expect(page.getByRole('heading', { level: 2, name: /Kalos/ })).toBeVisible();
    await expect(hero.locator('img.knowledge-hero__art')).toHaveAttribute('src', '/art/entry/Kalos');
    await expect(hero.locator('video')).toHaveCount(0);
    expect((await (await page.request.get(`${ADMIN}/api/admin/bosses/Kalos/knowledge`)).json()).animated).toBeNull();
  });

  test('a failing video falls back to the still; a failing still leaves the plain hero', async ({ page }) => {
    await page.route(/\/art\/animated\/MaleficStar$/, (route) => route.fulfill({ status: 404 }));
    await go(page, '/bosses/MaleficStar/knowledge');
    const hero = page.locator('.knowledge-hero');
    await expect(hero.locator('img.knowledge-hero__art')).toHaveAttribute('src', '/art/entry/MaleficStar');
    await expect(hero.locator('video')).toHaveCount(0);

    await page.route(/\/art\/entry\/MaleficStar$/, (route) => route.fulfill({ status: 404 }));
    await go(page, '/bosses/MaleficStar/knowledge');
    await expect(page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' })).toBeVisible();
    await expect(hero.locator('.knowledge-hero__art')).toHaveCount(0);
  });

  test('the admin service worker leaves animated art (and its byte ranges) to the network', async ({ page }) => {
    await page.goto(`${ADMIN}/`);
    await page.evaluate(() => navigator.serviceWorker.ready);
    await page.reload();
    await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);
    const fetched = async (path: string, headers: Record<string, string> = {}) => {
      const [response] = await Promise.all([page.waitForResponse((r) => r.url().endsWith(path) && r.request().resourceType() === 'fetch' && !r.request().serviceWorker()), page.evaluate(([url, h]) => fetch(url, { headers: h }).then(() => undefined), [path, headers] as const)]);
      return response;
    };
    // Control: the worker does answer other same-origin art.
    expect((await fetched('/art/entry/MaleficStar')).fromServiceWorker()).toBe(true);
    const ranged = await fetched('/art/animated/MaleficStar', { Range: 'bytes=0-9' });
    expect(ranged.status()).toBe(206);
    expect(ranged.fromServiceWorker()).toBe(false);
    const cached = await page.evaluate(async () => (await Promise.all((await caches.keys()).map(async (name) => (await (await caches.open(name)).keys()).map((r) => r.url)))).flat());
    expect(cached.filter((url) => url.includes('/art/animated/'))).toEqual([]);
  });
});
