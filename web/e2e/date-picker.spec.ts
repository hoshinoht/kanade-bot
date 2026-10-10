import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, expect, settle, test, choose } from './support';

// The date picker (boards P_Dates, P_DatesSpec, P_DatesPhone): Chat and
// Extractions' From–To range and History's "Since". The mock's clock is Tue
// 29 Sep 2026 12:00 in Kuala Lumpur, so today is the 29th and the boss week
// started Thu 24 Sep; later days are disabled.

test.describe.configure({ mode: 'parallel' });

async function openRange(page: Page, path = '/chat') {
  await page.goto(`${ADMIN}${path}?sw=off`);
  await page.getByRole('button', { name: /^Filters/ }).click();
  const trigger = page.getByRole('button', { name: /^Dates/ });
  await expect(trigger).toHaveText(/any date/);
  await trigger.click();
  const dialog = page.getByRole('dialog', { name: /Date range|Dates/ });
  await expect(dialog).toBeVisible();
  return { trigger, dialog };
}

async function serious(page: Page, label: string) {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

test('chat range: Thursday-first grid, server today, keyboard start and end, Apply', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { trigger, dialog } = await openRange(page);
  const grid = dialog.getByRole('grid', { name: 'September 2026' });
  await expect(grid.getByRole('columnheader')).toHaveText(['THU', 'FRI', 'SAT', 'SUN', 'MON', 'TUE', 'WED']);
  // One boss week is one row.
  await expect(grid.getByRole('row').nth(5).getByRole('gridcell')).toHaveText(['24', '25', '26', '27', '28', '29', '30']);
  const today = grid.getByRole('gridcell', { name: 'Tuesday 29 September 2026, today' });
  await expect(today).toBeFocused();
  await expect(grid.getByRole('gridcell', { name: /^Wednesday 30 September 2026, in the future$/ })).toHaveAttribute('aria-disabled', 'true');

  // ← day, ↑ week back, ↓ week forward, Home: the boss week's Thursday.
  await page.keyboard.press('ArrowLeft');
  await expect(grid.getByRole('gridcell', { name: /^Monday 28 September/ })).toBeFocused();
  await page.keyboard.press('ArrowUp');
  await expect(grid.getByRole('gridcell', { name: /^Monday 21 September/ })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Home');
  await expect(grid.getByRole('gridcell', { name: /^Thursday 24 September/ })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(dialog.getByText('Thu 24 Sep → …')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Apply' })).toBeDisabled();
  // End: Wednesday, held to today.
  await page.keyboard.press('End');
  await expect(today).toBeFocused();
  await page.keyboard.press('Space');
  await expect(grid.getByRole('gridcell', { selected: true })).toHaveCount(6);
  await expect(grid.getByRole('gridcell', { name: /^Thursday 24 September 2026, range start$/ })).toBeVisible();
  await expect(dialog.locator('.datepick__summary')).toHaveText(/Thu 24 Sep – Tue 29 Sep\s*6 days · Kuala Lumpur time/);
  await dialog.getByRole('button', { name: 'Apply' }).click();
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(/from=2026-09-24&to=2026-09-29/);
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveText(/Thu 24 Sep – Tue 29 Sep/);
  await expect(trigger).toHaveClass(/datepick-trigger--set/);
  // The trigger shows the range in the filter row, so there is no separate Dates chip.
  await expect(page.getByRole('button', { name: /^Dates: .* — remove$/ })).toHaveCount(0);

  // Escape closes the picker only: the value and the Filters popover stay.
  await trigger.click();
  await expect(dialog).toBeVisible();
  await page.keyboard.press('PageUp');
  await page.keyboard.press('PageUp');
  await expect(dialog.getByRole('grid', { name: 'August 2026' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
  await expect(page.getByRole('group', { name: 'Filters' })).toBeVisible();
  await expect(page).toHaveURL(/from=2026-09-24&to=2026-09-29/);
});

test('chat range: quick chips, a one-day range, typed-field errors, Any date', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { trigger, dialog } = await openRange(page);
  const chip = dialog.getByRole('button', { name: 'Last boss week' });
  await chip.click();
  await expect(chip).toHaveAttribute('aria-pressed', 'true');
  await expect(dialog.getByLabel('From date')).toHaveValue('2026-09-17');
  await expect(dialog.getByLabel('To date')).toHaveValue('2026-09-23');
  await dialog.getByRole('button', { name: 'This boss week' }).click();
  await dialog.getByRole('button', { name: 'Apply' }).click();
  await expect(page).toHaveURL(/from=2026-09-24&to=2026-09-29/);

  // The same day twice is a one-day range, shown as one date.
  await trigger.click();
  const day = dialog.getByRole('gridcell', { name: /^Saturday 26 September/ });
  await day.click();
  await day.click();
  await dialog.getByRole('button', { name: 'Apply' }).click();
  await expect(trigger).toHaveText(/Sat 26 Sep/);
  await expect(trigger).not.toHaveText(/–/);
  await expect(page).toHaveURL(/from=2026-09-26&to=2026-09-26/);

  // Typed fields: errors say what is wrong and block Apply.
  await trigger.click();
  const from = dialog.getByLabel('From date');
  const to = dialog.getByLabel('To date');
  await from.fill('2026-09-28');
  await to.fill('2026-09-25');
  await to.press('Enter');
  await expect(dialog.getByRole('alert')).toHaveText('To (Fri 25 Sep) is before From (Mon 28 Sep).');
  await expect(to).toHaveAttribute('aria-invalid', 'true');
  await expect(dialog.getByRole('button', { name: 'Apply' })).toBeDisabled();
  await from.fill('2026-02-30');
  await to.fill('2026-09-29');
  await to.press('Enter');
  await expect(dialog.getByRole('alert')).toHaveText('That day doesn’t exist in that month.');
  await expect(from).toHaveAttribute('aria-invalid', 'true');
  await from.fill('2026-09-21');
  await from.press('Enter');
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  await expect(dialog.locator('.datepick__summary')).toContainText('Mon 21 Sep – Tue 29 Sep');
  await dialog.getByRole('button', { name: 'Apply' }).click();
  await expect(page).toHaveURL(/from=2026-09-21&to=2026-09-29/);

  // Any date clears both ends.
  await trigger.click();
  await dialog.getByRole('button', { name: 'Any date' }).click();
  await expect(page).not.toHaveURL(/from=/);
  await expect(trigger).toHaveText(/any date/);
});

test('extractions range: the same picker, and axe finds nothing serious with it open', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { dialog } = await openRange(page, '/extractions');
  await dialog.getByRole('button', { name: 'Last 7 days' }).click();
  await serious(page, 'extractions date range');
  await dialog.getByRole('button', { name: 'Apply' }).click();
  await expect(page).toHaveURL(/from=2026-09-23&to=2026-09-29/);
});

test('history since: picks on click, closes, and sends the start of that guild day', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/history?sw=off`);
  const pane = page.getByRole('complementary', { name: 'Change details' });
  const trigger = pane.getByRole('button', { name: /^Since/ });
  await expect(trigger).toHaveText(/any date/);
  await trigger.click();
  const dialog = page.getByRole('dialog', { name: 'Since' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Apply' })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Last reset · Thu 24' })).toBeVisible();
  await serious(page, 'history since');
  await dialog.getByRole('gridcell', { name: /^Friday 25 September/ }).click();
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveText(/Fri 25 Sep/);

  await trigger.click();
  await expect(dialog.locator('.datepick__summary')).toHaveText(/Changes since\s*Fri 25 Sep 00:00\s*Kuala Lumpur time/);
  await dialog.getByRole('button', { name: 'Last reset · Thu 24' }).click();
  await expect(trigger).toHaveText(/Thu 24 Sep/);

  await choose(pane.getByLabel('Member'), { label: 'Rin' });
  const preview = page.waitForRequest((r) => r.url().endsWith('/api/admin/history/revert-actor'));
  await pane.getByRole('button', { name: 'Preview' }).click();
  expect((await preview).postDataJSON()).toMatchObject({ since: '2026-09-24T00:00:00' });
  await expect(page.getByRole('dialog', { name: /^Revert everything Rin changed since 2026-09-24/ })).toBeVisible();
});

test('phone: the range opens as a bottom sheet and taps a start and an end', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const { trigger, dialog } = await openRange(page);
  await settle(page);
  const box = (await dialog.boundingBox())!;
  expect(Math.round(box.y + box.height)).toBe(844);
  expect(box.width).toBe(390);
  const cell = (await dialog.getByRole('gridcell').first().boundingBox())!;
  expect(cell.height).toBeGreaterThanOrEqual(44);
  await expect(dialog.getByRole('heading', { name: 'Dates' })).toBeVisible();
  await dialog.getByRole('gridcell', { name: /^Thursday 24 September/ }).click();
  await expect(dialog.getByText('tap an end day')).toBeVisible();
  await dialog.getByRole('gridcell', { name: /^Sunday 27 September/ }).click();
  await serious(page, 'phone date sheet');
  await dialog.getByRole('button', { name: 'Apply' }).click();
  await expect(page).toHaveURL(/from=2026-09-24&to=2026-09-27/);
  await expect(trigger).toBeFocused();
  expect(await page.evaluate(() => document.scrollingElement!.scrollWidth <= innerWidth)).toBe(true);

  // The close button and the scrim keep the value.
  await trigger.click();
  await dialog.getByRole('button', { name: 'Close' }).click();
  await expect(dialog).toBeHidden();
  await trigger.click();
  await page.mouse.click(195, 40);
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(/from=2026-09-24&to=2026-09-27/);
});

test('motion: 180 ms drop, months cross-fade; reduced motion is a 120 ms fade only', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const anim = (sel: string) => page.locator(sel).evaluate((el) => ({ name: getComputedStyle(el).animationName, ms: getComputedStyle(el).animationDuration }));
  await openRange(page);
  expect(await anim('dialog.datepick')).toEqual({ name: 'datepick-drop', ms: '0.18s' });
  expect((await anim('.dp-grid')).name).toMatch(/^datepick-month-/);
  await page.keyboard.press('Escape');

  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.getByRole('button', { name: /^Dates/ }).click();
  expect(await anim('dialog.datepick')).toEqual({ name: 'datepick-fade', ms: '0.12s' });
  expect((await anim('.dp-grid')).name).toBe('none');
  // Picks change at once (the app-wide reduced-motion rule leaves 0.001 ms).
  expect(await page.locator('.dp-day').first().evaluate((el) => parseFloat(getComputedStyle(el).transitionDuration))).toBeLessThan(0.001);
});
