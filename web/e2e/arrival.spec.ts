import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Arrival motion (live updates stage 2): a change from elsewhere, hinted by the
// mock's event stream (`POST /__mock/arrive`), glides the Week cards it moved
// (FLIP), marks new rows once (`data-new`) and pulses changed counts
// (`data-tick`). The admin's own writes never do; reduced motion moves nothing.

/** Records Web Animations (target, first transform) and every `data-new`/`data-tick` mark set on the page. */
async function record(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as { __animated: { target: string; from: string }[]; __marks: { mark: string; what: string }[] };
    w.__animated = [];
    w.__marks = [];
    const original = Element.prototype.animate;
    Element.prototype.animate = function (this: Element, frames, options) {
      const first = Array.isArray(frames) ? (frames[0] as Record<string, unknown> | undefined) : undefined;
      w.__animated.push({ target: (this as HTMLElement).dataset?.run ?? this.className, from: String(first?.transform ?? '') });
      return original.call(this, frames, options);
    };
    new MutationObserver((list) => {
      for (const m of list) {
        const el = m.target as HTMLElement;
        if (m.attributeName && el.hasAttribute(m.attributeName))
          w.__marks.push({ mark: m.attributeName, what: el.dataset.run ?? el.dataset.item ?? el.className });
      }
    }).observe(document, { subtree: true, attributes: true, attributeFilter: ['data-new', 'data-tick'] });
  });
}

const animated = (page: Page) => page.evaluate(() => (window as unknown as { __animated: { target: string; from: string }[] }).__animated);
const marks = (page: Page) => page.evaluate(() => (window as unknown as { __marks: { mark: string; what: string }[] }).__marks);

async function arrive(page: Page, kind: 'move' | 'run' | 'proposal') {
  const reply = await page.request.post(`${ADMIN}/__mock/arrive`, { data: { kind } });
  expect(reply.status()).toBe(204);
}

/** The event stream is open, so the next change is hinted. */
async function streaming(page: Page) {
  await expect.poll(() => page.evaluate(() => performance.getEntriesByType('resource').some((e) => e.name.includes('/api/admin/events')))).toBe(true);
}

test('Week: a run another admin moved glides to its new day', async ({ page }) => {
  await record(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/?sw=off`);
  const limbo = page.locator('[data-run="r-limbo"]');
  await expect(limbo).toBeVisible();
  await streaming(page);
  const from = await limbo.boundingBox();
  await arrive(page, 'move');
  await expect.poll(async () => (await animated(page)).filter((a) => a.target === 'r-limbo' && a.from.startsWith('translate')).length, { timeout: 6_000 }).toBe(1);
  await expect(limbo).toContainText('21:00');
  // The glide starts at the old place (FLIP), so wait for it to land.
  await expect.poll(async () => (await limbo.boundingBox())!.x).not.toBe(from!.x);
});

test('Week: a run another admin added keeps its mark for the whole tint, not just the settle', async ({ page }) => {
  await record(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run]').first()).toBeVisible();
  await streaming(page);
  await arrive(page, 'run');
  const card = page.locator('.board.planner [data-run="r-arrived"]');
  await expect(card).toHaveAttribute('data-new', '', { timeout: 6_000 });
  const marked = Date.now();
  // The settle (250 ms) ends first; the tint (1.2 s) keeps the mark.
  await page.waitForTimeout(600);
  await expect(card).toHaveAttribute('data-new', '');
  expect(await card.evaluate((el) => el.getAnimations().some((a) => a instanceof CSSAnimation && a.animationName === 'arrive-tint'))).toBe(true);
  // Cleared once the tint has played (well before the 2 s fallback).
  await expect(card).not.toHaveAttribute('data-new', '', { timeout: 3_000 });
  expect(Date.now() - marked).toBeLessThan(2_000);
});

test('Inbox: a proposal from the extractor is marked new and the badge pulses', async ({ page }) => {
  await record(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/inbox?sw=off`);
  const list = page.getByRole('listbox', { name: 'Extractor items' });
  await expect(list.getByRole('option').first()).toBeVisible();
  await streaming(page);
  expect(await marks(page)).toEqual([]);
  await arrive(page, 'proposal');
  await expect(list.locator('[data-item="p-arrived"]')).toHaveAttribute('data-new', '', { timeout: 6_000 });
  await expect.poll(async () => (await marks(page)).filter((m) => m.mark === 'data-tick' && m.what.includes('navlist__badge')).length).toBe(1);
  // Only the newcomer is marked.
  await expect(list.locator('[data-new]')).toHaveCount(1);
  // Once the mark has played, the list mounting again (Past and back) does not replay it.
  await page.waitForTimeout(2_400);
  await page.getByRole('tab', { name: /^Past/ }).click();
  await expect(page.getByRole('listbox', { name: 'Past items' })).toBeVisible();
  await page.getByRole('tab', { name: /^Extractor/ }).click();
  await expect(list.locator('[data-item="p-arrived"]')).toBeVisible();
  await expect(list.locator('[data-new]')).toHaveCount(0);
});

