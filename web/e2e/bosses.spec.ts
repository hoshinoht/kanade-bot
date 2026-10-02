import type { Page } from '@playwright/test';
import { ADMIN, REAL_ART, expect, test } from './support';

// Event bosses (knowledge documents with an `event`, outside the catalog: Kai,
// Meilin in the tracked boss/knowledge): art like catalog rows and the
// "Seasonal boss" label. The synthetic fixtures carry no event art, so the
// image path is driven through the events read with fixture art.

async function go(page: Page, path: string) {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

const SEASON_3 = 'Seasonal boss · Challengers World Season 3';
const SEASON_4 = 'Seasonal boss · Challengers World Season 4';

test.describe('event bosses', () => {
  test.skip(REAL_ART, 'fixture-specific assertions');

  test('the Bosses page labels event rows by season and only them', async ({ page }) => {
    await go(page, '/bosses');
    const events = page.getByRole('list', { name: 'Event bosses' });
    const kai = events.getByRole('listitem').filter({ has: page.getByRole('link', { name: 'Kai', exact: true }) });
    const meilin = events.getByRole('listitem').filter({ has: page.getByRole('link', { name: 'Meilin', exact: true }) });
    await expect(kai.locator('.status-chip')).toHaveText(SEASON_3);
    await expect(meilin.locator('.status-chip')).toHaveText(SEASON_4);
    // No fixture art for event keys: the monogram holds the same box.
    await expect(kai.locator('.portrait--mono')).toHaveText('Ka');
    await expect(page.getByRole('group', { name: 'Bosses' })).not.toContainText('Seasonal boss');
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
    await expect(page.getByRole('heading', { level: 1 }).locator('.status-chip')).toHaveText(SEASON_4);
    await go(page, '/bosses/MaleficStar/knowledge');
    await expect(page.getByRole('heading', { level: 1 })).toContainText('Radiant Malefic Star');
    await expect(page.getByRole('heading', { level: 1 })).not.toContainText('Seasonal boss');
  });
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
  await expect(page.getByRole('heading', { level: 1 }).locator('.status-chip')).toHaveText(SEASON_4);
  await expect(page.getByRole('heading', { level: 1 }).locator('img.portrait')).toBeVisible();
  await settle();
  await page.screenshot({ path: 'e2e/.captures/real/knowledge-meilin-blossom-light.png', animations: 'disabled' });
});
