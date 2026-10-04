import type { Locator, Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// M3E progress bars, always on (user decision 2026-10-04): the boss week in
// the Week footer, run countdowns (wavy over the final 24 h, filling to T-1h,
// then over the last hour with a T-15m mark), answers as a segmented bar,
// proposal expiry in the Inbox, and model permits on Limits and Config, which
// wave (even when full) while calls are in flight (user decision 2026-10-04).
// At most two bars wave on a screen.
// Captures for review go to the git-ignored e2e/.captures/progress/.
const OUT = 'e2e/.captures/progress';

const ys = (d: string) => [...d.matchAll(/[ML][\d.]+ ([\d.]+)/g)].map((m) => m[1]!);
/** The bars that may move: not flat and not finished. */
const waves = (page: Page) => page.locator('.wavy:not(.wavy--flat)');

/** A moving bar's fill swings between different heights (the drift has started). */
async function isWaving(bar: Locator) {
  await expect.poll(async () => new Set(ys((await bar.locator('.wavy__wave').getAttribute('d')) ?? '')).size).toBeGreaterThan(1);
}

/** An empty bar draws no fill at all; any other fill must be one straight line. */
async function isFlat(bar: Locator) {
  if ((await bar.getAttribute('aria-valuenow')) === '0') return (await bar.locator('.wavy__wave').getAttribute('d')) === '';
  await expect.poll(async () => (await bar.locator('.wavy__wave').getAttribute('d')) ?? '').not.toBe('');
  return new Set(ys((await bar.locator('.wavy__wave').getAttribute('d'))!)).size === 1;
}

test('Week: the footer shows the boss week as a flat bar, and at most two bars wave', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${ADMIN}/?sw=off`);
  const week = page.getByRole('progressbar', { name: 'Boss week' });
  await expect(week).toHaveAttribute('aria-valuetext', /^Day [1-7] of 7 · resets \w{3} \d\d:\d\d$/);
  await expect(page.locator('.week-foot__week')).toContainText(/Day [1-7] of 7 · resets/);
  await expect(week).toHaveClass(/wavy--flat/);
  expect(await isFlat(week)).toBe(true);
  // Every card carries its answers, in words in the card's name too.
  const card = page.locator('[data-run="r-carling"]');
  await expect(card.locator('.answerbar')).toHaveCount(1);
  await expect(card.getByRole('button').first()).toHaveAccessibleName(/Answers: .*of \d+\./);
  // The glance's countdown to the next run; only the last-hour stage has a (T-15m) mark.
  const countdown = page.locator('.week-glance').getByRole('progressbar', { name: /^Countdown to / });
  await expect(countdown).toHaveAttribute('aria-valuetext', /^starts in /);
  await expect(countdown.locator('.wavy__tick')).toHaveCount(await stageMarks(countdown));
  const far = /day/.test((await countdown.getAttribute('aria-valuetext'))!);
  await expect(countdown).toHaveClass(far ? /wavy--flat/ : /^(?!.*wavy--flat)/);
  expect(await waves(page).count()).toBeLessThanOrEqual(2);
  await page.locator('.week-window').screenshot({ path: `${OUT}/week-glance.png` });
});

/** Marks a countdown draws: the T-15m mark inside the last hour, none before. */
async function stageMarks(countdown: Locator): Promise<number> {
  const text = (await countdown.getAttribute('aria-valuetext'))!;
  return /^starts in (\d+ min|1 h)$/.test(text) ? 1 : 0;
}

test('Week: the run pane shows the countdown and the answers bar', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const pane = page.locator('.week-pane');
  await expect(pane).toBeVisible();
  await expect(pane.getByRole('img', { name: /^Answers: .* of \d+$/ })).toBeVisible();
  const countdown = pane.getByRole('progressbar', { name: /^Countdown to / });
  if (await countdown.count()) {
    await expect(countdown).toHaveAttribute('aria-valuetext', /^starts in /);
    await expect(countdown.locator('.wavy__tick')).toHaveCount(await stageMarks(countdown));
  }
  // The pane replaces the glance: never more than two waves.
  expect(await waves(page).count()).toBeLessThanOrEqual(2);
  await pane.screenshot({ path: `${OUT}/week-pane.png` });
});

test('Week on a phone: the footer bar is flat, prints the day, and nothing in the footer is clipped', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  const week = page.getByRole('progressbar', { name: 'Boss week' });
  await expect(week).toHaveAttribute('aria-valuetext', /resets/);
  await expect(page.locator('.week-foot__week')).toHaveText(/^Day [1-7] of 7$/);
  expect(await isFlat(week)).toBe(true);
  // Nothing in the footer is clipped: every item ends inside it.
  const foot = (await page.locator('.week-window__foot').boundingBox())!;
  for (const item of await page.locator('.week-window__foot > *').all()) {
    const box = (await item.boundingBox())!;
    expect(box.x + box.width).toBeLessThanOrEqual(foot.x + foot.width + 0.5);
    expect(box.y + box.height).toBeLessThanOrEqual(foot.y + foot.height + 0.5);
  }
  await page.locator('.week-window__foot').screenshot({ path: `${OUT}/week-foot-phone.png` });
});

test('Week at 1000 px: without the glance the footer keeps every fact, the week line wrapping if it must', async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 670 });
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('.week-foot__week')).toHaveText(/^Day [1-7] of 7 · resets/);
  const foot = (await page.locator('.week-window__foot').boundingBox())!;
  for (const item of await page.locator('.week-window__foot > *').all()) {
    const box = (await item.boundingBox())!;
    expect(box.x + box.width).toBeLessThanOrEqual(foot.x + foot.width + 0.5);
  }
  await page.locator('.week-window__foot').screenshot({ path: `${OUT}/week-foot-1000.png` });
});

test('Inbox: an open proposal drains toward its expiry', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${ADMIN}/inbox?sw=off`);
  const bar = page.getByRole('progressbar', { name: 'Time left to decide' }).first();
  await expect(bar).toBeVisible();
  await expect(bar).toHaveAttribute('aria-valuetext', /left(, expiring soon)?, expires \w{3} \d\d \w{3} \d\d:\d\d$/);
  expect(await isFlat(bar)).toBe(true);
  const soon = /expiring soon/.test((await bar.getAttribute('aria-valuetext'))!);
  await expect(bar).toHaveClass(soon ? /wavy--warn/ : /wavy--accent/);
  await page.locator('.proposal__expiry').first().screenshot({ path: `${OUT}/inbox-expiry.png` });
});

