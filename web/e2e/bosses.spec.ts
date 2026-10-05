import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { GUIDE_DOC, GUIDE_MISSIONS } from './guide-fixture';
import { ADMIN, REAL_ART, expect, settle, test } from './support';

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
  // The tracked document's facts: tiles, and the guild's difficulty open.
  await expect(page.locator('.guide-tile dt').first()).toHaveText('Boss level');
  const timing = page.locator('.knowledge-aside__timings li').first();
  await expect(timing.locator('strong')).toHaveText(/^(Mon|Tue|Wed|Thu|Fri|Sat|Sun) \d\d:\d\d$/);
  await expect(timing.locator('.pill')).toHaveText(/^(EASY|NORMAL|HARD|CHAOS|EXTREME)$/);
  await expect(page.locator('.knowledge-aside__count')).toContainText('next');
});

test('bosses: knowledge strategies get a tab when the document has them and not otherwise', async ({ page }) => {
  const knowledge = await (await page.request.get(`${ADMIN}/api/admin/bosses/Lotus/knowledge`)).json() as { doc: { strategies?: { name: string; risk: string; damage: string; when: string; payoff: string; steps: string[] }[] } };
  const expected = knowledge.doc.strategies ?? [];
  expect(expected.length).toBeGreaterThan(0);
  await go(page, '/bosses/Lotus/knowledge');
  const tabs = page.getByRole('tablist', { name: 'Guide sections' });
  await tabs.getByRole('tab', { name: /^Strategies/ }).click();
  await expect(page).toHaveURL(/[?&]tab=strategies/);
  const cards = page.locator('.guide-strategy');
  await expect(cards).toHaveCount(expected.length);
  const words = { low: 'Low', medium: 'Medium', high: 'High' } as Record<string, string>;
  for (const [index, strategy] of expected.entries()) {
    const card = cards.nth(index);
    await expect(card.getByRole('heading', { level: 3 })).toHaveText(strategy.name);
    await expect(card.locator('.guide-meter strong')).toHaveText([words[strategy.risk]!, words[strategy.damage]!]);
    await expect(card.locator('.guide-strategy__when dd')).toHaveText([strategy.when, strategy.payoff]);
    // Steps stay folded until asked for.
    await expect(card.locator('ol.guide-steps > li')).toHaveCount(strategy.steps.length);
    await expect(card.getByRole('button', { name: `Show ${strategy.steps.length} step${strategy.steps.length === 1 ? '' : 's'}` })).toHaveAttribute('aria-expanded', 'false');
  }

  // Kai's document declares no strategies: no tab for them.
  const kai = await (await page.request.get(`${ADMIN}/api/admin/bosses/Kai/knowledge`)).json() as { doc: { strategies?: unknown[] } };
  expect(kai.doc.strategies ?? []).toHaveLength(0);
  await go(page, '/bosses/Kai/knowledge');
  await expect(tabs.getByRole('tab', { name: /^Sources/ })).toBeVisible();
  await expect(tabs.getByRole('tab', { name: /^Strategies/ })).toHaveCount(0);
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
  const timing = page.locator('.knowledge-aside__timings li a').first();
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
// Champion and Destiny live only in boss knowledge (never the scheduler's
// catalog): no tracked document has them yet, so the read is extended here.
test.describe('info-only difficulties', () => {
  test.skip(REAL_ART, 'fixture-specific assertions');

  for (const scheme of ['light', 'dark'] as const) {
    test(`Champion and Destiny show on the knowledge page in their own colours (${scheme})`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.route(/\/api\/admin\/bosses\/MaleficStar\/knowledge$/, async (route) => {
        const response = await route.fetch();
        const body = (await response.json()) as { doc: { difficulties: object[] } };
        body.doc.difficulties.push({ name: 'Champion', boss_level: 285 }, { name: 'Destiny', boss_level: 290, notes: ['Placeholder note.'] });
        await route.fulfill({ response, json: body });
      });
      await go(page, '/bosses/MaleficStar/knowledge');
      const row = page.locator('.bossrow--active');
      await expect(row.locator('.row-content__full .boss-tick--champion')).toHaveText('CHAMPION');
      await expect(row.locator('.row-content__full .boss-tick--destiny')).toHaveText('DESTINY');
      await expect(row.locator('.row-content__full .bossrow__difficulties > span').filter({ has: page.locator('.boss-tick--destiny') })).toHaveText('DESTINYinfo only');

      const switcher = page.getByRole('group', { name: 'Difficulty' });
      await switcher.getByRole('button', { name: 'Champion', exact: true }).click();
      await expect(page.locator('#facts-heading')).toHaveText('Champion facts');
      await expect(page.locator('.guide-tiles')).toContainText('285');
      await switcher.getByRole('button', { name: 'Destiny', exact: true }).click();
      await expect(page.locator('#difficulty-notes-heading')).toHaveText('Destiny notes');

      await settle(page);
      const axe = await new AxeBuilder({ page }).include('.bossrow--active .bossrow__difficulties').withRules(['color-contrast']).analyze();
      expect(axe.violations).toEqual([]);
    });
  }
});

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

// The tabbed guide (user-approved redesign, 2026-10-05), over an invented
// document in the new shapes; MaleficStar's tracked document is the old shape.
test.describe('boss guide', () => {
  test.skip(REAL_ART, 'fixture-specific assertions');

  async function inject(page: Page, missions: unknown[] = GUIDE_MISSIONS) {
    await page.route(/\/api\/admin\/bosses\/MaleficStar\/knowledge$/, async (route) => {
      const response = await route.fetch();
      const body = (await response.json()) as Record<string, unknown>;
      await route.fulfill({ response, json: { ...body, doc: GUIDE_DOC, missions } });
    });
  }
  const tablist = (page: Page) => page.getByRole('tablist', { name: 'Guide sections' });

  test('pill tabs carry counts, hide empty sections and follow ?tab=', async ({ page }) => {
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    const tabs = tablist(page).getByRole('tab');
    await expect(tabs).toHaveText(['Overview', /^Phases\s*4$/, /^Strategies\s*2$/, /^Notes\s*2$/, /^Sources\s*3$/]);
    await expect(tabs.first()).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('.guide-lead')).toHaveText(GUIDE_DOC.lead);

    await tablist(page).getByRole('tab', { name: /^Phases/ }).click();
    await expect(page).toHaveURL(/\/bosses\/MaleficStar\/knowledge\?(.+&)?tab=phases$/);
    // The timeline opens on the first phase; its items show below.
    await expect(page.getByRole('tablist', { name: 'Phases' }).getByRole('tab').first()).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('.guide-phase h3')).toHaveText('Lamp rooms');
    await expect(page.locator('.guide-phase li')).toHaveText(['Split up Two per room.', 'A plain phase line.']);
    await tablist(page).getByRole('tab', { name: /^Phases/ }).focus();
    // Arrow keys move along the tabs and take the panel with them.
    await page.keyboard.press('ArrowRight');
    await expect(tablist(page).getByRole('tab', { name: /^Strategies/ })).toBeFocused();
    await expect(page).toHaveURL(/[?&]tab=strategies$/);
    await page.keyboard.press('End');
    await expect(tablist(page).getByRole('tab', { name: /^Sources/ })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('list', { name: 'Sources by kind' }).getByRole('listitem')).toHaveText(['Official 1', 'Guide 1', 'Wiki 1']);
    await expect(page.locator('.guide-sources li').nth(1)).toContainText('by Someone · guide · fetched 2026-10-05 · updated 2026-10-01');
    await page.keyboard.press('Home');
    await expect(page).not.toHaveURL(/tab=/);

    await go(page, '/bosses/MaleficStar/knowledge?tab=notes');
    await expect(tablist(page).getByRole('tab', { name: /^Notes/ })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('list', { name: 'Notes' }).getByRole('listitem')).toHaveText(['Old buildNo longer works after the room rule.', 'A plain note line.']);
    // A tab the document cannot fill falls back to Overview.
    await go(page, '/bosses/Kai/knowledge?tab=strategies');
    await expect(tablist(page).getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true');
  });

  test('Overview: mechanic blocks, then danger and tips as titled rows; the bot-only detail never shows', async ({ page }) => {
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    const panel = page.getByRole('tabpanel');
    await expect(panel.locator('.guide-mechanic h3')).toHaveText(['Pool · 1000 shared', 'Room zones', 'Lamp scale']);
    await expect(panel.locator('.guide-ledger li > span:first-child')).toHaveText(['Death', 'Clean phase', 'Neutral row']);
    await expect(panel.locator('.guide-ledger__value')).toHaveText([/▼\s*−100/, /▲\s*\+300/, '0']);
    await expect(panel.locator('.guide-ledger .guide-down')).toHaveCount(1);
    await expect(panel.locator('.guide-zones li')).toHaveText(['Leftred', 'Centreyellow', 'Rightgreen']);
    await expect(panel.locator('.guide-zones li').first()).toHaveClass(/guide-tone--red/);
    await expect(panel.locator('.guide-scale li')).toHaveText(['0', 'low', '250 – 750 safe', 'high', '1000']);
    // Band widths follow `span` (6 : 50), never under the label.
    const [edge, middle] = [await panel.locator('.guide-scale li').first().boundingBox(), await panel.locator('.guide-scale li').nth(2).boundingBox()];
    expect(middle!.width).toBeGreaterThan(edge!.width * 3);
    const danger = page.getByRole('region', { name: 'Danger' });
    await expect(danger.locator('.guide-row__mark--risk')).toHaveCount(2);
    await expect(danger.locator('.guide-row strong')).toHaveText(['Lamp flare']);
    await expect(danger.locator('.guide-row').nth(1)).toHaveText('A plain danger line.');
    await expect(page.getByRole('region', { name: 'Tips' }).locator('.guide-row__mark--ok')).toHaveCount(1);
    await expect(page.getByRole('region', { name: 'Core' })).toHaveCount(0);
    await expect(page.locator('.knowledge-detail')).not.toContainText('BOT-ONLY');
    await tablist(page).getByRole('tab', { name: /^Phases/ }).click();
    await expect(page.locator('.knowledge-detail')).not.toContainText('BOT-ONLY');
  });

  test('facts: tiles, the HP breakdown per phase and target, and the difficulty notes', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    await expect(page.getByRole('group', { name: 'Difficulty' }).getByRole('button', { name: /^Hard/ })).toHaveAttribute('aria-pressed', 'true');
    const tiles = page.locator('.guide-tile');
    await expect(tiles.locator('dt')).toHaveText(['Boss level', 'Defence (PDR)', 'Authentic Force', 'Party', 'Recommended']);
    await expect(tiles.first().locator('dd')).toHaveText(['285', 'entry 275']);
    await expect(tiles.last().locator('dd')).toHaveText(['≈ 99k', 'Invented, 2026']);
    const hp = page.getByRole('region', { name: 'HP', exact: true });
    // Short values as stored on screen; spelled out in the title and for screen readers.
    const shown = (scope: typeof hp) => scope.locator('[aria-hidden="true"]');
    await expect(hp.locator('.guide-hp__total > span').first()).toHaveText('Total HP');
    await expect(shown(hp.locator('.guide-hp__total'))).toHaveText('6q');
    await expect(hp.locator('.guide-hp__total .vh')).toHaveText('6 quadrillion');
    await expect(hp.locator('.guide-hp__total .mono')).toHaveAttribute('title', '6 quadrillion');
    const labels = hp.locator('.guide-hp__label');
    await expect(labels.locator('> span:first-child')).toHaveText(['Phase 1 (Lamps)', 'Phase 2', 'Phase 3 (Lamps)', 'Phase 3 (Keeper)']);
    // Split phases carry their total (stored decimals kept: 2 × 1.05q is 2.10q); a single target shows its value once, on its bar.
    await expect(shown(labels.nth(0))).toHaveText('2.7q');
    await expect(shown(labels.nth(2))).toHaveText('2.10q');
    await expect(labels.nth(1).locator('.mono')).toHaveCount(0);
    await expect(labels.nth(3).locator('.mono')).toHaveCount(0);
    const phases = hp.locator('.guide-hp__phase');
    await expect(shown(phases.nth(1).locator('.guide-hp__bar'))).toHaveText('1.2q');
    await expect(shown(phases.nth(0).locator('.guide-hp__bar'))).toHaveText(['900t', '900t', '900t']);
    await expect(phases.nth(0).getByRole('list', { name: '3 targets, each' })).toBeVisible();
    await expect(shown(phases.nth(2).locator('.guide-hp__bar'))).toHaveText(['1.05q', '1.05q']);
    await expect(phases.nth(3).locator('.guide-hp__bar .vh')).toHaveText('2.1 quadrillion');
    await expect(hp.locator('.guide-hp__bar').first()).toHaveClass(/guide-hp__bar--h/);
    await settle(page);
    // Targets of a phase share its width equally; a lone target fills it.
    const widths = async (index: number) => phases.nth(index).locator('.guide-hp__bar').evaluateAll((bars) => bars.map((bar) => Math.round(bar.getBoundingClientRect().width)));
    const [first] = await widths(0);
    expect(await widths(0)).toEqual([first, first, first]);
    const label = (await labels.nth(1).boundingBox())!.width;
    expect((await widths(1))[0]).toBeCloseTo(label, -1);
    await expect(page.locator('.guide-notes .guide-row')).toHaveText(['Hard twistA random second room is hit.', 'Invented Hard note.']);
    // Wide, the column is stretched to the body's height: the tab strip keeps its full height.
    const strip = await page.locator('.guide-tabs').evaluate((element) => [element.clientHeight, element.scrollHeight]);
    expect(strip[0]).toBeGreaterThanOrEqual(40);
    expect(strip[1]).toBeLessThanOrEqual(strip[0]!);
  });

  test('a Destiny mission card shows the series track from the API, or just its number', async ({ page }) => {
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    const switcher = page.getByRole('group', { name: 'Difficulty' });
    await expect(switcher.getByRole('button', { name: 'Destiny', exact: true })).toHaveClass(/seg__info/);
    await expect(page.getByRole('region', { name: 'Invented mission title' })).toHaveCount(0);
    await switcher.getByRole('button', { name: 'Destiny', exact: true }).click();
    const card = page.getByRole('region', { name: 'Invented mission title' });
    await expect(card.locator('.cap').first()).toHaveText('Destiny Weapon mission · 2 of 3');
    await expect(card.locator('.boss-tick--destiny')).toHaveText('DESTINY');
    await expect(card.locator('.guide-mission__mod--up')).toHaveText(/\+20% Final Damage\s*in your favour/);
    const track = card.getByRole('list', { name: 'Mission order' }).getByRole('listitem');
    await expect(track).toHaveText(['1Invented First', '2 · nowInvented Second', '3Invented Third']);
    await expect(track.nth(1)).toHaveAttribute('aria-current', 'step');
    await expect(card.locator('.guide-mission__chips li')).toHaveText(['Needs 3,000 Invented Resolve', 'Practice counts', 'No Cross World']);
    await expect(page.locator('.guide-tile dt')).toContainText(['Party']);
    await expect(page.locator('.guide-tile').filter({ hasText: 'Party' }).locator('dd')).toHaveText('Solo');

    await page.unrouteAll();
    await inject(page, []);
    await go(page, '/bosses/MaleficStar/knowledge?difficulty=Destiny');
    await expect(page.getByRole('region', { name: 'Invented mission title' }).locator('.cap').first()).toHaveText('Destiny Weapon mission · Mission 2');
    await expect(page.getByRole('list', { name: 'Mission order' })).toHaveCount(0);
  });

  test('Phases: one timeline bar picks a phase; groups bracket their segments with the loop cue in words', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge?tab=phases');
    const bar = page.getByRole('tablist', { name: 'Phases' });
    const segments = bar.getByRole('tab');
    // Names, the group in the accessible name, tags after them.
    await expect(segments).toHaveText(['Lamp rooms, two per room', 'The keeper · repeats: Day, gauge fills', 'The keeper · repeats: Night, burst · no gauge', 'Last stand']);
    await expect(bar.locator('.guide-timeline__bracket')).toHaveText(['', '↻The keeper · repeats', '']);
    // Tone tints from data; untoned segments stay plain.
    await expect(segments.nth(1)).toHaveClass(/guide-tone--yellow/);
    await expect(segments.nth(2)).toHaveClass(/guide-tone--blue/);
    await expect(segments.nth(0)).toHaveClass(/guide-timeline__seg--plain/);
    await settle(page);
    // One horizontal bar of equal segments; the bracket spans its group's two.
    const boxes = await segments.evaluateAll((list) => list.map((seg) => seg.getBoundingClientRect()).map((r) => ({ top: Math.round(r.top), width: Math.round(r.width) })));
    expect(new Set(boxes.map((box) => box.top)).size).toBe(1);
    expect(new Set(boxes.map((box) => box.width)).size).toBe(1);
    const bracket = (await bar.locator('.guide-timeline__run--group .guide-timeline__bracket').boundingBox())!;
    expect(bracket.width).toBeGreaterThan(boxes[0]!.width * 2 - 2);

    const panel = page.getByRole('tabpanel', { name: /Lamp rooms/ });
    await expect(panel.locator('h3')).toHaveText('Lamp rooms');
    await segments.nth(2).click();
    await expect(page).toHaveURL(/[?&]phase=3(&|$)/);
    const night = page.locator('.guide-phase');
    await expect(night.locator('.cap')).toHaveText(/↻\s*The keeper · repeats/);
    await expect(night.locator('h3')).toHaveText('Night');
    await expect(night.locator('.guide-phase__tag')).toHaveText('burst · no gauge');
    await expect(night.locator('li')).toHaveText(['Invented night line.']);
    await expect(page.locator('.guide-phase')).toHaveCount(1);
    // Keyboard: arrows move and select, Home/End jump; focus follows.
    await page.keyboard.press('ArrowRight');
    await expect(segments.nth(3)).toBeFocused();
    await expect(segments.nth(3)).toHaveAttribute('aria-selected', 'true');
    await page.keyboard.press('ArrowRight');
    await expect(segments.nth(0)).toBeFocused();
    await page.keyboard.press('End');
    await expect(night.locator('h3')).toHaveText('Last stand');
    await page.keyboard.press('Home');
    await expect(night.locator('h3')).toHaveText('Lamp rooms');
    await expect(page).not.toHaveURL(/phase=/);
    // Deep link, and leaving the tab drops it.
    await go(page, '/bosses/MaleficStar/knowledge?tab=phases&phase=2');
    await expect(segments.nth(1)).toHaveAttribute('aria-selected', 'true');
    await expect(night.locator('h3')).toHaveText('Day');
    await tablist(page).getByRole('tab', { name: /^Notes/ }).click();
    await expect(page).not.toHaveURL(/phase=/);
  });

  test('a single phase shows just its card: no timeline bar, no tablist', async ({ page }) => {
    const doc = { ...structuredClone(GUIDE_DOC), phases: [{ name: 'Only phase', tag: 'one tag', items: ['Invented only line.'] }] };
    await page.route(/\/api\/admin\/bosses\/MaleficStar\/knowledge$/, async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, json: { ...(await response.json()), doc, missions: GUIDE_MISSIONS } });
    });
    await go(page, '/bosses/MaleficStar/knowledge?tab=phases');
    await expect(page.locator('.guide-phase h3')).toHaveText('Only phase');
    await expect(page.locator('.guide-phase__tag')).toHaveText('one tag');
    await expect(page.locator('.guide-phase li')).toHaveText(['Invented only line.']);
    await expect(page.getByRole('tablist', { name: 'Phases' })).toHaveCount(0);
    await expect(page.locator('.guide-timeline__bar')).toHaveCount(0);
    await expect(page.locator('.guide-phase')).not.toHaveAttribute('role', 'tabpanel');
  });

  test('HP is a disclosure, open by default; closed, the total stays in its head', async ({ page }) => {
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    const hp = page.getByRole('region', { name: 'HP', exact: true });
    const toggle = hp.getByRole('button', { name: 'HP' });
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');
    await expect(hp.locator('.guide-hp__phase')).toHaveCount(4);
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await expect(hp.locator('.guide-hp__phase').first()).toBeHidden();
    await expect(hp.locator('.guide-hp__head .guide-hp__total [aria-hidden="true"]')).toHaveText('6q');
    await page.keyboard.press('Enter');
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');
    await expect(hp.locator('.guide-hp__phase').first()).toBeVisible();
  });

  test('the Recommended tile lists one figure per party size, its basis underneath', async ({ page }) => {
    const doc = structuredClone(GUIDE_DOC);
    Object.assign(doc.difficulties[0]!.recommended_spec, { parties: [{ party: 'Solo', value: '≈ 120k' }, { party: 'Duo', value: '≈ 95k' }, { party: '6 players', value: '≈ 60k' }] });
    await page.route(/\/api\/admin\/bosses\/MaleficStar\/knowledge$/, async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, json: { ...(await response.json()), doc, missions: GUIDE_MISSIONS } });
    });
    await go(page, '/bosses/MaleficStar/knowledge');
    const tile = page.locator('.guide-tile').filter({ hasText: 'Recommended' });
    const rows = tile.getByRole('list', { name: 'Recommended by party size' }).getByRole('listitem');
    await expect(rows).toHaveText(['Solo≈ 120k', 'Duo≈ 95k', '6 players≈ 60k']);
    await expect(tile.locator('.guide-tile__value')).toHaveCount(0);
    await expect(tile.locator('.guide-tile__sub')).toHaveText('Invented, 2026');
  });

  test('Destiny and Champion in the difficulty switch carry no rim, and their plate when selected', async ({ page }) => {
    const doc = structuredClone(GUIDE_DOC);
    (doc.difficulties as unknown[]).push({ name: 'Champion', boss_level: 285 });
    await page.route(/\/api\/admin\/bosses\/MaleficStar\/knowledge$/, async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, json: { ...(await response.json()), doc, missions: GUIDE_MISSIONS } });
    });
    await go(page, '/bosses/MaleficStar/knowledge');
    const switcher = page.getByRole('group', { name: 'Difficulty' });
    for (const name of ['Destiny', 'Champion']) {
      const button = switcher.getByRole('button', { name, exact: true });
      expect(await button.evaluate((el) => getComputedStyle(el).boxShadow)).toBe('none');
      await button.click();
      await expect(button).toHaveAttribute('aria-pressed', 'true');
      expect(await button.evaluate((el) => getComputedStyle(el).boxShadow)).toBe('none');
      const [bg, plate] = await button.evaluate((el, token) => [getComputedStyle(el).backgroundColor, (() => {
        const probe = document.createElement('i');
        probe.style.setProperty('color', `var(${token})`);
        document.body.append(probe);
        const colour = getComputedStyle(probe).color;
        probe.remove();
        return colour;
      })()], `--pill-${name.toLowerCase()}-bg`);
      expect(bg).toBe(plate);
    }
  });

  test('On this page: sticky contents that scroll the panel, select tabs and mark where you are', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    const toc = page.getByRole('navigation', { name: 'On this page' });
    await expect(toc.getByRole('button')).toHaveText(['Facts', 'HP', 'Hard notes', 'Overview', 'Phases', 'Strategies', 'Notes', 'Sources']);
    const panel = page.locator('.knowledge-detail__body');
    const docTop = () => page.evaluate(() => document.scrollingElement!.scrollTop);
    // A section entry scrolls the panel (never the document) and marks itself.
    await toc.getByRole('button', { name: 'Hard notes' }).click();
    await expect(toc.getByRole('button', { name: 'Hard notes' })).toHaveAttribute('aria-current', 'location');
    await expect.poll(() => panel.evaluate((b) => b.scrollTop)).toBeGreaterThan(100);
    expect(await docTop()).toBe(0);
    // Sticky: still in view after scrolling.
    await expect(toc).toBeInViewport();
    // A tab entry selects the tab and brings the tab strip up.
    await toc.getByRole('button', { name: 'Strategies' }).click();
    await expect(tablist(page).getByRole('tab', { name: /^Strategies/ })).toHaveAttribute('aria-selected', 'true');
    await expect(tablist(page).getByRole('tab', { name: /^Strategies/ })).toBeFocused();
    await expect(page).toHaveURL(/[?&]tab=strategies/);
    await expect(toc.getByRole('button', { name: 'Strategies' })).toHaveAttribute('aria-current', 'location');
    await expect.poll(async () => {
      const [strip, box] = await Promise.all([page.locator('.guide-tabs').boundingBox(), panel.boundingBox()]);
      return Math.abs(strip!.y - box!.y);
    }).toBeLessThan(40);
    // Scroll-spy: back at the top, the first section is current again.
    await panel.evaluate((b) => b.scrollTo(0, 0));
    await expect(toc.getByRole('button', { name: 'Facts' })).toHaveAttribute('aria-current', 'location');
    expect(await docTop()).toBe(0);
  });

  test('On this page is hidden where the aside sits under the guide', async ({ page }) => {
    await page.setViewportSize({ width: 1000, height: 670 });
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    await expect(page.getByRole('tablist', { name: 'Guide sections' })).toBeVisible();
    await expect(page.getByRole('navigation', { name: 'On this page' })).toBeHidden();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole('navigation', { name: 'On this page' })).toBeHidden();
  });

  test('the hero collapses to one line with the difficulty when scrolled, and comes back at the top', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    const hero = page.locator('.knowledge-hero');
    const panel = page.locator('.knowledge-detail__body');
    await expect(hero).not.toHaveClass(/knowledge-hero--compact/);
    await expect(hero.locator('.knowledge-hero__pill')).toBeHidden();
    const tall = (await hero.boundingBox())!.height;
    await panel.evaluate((b) => b.scrollTo(0, 400));
    await expect(hero).toHaveClass(/knowledge-hero--compact/);
    await settle(page);
    await expect(hero.locator('.knowledge-hero__pill')).toHaveText(/HARD/);
    await expect(hero.locator('.knowledge-hero__meta')).toHaveCSS('opacity', '0');
    expect((await hero.boundingBox())!.height).toBeLessThan(tall / 2);
    expect(await page.evaluate(() => document.scrollingElement!.scrollTop)).toBe(0);
    await panel.evaluate((b) => b.scrollTo(0, 0));
    await expect(hero).not.toHaveClass(/knowledge-hero--compact/);
  });

  test('the phase bar is a connected button group: round outer ends, small inner corners, the selected one a pill', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge?tab=phases&phase=2');
    const segments = page.getByRole('tablist', { name: 'Phases' }).getByRole('tab');
    await settle(page);
    const radii = await segments.evaluateAll((list) => list.map((seg) => { const cs = getComputedStyle(seg); return [parseFloat(cs.borderTopLeftRadius), parseFloat(cs.borderTopRightRadius)]; }));
    // First: round left, small right. Selected (2nd): round both. Third: small both. Last: small left, round right.
    expect(radii[0]![0]).toBeGreaterThanOrEqual(20);
    expect(radii[0]![1]).toBeLessThanOrEqual(8);
    expect(Math.min(...radii[1]!)).toBeGreaterThanOrEqual(20);
    expect(Math.max(...radii[2]!)).toBeLessThanOrEqual(8);
    expect(radii[3]![0]).toBeLessThanOrEqual(8);
    expect(radii[3]![1]).toBeGreaterThanOrEqual(20);
  });

  test('a narrow scale band breaks between words, never inside a figure', async ({ page }) => {
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge');
    const figure = page.locator('.guide-scale .guide-figure').filter({ hasText: '250 – 750' });
    await expect(figure).toHaveCSS('white-space', 'nowrap');
    const lines = await figure.evaluate((el) => new Set([...el.getClientRects()].map((r) => Math.round(r.top))).size);
    expect(lines).toBe(1);
  });

  test('the last card ends with room below it on every tab', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await inject(page);
    for (const tab of ['', '?tab=notes', '?tab=sources']) {
      await go(page, `/bosses/MaleficStar/knowledge${tab}`);
      await expect(tablist(page)).toBeVisible();
      const gap = await page.locator('.knowledge-detail__body').evaluate((b) => {
        b.scrollTo(0, b.scrollHeight);
        const last = b.querySelector('.knowledge-detail__main')!.lastElementChild!.getBoundingClientRect();
        return b.getBoundingClientRect().bottom - last.bottom;
      });
      expect(gap, `room below the last card${tab}`).toBeGreaterThanOrEqual(32);
    }
  });

  test('Phases on a phone: the timeline turns vertical and nothing is cut', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge?tab=phases');
    const segments = page.getByRole('tablist', { name: 'Phases' }).getByRole('tab');
    await expect(segments).toHaveCount(4);
    await settle(page);
    const boxes = await segments.evaluateAll((list) => list.map((seg) => ({ top: Math.round(seg.getBoundingClientRect().top), fits: seg.scrollWidth <= seg.clientWidth })));
    expect(new Set(boxes.map((box) => box.top)).size).toBe(4);
    expect(boxes.every((box) => box.fits)).toBe(true);
    await expect(page.locator('.guide-timeline__run--group .guide-timeline__bracket')).toHaveText('↻The keeper · repeats');
    await segments.nth(1).click();
    await expect(page.locator('.guide-phase h3')).toHaveText('Day');
  });

  test('a Union Champion track labels ranks by letter and shows only the tracked ranks', async ({ page }) => {
    // Tracked documents (the mock fills `missions`): Lotus B, Black Mage S, Seren SS, Kalos SSS; rank A is not tracked.
    await go(page, '/bosses/Seren/knowledge?difficulty=Champion');
    const card = page.locator('.guide-mission');
    await expect(card.locator('.cap').first()).toHaveText('Union Champion mission · Rank SS');
    const track = card.getByRole('list', { name: 'Mission order' }).getByRole('listitem');
    await expect(track.locator('.guide-mission__num')).toHaveText(['B', 'S', 'SS · now', 'SSS']);
    await expect(track.nth(2)).toHaveAttribute('aria-current', 'step');
    // The same boss's Destiny mission keeps numbers.
    await page.getByRole('group', { name: 'Difficulty' }).getByRole('button', { name: 'Destiny', exact: true }).click();
    await expect(card.locator('.cap').first()).toHaveText('Destiny Weapon mission · 1 of 6');
    await expect(card.locator('.guide-mission__num')).toHaveText(['1 · now', '2', '3', '4', '5', '6']);
  });

  test('strategy steps fold behind a toggle', async ({ page }) => {
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge?tab=strategies');
    const card = page.locator('.guide-strategy').first();
    await expect(card.locator('.guide-meter')).toHaveText([/Risk\s*Low/, /Damage need\s*High/]);
    const steps = card.getByRole('list', { name: 'Steps for Balanced lamps' });
    const toggle = card.getByRole('button', { name: 'Show 3 steps' });
    await expect(steps).toBeHidden();
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await toggle.click();
    await expect(steps.getByRole('listitem')).toHaveText(['1Step one.', '2Step two.', '3Step three.']);
    await expect(card.getByRole('button', { name: 'Hide steps' })).toHaveAttribute('aria-expanded', 'true');
    await expect(page.locator('.guide-strategy').nth(1).getByRole('button', { name: 'Show 1 step' })).toBeVisible();
  });

  test('an old-shape document keeps core rows on Overview and has no Phases tab', async ({ page }) => {
    // The pre-redesign shape (plain strings, core, no lead/phases/mechanics; a lone HP total; a recommendation with no figure).
    const OLD = {
      boss: 'MaleficStar',
      summary: 'Invented old summary.',
      core: ['Invented core one.', 'Invented core two.'],
      danger: ['Invented danger.'],
      tips: ['Invented tip.'],
      difficulties: [{ name: 'Hard', boss_level: 280, hp: [{ phase: 'total', value: '14.74q' }], recommended_spec: { kind: 'HEXA-converted stat', text: 'Invented long recommendation.' } }],
      sources: [{ url: 'https://example.invalid/old', title: 'Invented', author: 'Nobody', kind: 'guide', fetched: '2026-10-01' }],
    };
    await page.route(/\/api\/admin\/bosses\/MaleficStar\/knowledge$/, async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, json: { ...(await response.json()), doc: OLD, missions: [] } });
    });
    await go(page, '/bosses/MaleficStar/knowledge');
    await expect(tablist(page).getByRole('tab')).toHaveText(['Overview', /^Sources\s*1$/]);
    await expect(page.getByRole('region', { name: 'Core' }).locator('.guide-row')).toHaveText(OLD.core);
    await expect(page.locator('.guide-lead')).toHaveText(OLD.summary);
    await expect(page.locator('.guide-hp')).toHaveCount(0); // a lone total is a tile
    await expect(page.locator('.guide-tile').filter({ hasText: 'HP (total)' }).locator('dd')).toHaveText('14.74q');
    await expect(page.locator('.guide-tile').filter({ hasText: 'Recommended' })).toHaveCount(0);
    await expect(page.locator('.guide-notes .guide-row')).toHaveText(['Recommended · HEXA-converted statInvented long recommendation.']);
  });

  for (const scheme of ['light', 'dark'] as const) {
    test(`the guide passes axe (${scheme})`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await page.setViewportSize({ width: 1280, height: 800 });
      await inject(page);
      await go(page, '/bosses/MaleficStar/knowledge');
      await settle(page);
      const check = async () => {
        const axe = await new AxeBuilder({ page }).include('.knowledge-detail').analyze();
        expect(axe.violations.map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(' ')).join(', ')}`)).toEqual([]);
      };
      await check();
      await page.getByRole('group', { name: 'Difficulty' }).getByRole('button', { name: 'Destiny', exact: true }).click();
      await tablist(page).getByRole('tab', { name: /^Strategies/ }).click();
      await page.getByRole('button', { name: 'Show 3 steps' }).click();
      await settle(page);
      await check();
      for (const tab of ['Phases', 'Notes', 'Sources']) {
        await tablist(page).getByRole('tab', { name: new RegExp(`^${tab}`) }).click();
        await settle(page);
        await check();
      }
    });
  }

  test('phone: six targets wrap into rows of equal bars, never cut', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const doc = structuredClone(GUIDE_DOC);
    (doc.difficulties[0]!.hp as unknown[])[0] = { phase: '1', value: '1,234.5t', count: 6, target: 'Many lamps' };
    await page.route(/\/api\/admin\/bosses\/MaleficStar\/knowledge$/, async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, json: { ...(await response.json()), doc, missions: GUIDE_MISSIONS } });
    });
    await go(page, '/bosses/MaleficStar/knowledge');
    const bars = page.locator('.guide-hp__phase').first().locator('.guide-hp__bar');
    await expect(bars).toHaveCount(6);
    await settle(page);
    const boxes = await bars.evaluateAll((list) => list.map((bar) => ({ top: Math.round(bar.getBoundingClientRect().top), width: Math.round(bar.getBoundingClientRect().width), fits: bar.scrollWidth <= bar.clientWidth })));
    expect(new Set(boxes.map((box) => box.top)).size).toBeGreaterThan(1);
    expect(new Set(boxes.map((box) => box.width)).size).toBe(1);
    expect(boxes.every((box) => box.fits)).toBe(true);
    await expect(page.locator('.guide-hp__phase').first().locator('.guide-hp__label [aria-hidden="true"]')).toHaveText('7.407q');
  });

  test('phone: tabs scroll sideways inside the one scrolling panel and cards stack', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await inject(page);
    await go(page, '/bosses/MaleficStar/knowledge?tab=strategies');
    const strip = page.locator('.guide-tabs');
    await expect(strip).toBeVisible();
    // The detail's enter motion slides it in; measure once it has landed.
    await settle(page);
    expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= window.innerHeight)).toBe(true);
    const box = (await strip.boundingBox())!;
    expect(box.x + box.width).toBeLessThanOrEqual(390);
    // The strip scrolls; the panel around it never does sideways.
    expect(await page.locator('.knowledge-detail__body').evaluate((body) => body.scrollWidth <= body.clientWidth)).toBe(true);
    const [first, second] = await page.locator('.guide-strategy').evaluateAll((cards) => cards.map((card) => card.getBoundingClientRect().top));
    expect(second!).toBeGreaterThan(first!);
    // The last tab can be reached by scrolling the strip itself.
    await tablist(page).getByRole('tab', { name: /^Sources/ }).click();
    await expect(page).toHaveURL(/[?&]tab=sources(&|$)/);
  });
});
