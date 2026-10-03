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
  const art = hero.locator('img.knowledge-hero__art');
  await expect(portrait).toHaveAttribute('src', '/art/portraits/MaleficStar');
  await expect(art).toHaveAttribute('src', '/art/entry/MaleficStar');
  for (const image of [portrait, art]) await expect.poll(() => image.evaluate((element) => (element as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
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
