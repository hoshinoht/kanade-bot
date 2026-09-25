import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

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

test('queued reminders: 15 per page, counted, one line per row', async ({ page }) => {
  await longQueue(page);
  await page.goto(`${ADMIN}/reminders?sw=off`);
  const queued = page.getByRole('table', { name: 'Queued reminders' });
  await expect(queued.locator('tbody tr')).toHaveCount(15);
  await expect(page.getByText('1–15 of 33 queued cards')).toBeVisible();
  await page.getByRole('button', { name: 'Later →' }).click();
  await expect(page.getByText('16–30 of 33 queued cards')).toBeVisible();
  await page.getByRole('button', { name: 'Later →' }).click();
  await expect(queued.locator('tbody tr')).toHaveCount(3);
  await expect(page.getByRole('button', { name: 'Later →' })).toBeDisabled();
  await page.getByRole('button', { name: '← Earlier' }).click();
  await page.getByRole('button', { name: '← Earlier' }).click();

  // One line: every row as tall as a single-line row; the party cut to three and "+3".
  const heights = await queued.locator('tbody tr').evaluateAll((rows) => rows.map((r) => r.getBoundingClientRect().height));
  expect(Math.max(...heights) - Math.min(...heights)).toBeLessThan(4);
  const first = queued.locator('tbody tr').first();
  const more = first.locator('.chip', { hasText: '+3' });
  await expect(more).toHaveAttribute('title', 'Rin, Kaho, Sora');
  await expect(first.locator('.chip:not(.chip--mono)')).toHaveCount(3);
});

test('queued reminders: kind, run, member and day filters narrow and clear', async ({ page }) => {
  await page.goto(`${ADMIN}/reminders?sw=off`);
  const queued = page.getByRole('table', { name: 'Queued reminders' });
  const filters = page.getByRole('group', { name: 'Filter reminders' });
  const all = await queued.locator('tbody tr').count();

  const kind = filters.getByLabel('Kind');
  const kinds = await kind.locator('option').allTextContents();
  await kind.selectOption(kinds[1]!);
  for (const row of await queued.locator('tbody tr').all()) await expect(row.getByRole('cell').nth(1)).toHaveText(kinds[1]!);
  await filters.getByRole('button', { name: 'Clear' }).click();
  await expect(queued.locator('tbody tr')).toHaveCount(all);

  const run = filters.getByLabel('Run');
  await run.selectOption({ index: 1 });
  const short = (await run.locator('option').nth(1).textContent())!.split('#')[1]!;
  for (const row of await queued.locator('tbody tr').all()) await expect(row).toContainText(`#${short}`);
  await filters.getByRole('button', { name: 'Clear' }).click();

  const member = filters.getByLabel('Member');
  const who = (await member.locator('option').nth(1).textContent())!;
  await member.selectOption(who);
  for (const row of await queued.locator('tbody tr').all()) {
    const chips = await row.locator('.chip').evaluateAll((c) => c.map((e) => `${e.textContent} ${e.getAttribute('title') ?? ''}`).join(' '));
    expect(chips).toContain(who);
  }
  await filters.getByRole('button', { name: 'Clear' }).click();

  const day = filters.getByLabel('Day');
  const when = (await day.locator('option').nth(1).textContent())!;
  await day.selectOption(when);
  for (const row of await queued.locator('tbody tr').all()) await expect(row.getByRole('cell').first()).toContainText(when);
});

test('reminder rows highlight across every cell on hover', async ({ page }) => {
  await page.goto(`${ADMIN}/reminders?sw=off`);
  const row = page.getByRole('table', { name: 'Queued reminders' }).locator('tbody tr').nth(1);
  const idle = await row.locator('td').first().evaluate((c) => getComputedStyle(c).backgroundColor);
  await row.hover();
  const cells = await row.evaluate((tr) => [...tr.children].map((c) => getComputedStyle(c).backgroundColor));
  expect(new Set(cells).size).toBe(1);
  expect(cells[0]).not.toBe(idle);
  // The row reaches the table's edges: the highlight cannot stop short.
  const [tr, table] = await Promise.all([row.boundingBox(), row.locator('xpath=ancestor::table').boundingBox()]);
  expect(Math.abs(tr!.width - table!.width)).toBeLessThan(2);
});
