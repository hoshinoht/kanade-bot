import type { Page } from '@playwright/test';
import { busyWeek } from './busy-week';
import { ADMIN, expect, test } from './support';

// The planner card's grip owns its own column: no difficulty pill runs under
// it, whatever the boss count, and populated days keep the M3E board's
// floor (B_WeekSel: busy days beside the pane are about 190 px; boss names
// may wrap, which the spec accepts; busier weeks scroll sideways).

const TRACK_PX = 184;

async function overlaps(page: Page) {
  return page.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>('.plan-card--movable')]
      .filter((card) => card.getClientRects().length > 0)
      .flatMap((card) => {
        const grip = card.querySelector('.plan-card__grip')!.getBoundingClientRect();
        const face = card.querySelector('.plan-card__open')!.getBoundingClientRect();
        const bosses = [...card.querySelectorAll<HTMLElement>('.runcard__bosses .boss')];
        return bosses.flatMap((boss) =>
          [...boss.querySelectorAll('.pill, .boss__name')].flatMap((el) => {
            const r = el.getBoundingClientRect();
            const hitsGrip = r.right > grip.left && r.left < grip.right && r.bottom > grip.top && r.top < grip.bottom;
            const clipped = r.right > face.right + 0.5 || r.left < face.left - 0.5;
            return hitsGrip || clipped ? [`${card.dataset.run} (${bosses.length} bosses) ${el.textContent}: ${hitsGrip ? 'under the grip' : 'clipped'}`] : [];
          }),
        );
      }),
  );
}

for (const size of [
  { width: 1280, height: 800 },
  { width: 1000, height: 670 },
  { width: 1920, height: 1080 },
  { width: 390, height: 844 },
]) {
  test(`planner card ${size.width}×${size.height}: no boss runs under the grip with 1, 2 or 3 bosses`, async ({ page }) => {
    await page.setViewportSize(size);
    await busyWeek(page);
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="busy-0-3"]')).toBeAttached();
    for (const count of [1, 2, 3]) {
      const card = page.locator(`[data-run="busy-0-${count}"]`);
      await card.scrollIntoViewIfNeeded();
      await expect(card.locator('.runcard__bosses .boss')).toHaveCount(count);
      await expect(card.locator('.plan-card__grip')).toBeVisible();
    }
    expect(await overlaps(page)).toEqual([]);
    // Hover and focus scale the grip up; it still clears the pills.
    if (size.width >= 900) {
      await page.locator('[data-run="busy-0-2"]').hover();
      await page.waitForTimeout(200);
      expect(await overlaps(page)).toEqual([]);
    }
    // The board scrolls sideways (or stacks); the document never does.
    expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight + 1)).toBe(true);
    expect(await page.evaluate(() => document.scrollingElement!.scrollWidth <= innerWidth + 1)).toBe(true);
  });
}

test('planner: populated days keep the 184 px floor and the busy board scrolls sideways, keyboard included', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await busyWeek(page);
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="busy-6-1"]')).toBeAttached();
  const widths = await page.locator('.board__col:not(.board__col--empty)').evaluateAll((cols) => cols.map((c) => c.getBoundingClientRect().width));
  expect(widths).toHaveLength(7);
  for (const w of widths) expect(w).toBeGreaterThanOrEqual(TRACK_PX - 0.5);
  const cards = await page.locator('.board__col .plan-card').evaluateAll((els) => els.map((e) => e.getBoundingClientRect().width));
  // One card per row in a minimum-width column, filling it (the column less its 8 px padding).
  for (const w of cards) expect(w).toBeGreaterThanOrEqual(TRACK_PX - 20);
  // A pill may drop under its name at that width, but never under the grip or past the card.
  expect(await overlaps(page)).toEqual([]);

  const board = page.locator('.board');
  const scroll = await board.evaluate((el) => ({ width: el.scrollWidth, client: el.clientWidth }));
  expect(scroll.width).toBeGreaterThan(scroll.client);
  // Moving focus to a card on the far day brings it into view.
  const last = page.locator('[data-run="busy-6-1"] .plan-card__open');
  await last.focus();
  await expect(last).toBeInViewport();
  expect(await board.evaluate((el) => el.scrollLeft)).toBeGreaterThan(0);
  expect(await page.evaluate(() => document.scrollingElement!.scrollWidth <= innerWidth + 1)).toBe(true);
});
