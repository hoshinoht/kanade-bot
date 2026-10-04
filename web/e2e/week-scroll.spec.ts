import { ADMIN, expect, test } from './support';

// A busy week keeps v4's 230 px column floor and scrolls sideways, fading the
// edge that has more board past it (user decision 2026-10-04). On Next week
// the glance still shows the next run, which usually lives in this week.

test('Week: busy columns keep their floor and the board fades the edge with more past it', async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 800 });
  await page.goto(`${ADMIN}/?week=next&sw=off`);
  const board = page.locator('.week-window .board');
  await expect(board.locator('section.board__col').first()).toBeVisible();
  const widths = await board
    .locator('section.board__col:not(.board__col--empty)')
    .evaluateAll((cols) => cols.map((c) => c.getBoundingClientRect().width));
  expect(widths.length).toBeGreaterThan(0);
  for (const w of widths) expect(w).toBeGreaterThanOrEqual(229.5);

  const overflow = await board.evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(overflow, 'the mock week is busy enough to scroll at 1000 px').toBeGreaterThan(1);
  await expect(board).toHaveClass(/scroll-more-end/);
  await expect(board).not.toHaveClass(/scroll-more-start/);
  await board.evaluate((el) => el.scrollTo({ left: el.scrollWidth }));
  await expect(board).toHaveClass(/scroll-more-start/);
  await expect(board).not.toHaveClass(/scroll-more-end/);
});

test('Week: a reset day head keeps the run count apart from the reset mark', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${ADMIN}/?week=next&sw=off`);
  // The mock's reset day is empty, so mark a busy head as the reset day.
  const head = page.locator('.week-window .board__head:has(.board__count)').first();
  await expect(head).toBeVisible();
  const gap = await head.evaluate((h) => {
    const mark = document.createElement('span');
    mark.className = 'board__reset';
    mark.textContent = 'reset';
    h.querySelector('.board__count')!.after(mark);
    return mark.getBoundingClientRect().left - h.querySelector('.board__count')!.getBoundingClientRect().right;
  });
  expect(gap).toBeGreaterThanOrEqual(6);
});

test('Week: on Next week the glance still shows the next run, and Open sheet goes back to it', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${ADMIN}/?week=next&sw=off`);
  const glance = page.locator('.week-glance');
  await expect(glance.getByRole('progressbar', { name: /^Countdown to / })).toBeVisible();
  await expect(glance.locator('.week-glance__bosses .bosstag, .week-glance__bosses > *').first()).toBeVisible();
  await glance.getByRole('button', { name: 'Open sheet' }).click();
  await expect(page).toHaveURL(`${ADMIN}/`);
  await expect(page.locator('.week-pane')).toBeVisible();
});
