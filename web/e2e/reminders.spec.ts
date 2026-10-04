import type { Page } from '@playwright/test';
import { ADMIN, expect, PINNED_NOW, test, choose, optionLabels } from './support';

type Row = { id: string; party: string[]; kind: string; at: string };

/** A live-sized queue: the mock's rows repeated to 33, one with a big party. */
async function longQueue(page: Page) {
  await page.route(/\/api\/admin\/reminders$/, async (route) => {
    const res = await route.fetch();
    const body = (await res.json()) as { upcoming: Row[] };
    const base = body.upcoming;
    body.upcoming = Array.from({ length: 33 }, (_, i) => ({ ...base[i % base.length]!, id: `q-${i}` }));
    body.upcoming[0]!.party = ['Asahi', 'Yuzu', 'Mio', 'Rin', 'Kaho', 'Sora'];
    await route.fulfill({ response: res, json: body });
  });
}

const queuedRows = (page: Page) => page.getByRole('table', { name: 'Queued reminders' }).locator('tbody tr:has(td)');
const groupRows = (page: Page) => page.getByRole('table', { name: 'Queued reminders' }).locator('tbody tr:not(:has(td))');
const panel = (page: Page) => page.getByRole('tabpanel');

test('queued reminders: one day-grouped table that scrolls, one line per row', async ({ page }) => {
  await longQueue(page);
  await page.goto(`${ADMIN}/reminders?sw=off`);
  const rows = queuedRows(page);
  await expect(rows).toHaveCount(33);
  await expect(page.getByText('33 of 33 shown')).toBeVisible();
  await expect(page.getByRole('tab', { name: /^Queued/ })).toContainText('33');
  // The table region scrolls, not the window.
  expect(await panel(page).evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);
  // Day groups: a label row before each day's rows.
  await expect(groupRows(page).first()).toHaveText(/^Tue 29 Sep/);

  // One line: every row as tall as a single-line row; the party cut to three and "+3".
  const heights = await rows.evaluateAll((all) => all.map((r) => r.getBoundingClientRect().height));
  expect(Math.max(...heights) - Math.min(...heights)).toBeLessThan(4);
  const first = rows.first();
  const more = first.locator('.chip', { hasText: '+3' });
  await expect(more).toHaveAttribute('title', 'Rin, Kaho, Sora');
  await expect(first.locator('.chip:not(.chip--mono)')).toHaveCount(3);
});

test('reminders: In, today and the next card read the guild clock', async ({ page }) => {
  await page.clock.setFixedTime(PINNED_NOW);
  await page.goto(`${ADMIN}/reminders?sw=off`);
  const first = queuedRows(page).first();
  await expect(first.getByRole('cell').first()).toHaveText('Tue 29 Sep 21:00');
  await expect(first.getByRole('cell').nth(1)).toHaveText('9 h');
  await expect(groupRows(page).first()).toHaveText(/^Tue 29 Sep · today\s*\d+$/);
  await expect(page.locator('footer', { hasText: 'shown' })).toContainText(/Next\s*in 9 h/);
});

test('reminders: Queued, Sent and Stale & other tabs, each keeping its scroll', async ({ page }) => {
  await longQueue(page);
  await page.goto(`${ADMIN}/reminders?sw=off`);
  await expect(queuedRows(page)).toHaveCount(33);
  await panel(page).evaluate((el) => el.scrollTo(0, 200));

  await page.getByRole('tab', { name: /^Sent/ }).click();
  const sent = page.getByRole('table', { name: 'Sent reminders' });
  await expect(sent.getByRole('link', { name: /open in Discord/ }).first()).toBeVisible();
  for (const row of await sent.locator('tbody tr:has(td)').all()) await expect(row.getByRole('cell').last()).toContainText('sent');

  await page.getByRole('tab', { name: /^Stale & other/ }).click();
  const stale = page.getByRole('table', { name: 'Stale and other reminders' });
  const row = stale.locator('tbody tr:has(td)').first();
  await expect(row).toHaveAttribute('title', /retired without posting/);
  await expect(row.getByRole('cell').last()).toContainText('stale');

  // Arrow keys move between tabs; Queued is back where it was left.
  await page.getByRole('tab', { name: /^Stale & other/ }).press('ArrowRight');
  await expect(page.getByRole('tab', { name: /^Queued/ })).toHaveAttribute('aria-selected', 'true');
  await expect.poll(() => panel(page).evaluate((el) => el.scrollTop)).toBe(200);
});

test('queued reminders: kind, run, member and day filters narrow and clear', async ({ page }) => {
  await page.goto(`${ADMIN}/reminders?sw=off`);
  const rows = queuedRows(page);
  const all = await rows.count();
  await page.getByRole('button', { name: 'Filters (0)' }).click();
  const filters = page.getByRole('group', { name: 'Filter reminders' });

  const kind = filters.getByLabel('Kind');
  const kinds = await optionLabels(kind);
  await choose(kind, kinds[1]!);
  await expect(page.getByRole('button', { name: 'Filters (1)' })).toBeVisible();
  for (const row of await rows.all()) await expect(row.getByRole('cell').nth(2)).toHaveText(kinds[1]!);
  await filters.getByRole('button', { name: 'Clear' }).click();
  await expect(rows).toHaveCount(all);

  const run = filters.getByLabel('Run');
  const short = (await optionLabels(run))[1]!.split('#')[1]!;
  await choose(run, { index: 1 });
  for (const row of await rows.all()) await expect(row).toContainText(`#${short}`);
  await filters.getByRole('button', { name: 'Clear' }).click();

  const member = filters.getByLabel('Member');
  const who = (await optionLabels(member))[1]!;
  await choose(member, who);
  for (const row of await rows.all()) {
    const chips = await row.locator('.chip').evaluateAll((c) => c.map((e) => `${e.textContent} ${e.getAttribute('title') ?? ''}`).join(' '));
    expect(chips).toContain(who);
  }
  await filters.getByRole('button', { name: 'Clear' }).click();

  const day = filters.getByLabel('Day');
  const when = (await optionLabels(day))[1]!;
  await choose(day, when);
  for (const row of await rows.all()) await expect(row.getByRole('cell').first()).toContainText(when);
  await expect(page.getByText(`${await rows.count()} of ${all} shown`)).toBeVisible();

  // Escape folds the popover back onto its button.
  await day.press('Escape');
  await expect(filters).toBeHidden();
  await expect(page.getByRole('button', { name: 'Filters (1)' })).toBeFocused();
});

test('reminder rows highlight across every cell on hover', async ({ page }) => {
  await page.goto(`${ADMIN}/reminders?sw=off`);
  const row = queuedRows(page).nth(1);
  const idle = await row.locator('td').first().evaluate((c) => getComputedStyle(c).backgroundColor);
  await row.hover();
  const cells = await row.evaluate((tr) => [...tr.children].map((c) => getComputedStyle(c).backgroundColor));
  expect(new Set(cells).size).toBe(1);
  expect(cells[0]).not.toBe(idle);
  // The row reaches the table's edges: the highlight cannot stop short.
  const [tr, table] = await Promise.all([row.boundingBox(), row.locator('xpath=ancestor::table').boundingBox()]);
  expect(Math.abs(tr!.width - table!.width)).toBeLessThan(2);
});
