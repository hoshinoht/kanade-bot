import AxeBuilder from '@axe-core/playwright';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, expect, settle, test } from './support';

// The dropdown (boards P_Select, P_SelectSpec, P_SelectPhone): a select-only
// combobox whose focus stays on the trigger, a search box above ten options,
// the Re-read multi-select, and the phone's native picker under the pill.

test.describe.configure({ mode: 'parallel' });

async function serious(page: Page, label: string) {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

/** The option the combobox (or search box) points at. */
async function active(owner: Locator): Promise<string | null> {
  const id = await owner.getAttribute('aria-activedescendant');
  if (!id) return null;
  return owner.page().locator(`[id="${id}"]`).getAttribute('data-value');
}

async function reminderFilters(page: Page) {
  await page.goto(`${ADMIN}/reminders?sw=off`);
  await page.getByRole('button', { name: /^Filters/ }).click();
  return page.getByRole('group', { name: 'Filter reminders' });
}

test('keyboard: open on the value, arrows, Home/End, type-ahead, Enter picks, Escape and Tab keep the value', async ({ page }) => {
  const filters = await reminderFilters(page);
  const kind = filters.getByRole('combobox', { name: 'Kind' });
  await expect(kind).toBeFocused();
  await expect(kind).toHaveAttribute('aria-haspopup', 'listbox');
  await expect(kind).toHaveAttribute('aria-expanded', 'false');

  await page.keyboard.press('ArrowDown');
  const list = page.getByRole('listbox', { name: 'Kind' });
  await expect(list).toBeVisible();
  const values = await list.getByRole('option').evaluateAll((os) => os.map((o) => (o as HTMLElement).dataset.value!));
  expect(values[0]).toBe('');
  await expect(kind).toHaveAttribute('aria-expanded', 'true');
  // Focus stays on the trigger; the active row is the current value.
  await expect(kind).toBeFocused();
  expect(await active(kind)).toBe('');
  await expect(list.getByRole('option', { selected: true })).toHaveAttribute('data-value', '');

  await page.keyboard.press('ArrowDown');
  expect(await active(kind)).toBe(values[1]);
  await page.keyboard.press('End');
  expect(await active(kind)).toBe(values.at(-1));
  await page.keyboard.press('Home');
  expect(await active(kind)).toBe('');
  // Type-ahead: the next row starting with the letter.
  await page.keyboard.press('t');
  const firstT = await list.locator('.dd-opt__label', { hasText: /^T/ }).first().textContent();
  expect(await active(kind)).toBe(firstT);

  // Escape closes with the value unchanged and stops there (the filters popover stays).
  await page.keyboard.press('Escape');
  await expect(list).toBeHidden();
  await expect(kind).toHaveAttribute('data-value', '');
  await expect(filters).toBeVisible();
  await expect(kind).toBeFocused();

  // Enter (and Space) open; Enter picks and closes.
  await page.keyboard.press('Enter');
  await expect(list).toBeVisible();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(list).toBeHidden();
  await expect(kind).toHaveAttribute('data-value', values[1]!);
  await expect(kind).toBeFocused();
  await expect(page.getByRole('button', { name: 'Filters (1)' })).toBeVisible();
  // A chosen filter value draws the pill tonal.
  await expect(kind).toHaveClass(/dd--set/);

  // Tab closes too, value unchanged, and focus moves on.
  await page.keyboard.press(' ');
  await expect(list).toBeVisible();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Tab');
  await expect(list).toBeHidden();
  await expect(kind).toHaveAttribute('data-value', values[1]!);
  await expect(kind).not.toBeFocused();

  // Escape on a closed trigger reaches the filters popover.
  await kind.focus();
  await page.keyboard.press('Escape');
  await expect(filters).toBeHidden();
});

test('pointer: a press picks, a press outside closes, and the chevron turns', async ({ page }) => {
  const filters = await reminderFilters(page);
  const day = filters.getByRole('combobox', { name: 'Day' });
  await day.click();
  const list = page.getByRole('listbox', { name: 'Day' });
  await expect(list).toBeVisible();
  await expect(day.locator('.dd__chev')).toHaveCSS('transform', /matrix\(-1/);
  // The popover sits under the trigger, placed through CSSOM custom properties.
  const [t, p] = [(await day.boundingBox())!, (await list.locator('xpath=..').boundingBox())!];
  expect(p.y).toBeGreaterThanOrEqual(t.y + t.height);
  expect(Math.abs(p.x - t.x)).toBeLessThan(2);
  const second = list.getByRole('option').nth(1);
  const value = await second.getAttribute('data-value');
  await second.click();
  await expect(list).toBeHidden();
  await expect(day).toHaveAttribute('data-value', value!);
  await expect(day).toBeFocused();
  await expect(filters).toBeVisible();
  // A press outside closes the list (and the filters popover around it).
  await day.click();
  await expect(list).toBeVisible();
  await page.getByRole('heading', { level: 1 }).click();
  await expect(list).toBeHidden();
  await expect(filters).toBeHidden();
});

test('search above ten options: focus in the box, arrows drive the list, groups drop out, no match', async ({ page }) => {
  await page.goto(`${ADMIN}/history?sw=off`);
  const who = page.getByRole('combobox', { name: 'Who' });
  await who.click();
  const search = page.getByRole('combobox', { name: 'Filter people' });
  await expect(search).toBeFocused();
  const list = page.getByRole('listbox', { name: 'Who' });
  await expect(list.getByRole('group', { name: 'Members' })).toBeVisible();
  await expect(list.getByRole('group', { name: 'System' })).toBeVisible();

  await search.fill('rin');
  await expect(list.getByRole('group', { name: 'System' })).toHaveCount(0);
  await expect(list.getByRole('option')).toHaveText([/Rin/]);
  expect(await active(search)).toMatch(/^member:/);
  await expect(list.locator('mark')).toHaveText('Rin');

  await search.fill('zzk');
  await expect(list.getByRole('option')).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: 'Nothing matches “zzk”.' })).toBeVisible();
  await page.getByRole('button', { name: 'Clear search' }).click();
  await expect(search).toHaveValue('');
  await expect(search).toBeFocused();

  await search.fill('sora');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Enter');
  await expect(list).toBeHidden();
  await expect(who).toHaveAttribute('data-value', /^member:/);
  await expect(who).toContainText('Sora');
  await expect(who).toBeFocused();
  await expect(page.getByRole('heading', { level: 1 })).not.toHaveText('9 changes');
});