test('Limits: one permit bar per group, waving only with requests in flight, two at most', async ({ page }) => {
  await page.goto(`${ADMIN}/limits?sw=off`);
  const bars = page.getByRole('progressbar', { name: / permits in use$/ });
  await expect(bars.first()).toBeVisible();
  for (const bar of await bars.all()) {
    const busy = Number(await bar.getAttribute('aria-valuenow')) > 0;
    if (!busy) await expect(bar).toHaveClass(/wavy--flat/);
  }
  // Every permit held: the full bar keeps waving while its calls run.
  const full = page.getByRole('progressbar', { name: 'gateway permits in use' });
  await expect(full).toHaveAttribute('aria-valuenow', (await full.getAttribute('aria-valuemax'))!);
  await isWaving(full);
  expect(await waves(page).count()).toBeGreaterThan(0);
  expect(await waves(page).count()).toBeLessThanOrEqual(2);
  await page.locator('.stats').screenshot({ path: `${OUT}/limits.png` });
});

test('Config: each capacity group shows its permits against what Kanata admits, waving while calls run', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=models&sw=off`);
  await page.getByRole('tab', { name: 'Capacity' }).click();
  const bar = page.getByRole('progressbar', { name: 'gateway permits of what Kanata admits' });
  await expect(bar).toBeVisible();
  // The mock's gateway group has a call in flight: its full bar waves.
  await expect(bar).toHaveAttribute('aria-valuetext', /^\d+ of the \d+ Kanata admits, \d+ in flight$/);
  await expect(bar).not.toHaveClass(/wavy--flat/);
  await isWaving(bar);
  expect(await waves(page).count()).toBeLessThanOrEqual(2);
});

test('Config: idle groups stay flat, and at most two of the busy ones wave', async ({ page }) => {
  await page.route(`${ADMIN}/api/admin/config`, async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const res = await route.fetch();
    const body = await res.json();
    body.models.groups_source = 'config';
    body.models.groups = [
      { model: 'kanata/extract', group: 'a', permits: 1, in_use: 1 },
      { model: 'kanata/chat', group: 'b', permits: 1, in_use: 0 },
      { model: 'kanata/legacy', group: 'c', permits: 1, in_use: 1 },
      { model: 'kanata/rewrite-small', group: 'd', permits: 1, in_use: 1 },
    ];
    await route.fulfill({ response: res, json: body });
  });
  await page.goto(`${ADMIN}/config?section=models&sw=off`);
  await page.getByRole('tab', { name: 'Capacity' }).click();
  await expect(page.getByRole('progressbar', { name: /permits of what Kanata admits$/ }).first()).toBeVisible();
  await expect(page.getByRole('progressbar', { name: 'b permits of what Kanata admits' })).toHaveClass(/wavy--flat/);
  expect(await waves(page).count()).toBe(2);
});

test('reduced motion: countdowns and permits draw flat and still', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${ADMIN}/limits?sw=off`);
  for (const bar of await page.getByRole('progressbar', { name: / permits in use$/ }).all()) expect(await isFlat(bar)).toBe(true);
});