test("the admin's own decision neither marks rows nor pulses the badge", async ({ page }) => {
  await record(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/inbox?sw=off`);
  const list = page.getByRole('listbox', { name: 'Extractor items' });
  await list.getByRole('option').nth(1).click();
  await streaming(page);
  await page.getByRole('button', { name: 'Reject…' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Reject change' }).click();
  const nav = page.locator('.navrail').getByRole('navigation', { name: 'Sections' });
  await expect(nav.getByRole('link', { name: 'Inbox 8 waiting', exact: true })).toBeVisible();
  // Past the hint's re-read: still nothing marked.
  await page.waitForTimeout(1_500);
  expect(await marks(page)).toEqual([]);
});

test.describe('reduced motion', () => {
  test.use({ reducedMotion: 'reduce' });

  test('nothing travels: no FLIP, and a new row only fades its colour', async ({ page }) => {
    await record(page);
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-limbo"]')).toBeVisible();
    await streaming(page);
    await arrive(page, 'move');
    await expect(page.locator('[data-run="r-limbo"]')).toContainText('21:00', { timeout: 6_000 });
    expect((await animated(page)).filter((a) => a.from.startsWith('translate'))).toEqual([]);

    await page.goto(`${ADMIN}/inbox?sw=off`);
    const list = page.getByRole('listbox', { name: 'Extractor items' });
    await expect(list.getByRole('option').first()).toBeVisible();
    await streaming(page);
    await arrive(page, 'proposal');
    const row = list.locator('[data-item="p-arrived"]');
    await expect(row).toHaveAttribute('data-new', '', { timeout: 6_000 });
    expect(await row.evaluate((el) => getComputedStyle(el).animationName)).toBe('arrive-tint');
    // The badge's pulse is a scale: under reduced motion it stays still.
    const badge = page.locator('.navrail .navlist__badge');
    await expect(badge).toHaveAttribute('data-tick', '');
    expect(await badge.evaluate((el) => getComputedStyle(el).animationName)).toBe('none');
  });
});

test("a hint's re-reads revalidate: unchanged reads answer 304", async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="r-kalos"]')).toBeVisible();
  await streaming(page);
  const statuses: string[] = [];
  page.on('response', (r) => {
    const path = new URL(r.url()).pathname;
    if (['/api/admin/week', '/api/admin/stats', '/api/admin/summary'].includes(path)) statuses.push(`${path} ${r.status()}`);
  });
  const sentTag = page.waitForRequest((r) => r.url().includes('/api/admin/week') && Boolean(r.headers()['if-none-match']));
  // An inbox change: the summary differs, the week and stats do not.
  await arrive(page, 'proposal');
  await sentTag;
  await expect.poll(() => statuses.length, { timeout: 6_000 }).toBeGreaterThanOrEqual(3);
  expect(statuses).toEqual(expect.arrayContaining(['/api/admin/week 304', '/api/admin/stats 304', '/api/admin/summary 200']));
  const nav = page.locator('.navrail').getByRole('navigation', { name: 'Sections' });
  await expect(nav.getByRole('link', { name: 'Inbox 10 waiting', exact: true })).toBeVisible();
});