test('multi-select: Space toggles and stays open, the trigger counts, All · None, Enter closes', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=rescan&sw=off`);
  const channels = page.getByRole('combobox', { name: 'Channels to re-read' });
  await expect(channels).toHaveText(/none/);
  await channels.focus();
  await page.keyboard.press('ArrowDown');
  const list = page.getByRole('listbox', { name: 'Channels to re-read' });
  await expect(list).toHaveAttribute('aria-multiselectable', 'true');
  const total = await list.getByRole('option').count();
  const first = (await list.locator('.dd-opt__label').first().textContent())!;
  await page.keyboard.press(' ');
  await expect(list).toBeVisible();
  await expect(list.getByRole('option').first()).toHaveAttribute('aria-selected', 'true');
  // With one picked, the trigger names it.
  await expect(channels).toContainText(first);
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press(' ');
  await expect(channels).toContainText(`2 of ${total}`);
  await expect(list.locator('..')).toContainText('2 selected');
  await list.getByRole('option').nth(1).click();
  await expect(channels).toContainText(`3 of ${total}`);
  await expect(channels).toBeFocused();
  await page.getByRole('button', { name: 'None', exact: true }).click();
  await expect(channels).toContainText('none');
  await page.getByRole('button', { name: 'All', exact: true }).click();
  await expect(channels).toContainText(`all ${total}`);
  await channels.focus();
  await page.keyboard.press('Enter');
  await expect(list).toBeHidden();
  await expect(channels).toBeFocused();
});

test('motion: a 180 ms drop, and a 120 ms fade only under reduced motion', async ({ page }) => {
  const filters = await reminderFilters(page);
  const kind = filters.getByRole('combobox', { name: 'Kind' });
  const pop = (box: Locator) => box.locator('xpath=following-sibling::div[@popover]');
  await kind.click();
  const anim = () => pop(kind).evaluate((el) => ({ name: getComputedStyle(el).animationName, ms: getComputedStyle(el).animationDuration }));
  expect(await anim()).toEqual({ name: 'dd-drop', ms: '0.18s' });
  await page.keyboard.press('Escape');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await kind.click();
  expect(await anim()).toEqual({ name: 'dd-fade', ms: '0.12s' });
});

test('a11y: open single, searchable and multi dropdowns pass axe', async ({ page }) => {
  const filters = await reminderFilters(page);
  await filters.getByRole('combobox', { name: 'Run' }).click();
  await serious(page, 'reminders run');

  await page.goto(`${ADMIN}/history?sw=off`);
  await page.getByRole('combobox', { name: 'Who' }).click();
  await page.getByRole('combobox', { name: 'Filter people' }).fill('r');
  await serious(page, 'history who');

  await page.goto(`${ADMIN}/config?section=run-lengths&sw=off`);
  const panel = page.getByRole('tabpanel', { name: 'Run lengths' });
  await panel.getByRole('button', { name: 'Add an override' }).click();
  await panel.getByRole('button', { name: 'Save run lengths' }).click();
  const boss = panel.getByRole('combobox', { name: 'Boss', exact: true }).last();
  await expect(boss).toHaveAttribute('aria-invalid', 'true');
  await expect(boss).toHaveAccessibleDescription('Pick a boss before saving this length.');
  await expect(panel.getByRole('combobox', { name: 'Difficulty' }).last()).toBeDisabled();
  await expect(panel.getByRole('combobox', { name: 'Difficulty' }).last()).toContainText('pick a boss first');
  await serious(page, 'run lengths error');

  await page.goto(`${ADMIN}/config?section=rescan&sw=off`);
  await page.getByRole('combobox', { name: 'Channels to re-read' }).click();
  await serious(page, 'reread multi');
});

test('phones: a real, transparent native select sits under each pill', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const filters = await reminderFilters(page);
  const kind = filters.getByRole('combobox', { name: 'Kind' });
  const native = await kind.evaluate((el) => {
    const pill = el.closest('.dd')!.getBoundingClientRect();
    const box = el.getBoundingClientRect();
    return { tag: el.tagName, opacity: getComputedStyle(el).opacity, same: Math.abs(pill.width - box.width) < 1 && Math.abs(pill.height - box.height) < 1, height: pill.height };
  });
  expect(native).toEqual({ tag: 'SELECT', opacity: '0', same: true, height: 44 });
  // The top element at the pill's centre is the select, so a tap opens the phone's picker.
  const hit = await kind.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2) === el;
  });
  expect(hit).toBe(true);
  await kind.selectOption({ index: 1 });
  await expect(page.getByRole('button', { name: 'Filters (1)' })).toBeVisible();
  await expect(filters.locator('.dd--set')).toHaveCount(1);

  // The multi-select opens a full-screen sheet of checkboxes instead.
  await page.goto(`${ADMIN}/config?section=rescan&sw=off`);
  const channels = page.getByRole('button', { name: 'Channels to re-read' });
  await expect(channels).toHaveAttribute('aria-haspopup', 'dialog');
  await channels.click();
  const sheet = page.getByRole('dialog', { name: 'Channels' });
  await expect(sheet).toBeVisible();
  const size = await sheet.evaluate((el) => [el.getBoundingClientRect().width, el.getBoundingClientRect().height]);
  expect(size).toEqual([390, 844]);
  await sheet.getByRole('checkbox').nth(1).check();
  await sheet.getByRole('button', { name: /^Done/ }).click();
  await expect(sheet).toBeHidden();
  await expect(channels).toBeFocused();
  await expect(channels).not.toContainText('none');
});

test('a coarse pointer on a wide screen also gets the native picker', async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, hasTouch: true, isMobile: false });
  const page = await context.newPage();
  const filters = await reminderFilters(page);
  const coarse = await page.evaluate(() => matchMedia('(pointer: coarse)').matches);
  test.skip(!coarse, 'this Chrome does not report a coarse pointer for touch emulation');
  await expect(filters.getByRole('combobox', { name: 'Kind' })).toHaveJSProperty('tagName', 'SELECT');
  await context.close();
});

test('Reminders Run list groups runs by day with their time and queued count', async ({ page }) => {
  const filters = await reminderFilters(page);
  await filters.getByRole('combobox', { name: 'Run' }).click();
  const list = page.getByRole('listbox').last();
  await expect(list.getByRole('group').first()).toHaveAccessibleName(/^\w{3} \d{1,2} \w{3}$/);
  await expect(list.getByRole('option').nth(1)).toContainText(/\d{2}:\d{2} · \d+ queued|none queued/);
});
