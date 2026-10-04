import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, expect, settle, test } from './support';

// The Move picker (boards P_MoveStates, Main, P_MoveWidths, P_MovePhone):
// keyboard, accessible names and state, the clash warning and reduced motion,
// on the run pane, the phone sheet's Move view and the Inbox edit. The mock's
// today is Tue 29 Sep: Thu–Mon are past this week, next week is all open.
test.describe.configure({ mode: 'parallel' });

const column = (page: Page, dow: string) => page.locator('section.board__col').filter({ has: page.locator(`h2 .board__dow:text-is("${dow}")`) });

async function pane(page: Page, path: string, run: string, name: string) {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
  await page.locator(`[data-run="${run}"] .plan-card__open`).click();
  const aside = page.getByRole('complementary', { name });
  await expect(aside).toBeVisible();
  return aside;
}

async function axe(page: Page, include: string) {
  await settle(page);
  const result = await new AxeBuilder({ page }).include(include).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  return result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`);
}

test('day strip: past days are marked and skipped; ←/→, Home and End pick open days', async ({ page }) => {
  const run = await pane(page, '/', 'r-bm', 'XBM');
  const days = run.getByRole('radiogroup', { name: 'Day' });
  const radio = (name: RegExp) => days.getByRole('radio', { name });
  // Past days: aria-disabled, named "past"; today and the reset day say so.
  await expect(radio(/^Thu 24/)).toHaveAccessibleName('Thu 24, reset day, past');
  await expect(radio(/^Sat 26/)).toHaveAttribute('aria-disabled', 'true');
  await expect(radio(/^Tue 29/)).toHaveAccessibleName("Tue 29, today, the run's day, 1 other run");
  await expect(radio(/^Tue 29/)).toHaveAttribute('aria-checked', 'true');
  // A past day does not take a click.
  await radio(/^Mon 28/).click({ force: true });
  await expect(radio(/^Tue 29/)).toHaveAttribute('aria-checked', 'true');
  // One tab stop: the picked day. ← stays (Mon is past), → picks Wed, Home today, End the last day.
  await radio(/^Tue 29/).focus();
  await page.keyboard.press('ArrowLeft');
  await expect(radio(/^Tue 29/)).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(radio(/^Wed 30/)).toBeFocused();
  await expect(radio(/^Wed 30/)).toHaveAttribute('aria-checked', 'true');
  // The run's own day takes the dashed ring; its word stays "today" (reset, then today, then from).
  await expect(radio(/^Tue 29/)).toHaveClass(/daystrip__day--from/);
  await page.keyboard.press('Home');
  await expect(radio(/^Tue 29/)).toBeFocused();
  await page.keyboard.press('End');
  await expect(radio(/^Wed 30/)).toBeFocused();
  await expect(days.getByRole('radio').and(page.locator('[tabindex="0"]'))).toHaveCount(1);
});

test('time: one spinbutton steps by Run lengths, PgUp/PgDn by an hour, wraps at midnight, Enter moves', async ({ page }) => {
  const run = await pane(page, '/', 'r-bm', 'XBM');
  const time = run.getByRole('spinbutton', { name: 'Time' });
  await expect(time).toHaveAttribute('aria-valuetext', '23:30');
  await run.getByRole('radio', { name: /^Wed 30/ }).focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Tab');
  await expect(time).toBeFocused();
  await page.keyboard.press('ArrowUp');
  // Wraps past midnight and stays on the picked day.
  await expect(time).toHaveAttribute('aria-valuetext', '00:00');
  await expect(run.getByRole('radio', { name: /^Wed 30/ })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('PageDown');
  await expect(time).toHaveAttribute('aria-valuetext', '22:30');
  await run.getByRole('button', { name: '30 minutes later' }).click();
  await expect(time).toHaveAttribute('aria-valuetext', '23:00');
  await expect(run.getByText('Moves to')).toBeVisible();
  await expect(run.locator('.movepick__to')).toHaveText('Wed 30 23:00');
  await time.press('Enter');
  await expect(column(page, 'Wed').locator('[data-run="r-bm"]')).toContainText('23:00');
});

test('Escape clears the typed field, then undoes the pick, then closes the pane', async ({ page }) => {
  const run = await pane(page, '/', 'r-bm', 'XBM');
  const typed = run.getByRole('textbox', { name: 'Type a day and time' });
  await run.getByRole('radio', { name: /^Wed 30/ }).click();
  await typed.fill('22:00');
  await expect(run.getByText('reads as Wed 30 22:00')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(typed).toHaveValue('');
  await expect(run.getByRole('spinbutton', { name: 'Time' })).toHaveAttribute('aria-valuetext', '23:30');
  await expect(run.getByRole('radio', { name: /^Wed 30/ })).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('Escape');
  await expect(run.getByRole('radio', { name: /^Tue 29/ })).toHaveAttribute('aria-checked', 'true');
  await expect(run).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(run).toBeHidden();
});

test('suggestions come from the picked day, and a clash warns in words without blocking', async ({ page }) => {
  const run = await pane(page, '/?week=next', 'n-carling', 'HCarling + HStar');
  // Next week: no day is past or today.
  await expect(run.getByRole('radio', { name: /past/ })).toHaveCount(0);
  await run.getByRole('radio', { name: /^Mon 05/ }).click();
  const ideas = run.getByRole('group', { name: 'Suggestions' });
  await expect(ideas.getByRole('button', { name: 'same time 22:00' })).toBeVisible();
  await expect(ideas.getByRole('button', { name: /^after HFA/ })).toBeVisible();
  await expect(ideas.getByRole('button', { name: /^before HFA/ })).toBeVisible();
  await expect(run.getByRole('status').filter({ hasText: 'Clash:' })).toHaveCount(0);
  // 20:00 is HFA's slot, and two of its members play both.
  await run.getByRole('textbox', { name: 'Type a day and time' }).fill('20:00');
  const clash = run.getByRole('status').filter({ hasText: 'Clash:' });
  await expect(clash).toContainText(/in HFA 20:00\. You can still move; they will be double-booked\./);
  const move = run.getByRole('button', { name: 'Move', exact: true });
  await expect(move).toBeEnabled();
  await move.click();
  await expect(column(page, 'Mon').locator('[data-run="n-carling"]')).toContainText('20:00');
});

test('reduced motion: no morph, the clash box fades in over 120 ms', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const run = await pane(page, '/?week=next', 'n-carling', 'HCarling + HStar');
  await run.getByRole('radio', { name: /^Mon 05/ }).click();
  await run.getByRole('textbox', { name: 'Type a day and time' }).fill('20:00');
  const motion = await run.evaluate((el) => {
    const day = getComputedStyle(el.querySelector('.daystrip__day')!);
    const box = getComputedStyle(el.querySelector('.movepick__clash')!);
    return { day: parseFloat(day.transitionDuration), name: box.animationName, box: box.animationDuration };
  });
  expect(motion.day).toBeLessThan(0.01);
  expect(motion).toMatchObject({ name: 'movepick-fade', box: '0.12s' });
});

test('own-time runs keep the day strip; the time waits for "Set a time"', async ({ page }) => {
  const run = await pane(page, '/?week=next', 'n-carling', 'HCarling + HStar');
  await run.getByRole('button', { name: 'Own time' }).click();
  const time = run.getByRole('spinbutton', { name: 'Time' });
  await expect(time).toHaveAttribute('aria-disabled', 'true');
  await expect(time).toHaveAttribute('aria-valuetext', 'no time set');
  await run.getByRole('radio', { name: /^Wed 07/ }).click();
  await expect(run.locator('.movepick__to')).toHaveText('Wed 07 own time');
  await run.getByRole('button', { name: 'Set a time' }).click();
  await expect(time).not.toHaveAttribute('aria-disabled', 'true');
  await expect(run.getByRole('button', { name: 'Keep own time' })).toBeVisible();
});

test('phone sheet: Move opens its own view, Back and Escape return focus, Move closes the sheet', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-bm"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'XBM' });
  const key = sheet.getByRole('button', { name: 'Move XBM…' });
  await key.click();
  const view = page.getByRole('dialog', { name: 'Move XBM' });
  // The picked day takes focus.
  await expect(view.getByRole('radio', { name: /^Tue 29/ })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(sheet).toBeVisible();
  await expect(key).toBeFocused();
  await key.click();
  await view.getByRole('button', { name: 'Back to the run' }).click();
  await expect(key).toBeFocused();
  await key.click();
  await page.keyboard.press('ArrowRight');
  await expect(view.getByRole('radio', { name: /^Wed 30/ })).toHaveAttribute('aria-checked', 'true');
  expect(await axe(page, 'dialog[open]')).toEqual([]);
  // Targets for touch: 44 px chevrons and Move, 64 px days.
  const sizes = await view.evaluate((el) => ({
    day: el.querySelector('.daystrip__day')!.getBoundingClientRect().height,
    chevron: el.querySelector('.timestep__btn')!.getBoundingClientRect().height,
    move: el.querySelector('.movepick__go')!.getBoundingClientRect().height,
  }));
  expect(sizes).toEqual({ day: 64, chevron: 44, move: 44 });
  await view.getByRole('button', { name: 'Move', exact: true }).click();
  await expect(view).toBeHidden();
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('axe: the run pane picker and the Inbox edit picker', async ({ page }) => {
  const run = await pane(page, '/', 'r-bm', 'XBM');
  await run.getByRole('radio', { name: /^Wed 30/ }).click();
  await run.getByRole('textbox', { name: 'Type a day and time' }).fill('sat 9pm');
  await run.getByRole('textbox', { name: 'Type a day and time' }).press('Enter');
  await expect(run.getByRole('alert').filter({ hasText: 'has passed' })).toBeVisible();
  expect(await axe(page, '.week-pane')).toEqual([]);

  await page.goto(`${ADMIN}/inbox?tab=extractor&item=p-bm-move&sw=off`);
  const detail = page.locator('.inbox__detail');
  await detail.getByRole('button', { name: 'Edit, then approve' }).click();
  const picker = detail.getByRole('group', { name: 'Edit, then approve' });
  await expect(picker).toBeVisible();
  expect(await axe(page, '.inbox__detail')).toEqual([]);
  // Cancel edit (or Escape) closes it and returns focus to the button.
  await page.keyboard.press('Escape');
  await expect(picker).toHaveCount(0);
  await expect(detail.getByRole('button', { name: 'Edit, then approve' })).toBeFocused();
});
