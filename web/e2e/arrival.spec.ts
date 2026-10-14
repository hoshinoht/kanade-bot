import type { Page } from '@playwright/test';
import type { Week } from '@kanade/api-types';
import { ADMIN, PUBLIC, csrf, expect, signInPublic, test } from './support';

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
  await expect(nav.getByRole('link', { name: 'Inbox 10 waiting', exact: true })).toBeVisible();
  // Past the hint's re-read: still nothing marked.
  await page.waitForTimeout(1_500);
  expect(await marks(page)).toEqual([]);
});

// The member portal: a member hint (`GET /api/public/events`; the mock sends
// one on `POST /__mock/public/hint`) re-reads the week at once, and without
// one the week arrives with the timed read. Either read that changed the week
// is an arrival (the admin store's semantics); the member's own Refresh is not.

/** The member stream hints `topic` (the mock's stand-in for a change elsewhere). */
async function hint(page: Page, topic: 'schedule' | 'mine' | 'allowance') {
  const reply = await page.request.post(`${PUBLIC}/__mock/public/hint`, { data: { topic } });
  expect(reply.status()).toBe(204);
}

/** The member stream is open, so the next hint reaches the page. */
async function memberStreaming(page: Page) {
  await expect.poll(() => page.evaluate(() => performance.getEntriesByType('resource').some((e) => e.name.includes('/api/public/events')))).toBe(true);
}

/** Runs the portal's timed week read now: hiding and showing the page resumes the paused poller. */
async function timedRead(page: Page) {
  const read = page.waitForResponse((r) => r.url().endsWith('/api/public/week?week=next'));
  await page.evaluate(() => {
    let state: DocumentVisibilityState = 'hidden';
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
    document.dispatchEvent(new Event('visibilitychange'));
    state = 'visible';
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await read;
}

async function openPortal(page: Page) {
  await record(page);
  await signInPublic(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.locator('[data-run="r-limbo"]')).toHaveCount(1);
}

test.describe('member portal', () => {
  test('without a hint, a run an admin moved glides to its new day on the next timed read', async ({ page }) => {
    await page.route(`${PUBLIC}/api/public/events`, (route) => route.abort());
    await openPortal(page);
    await arrive(page, 'move');
    await timedRead(page);
    await expect.poll(async () => (await animated(page)).filter((a) => a.target === 'r-limbo' && a.from.startsWith('translate')).length).toBe(1);
    await expect(page.locator('[data-run="r-limbo"]')).toContainText('21:00');
  });

  test('a schedule hint re-reads the week at once: a moved run glides, an added one is marked', async ({ page }) => {
    await openPortal(page);
    await memberStreaming(page);
    await arrive(page, 'move');
    await arrive(page, 'run');
    const read = page.waitForResponse((r) => r.url().endsWith('/api/public/week?week=next'));
    await hint(page, 'schedule');
    await read;
    await expect.poll(async () => (await animated(page)).filter((a) => a.target === 'r-limbo' && a.from.startsWith('translate')).length, { timeout: 6_000 }).toBe(1);
    await expect(page.locator('[data-run="r-limbo"]')).toContainText('21:00');
    const card = page.locator('.member-board [data-run="r-arrived"]');
    await expect(card).toHaveAttribute('data-new', '');
    await expect(card).not.toHaveAttribute('data-new', '', { timeout: 3_000 });
  });

  test("a hint after the member's own answer is its echo: nothing glides, marks or pulses", async ({ page }) => {
    await record(page);
    await signInPublic(page);
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
    const answers = page.getByRole('complementary', { name: /^Your run · .*Carling/ }).getByRole('group', { name: /^Your answer/ });
    await expect(answers.getByRole('button', { name: /^In/ })).toHaveAttribute('aria-pressed', 'true');
    await memberStreaming(page);
    await answers.getByRole('button', { name: /^Maybe/ }).click();
    await expect(answers).toHaveAccessibleName('Your answer: Maybe');
    // The server hints the change back to its own author.
    const read = page.waitForResponse((r) => r.url().endsWith('/api/public/week?week=next'));
    await hint(page, 'mine');
    await read;
    await page.waitForTimeout(400); // "nothing starts" needs real time to pass
    // The pane slid in when the run opened; no card glides.
    expect((await animated(page)).filter((a) => a.target.startsWith('r-') && a.from.startsWith('translate'))).toEqual([]);
    expect(await marks(page)).toEqual([]);
  });

  test('without a hint, a run an admin added is marked once; a changed number on Your week pulses', async ({ page }) => {
    await page.route(`${PUBLIC}/api/public/events`, (route) => route.abort());
    await openPortal(page);
    await arrive(page, 'run');
    // Someone answers on Carling, the member's next run (Your week's "4/7 on").
    const week: Week = await (await page.request.get(`${ADMIN}/api/admin/week`)).json();
    const someone = week.runs.find((r) => r.id === 'r-carling')!.participants.find((p) => p.answer !== 'yes')!;
    const answered = await page.request.post(`${ADMIN}/api/admin/runs/r-carling/rsvp`, {
      headers: await csrf(page.request),
      data: { member_id: someone.id, answer: 'yes', version: week.version },
    });
    expect(answered.ok()).toBe(true);
    await timedRead(page);
    const card = page.locator('.member-board [data-run="r-arrived"]');
    await expect(card).toHaveAttribute('data-new', '');
    await expect(card).not.toHaveAttribute('data-new', '', { timeout: 3_000 });
    expect((await marks(page)).filter((m) => m.mark === 'data-tick' && m.what.includes('week-glance__fill'))).toHaveLength(1);
  });

  test("the member's own Refresh neither glides, marks nor pulses", async ({ page }) => {
    await openPortal(page);
    await arrive(page, 'move');
    await arrive(page, 'run');
    await page.getByRole('button', { name: 'Refresh the week' }).click();
    await expect(page.locator('[data-run="r-limbo"]')).toContainText('21:00');
    await expect(page.locator('[data-run="r-arrived"]')).toHaveCount(1);
    await page.waitForTimeout(400); // "nothing starts" needs real time to pass
    expect((await animated(page)).filter((a) => a.from.startsWith('translate'))).toEqual([]);
    expect(await marks(page)).toEqual([]);
  });
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

  test('member portal: no FLIP on a timed read, and a new run only fades its colour', async ({ page }) => {
    await page.route(`${PUBLIC}/api/public/events`, (route) => route.abort());
    await openPortal(page);
    await arrive(page, 'move');
    await arrive(page, 'run');
    await timedRead(page);
    const card = page.locator('.member-board [data-run="r-arrived"]');
    await expect(card).toHaveAttribute('data-new', '');
    expect(await card.evaluate((el) => getComputedStyle(el).animationName)).toBe('arrive-tint');
    expect((await animated(page)).filter((a) => a.from.startsWith('translate'))).toEqual([]);
  });

  test('member portal: a hinted read moves nothing, and a new run only fades its colour', async ({ page }) => {
    await openPortal(page);
    await memberStreaming(page);
    await arrive(page, 'move');
    await arrive(page, 'run');
    const read = page.waitForResponse((r) => r.url().endsWith('/api/public/week?week=next'));
    await hint(page, 'schedule');
    await read;
    const card = page.locator('.member-board [data-run="r-arrived"]');
    await expect(card).toHaveAttribute('data-new', '');
    // The colour fade follows the short settle.
    await expect.poll(() => card.evaluate((el) => getComputedStyle(el).animationName)).toBe('arrive-tint');
    await expect(page.locator('[data-run="r-limbo"]')).toContainText('21:00');
    expect((await animated(page)).filter((a) => a.from.startsWith('translate'))).toEqual([]);
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
  await expect(nav.getByRole('link', { name: 'Inbox 12 waiting', exact: true })).toBeVisible();
});
