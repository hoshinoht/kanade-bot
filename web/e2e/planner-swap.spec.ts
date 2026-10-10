import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, csrf, expect, test, choose, optionLabels } from './support';

// Workplan step planner-swap (user decisions 2026-10-01): releasing a dragged
// run on another card's middle swaps their slots (between cards still moves);
// S during a keyboard lift swaps with the run on that slot; the run sheet has
// "Swap timing with…". One server change, both cards together, one-step undo.
// Seed: Mon HFA 20:00 (r-fa), own-time NBellona (r-bellona); Tue HCarling +
// HStar 22:00 (r-carling), XBM 23:30 (r-bm).

async function openAdmin(page: Page) {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.getByRole('tab', { name: 'Planner' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  await expect(page.locator('.board[data-hydrated]')).toBeVisible();
}

async function slots(page: Page, ...ids: string[]) {
  const week = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number; runs: { id: string; day: number; time: string | null }[] };
  return { version: week.version, runs: ids.map((id) => week.runs.find((r) => r.id === id)!).map((r) => ({ id: r.id, day: r.day, time: r.time })) };
}

/** Waits out the FLIP glide (and an emptied day collapsing) so boxes are final. */
async function settled(page: Page, ...ids: string[]) {
  let last = '';
  await expect
    .poll(
      async () => {
        const now = JSON.stringify(await Promise.all(ids.map((id) => page.locator(`[data-run="${id}"]`).boundingBox())));
        const same = now === last;
        last = now;
        return same;
      },
      { intervals: [150] },
    )
    .toBe(true);
}

/** The middle of a card: a drop there swaps with it. */
async function onCard(page: Page, runId: string) {
  const box = (await page.locator(`[data-run="${runId}"]`).boundingBox())!;
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

async function pointerDrag(page: Page, runId: string, to: { x: number; y: number }, beforeDrop?: () => Promise<void>) {
  const from = (await page.locator(`[data-run="${runId}"]`).boundingBox())!;
  const [x0, y0] = [from.x + from.width / 2, from.y + from.height / 2];
  await page.mouse.move(x0, y0);
  await page.mouse.down();
  for (let i = 1; i <= 12; i++) await page.mouse.move(x0 + ((to.x - x0) * i) / 12, y0 + ((to.y - y0) * i) / 12);
  await page.mouse.move(to.x + 1, to.y + 1);
  await beforeDrop?.();
  await page.mouse.up();
}

test('pointer: a drop on another card swaps the two slots, saves both, and undo swaps them back', async ({ page }) => {
  await openAdmin(page);
  await pointerDrag(page, 'r-fa', await onCard(page, 'r-bm'), async () => {
    await expect(page.locator('.dnd-ghost--on')).toContainText('Swap with XBM: Mon 28 20:00 ⇄ Tue 29 23:30');
    await expect(page.locator('[data-run="r-bm"]')).toHaveClass(/plan-card--swap-target/);
    // No "between" bar while over a card's middle.
    await expect(page.locator('.plan-card--drop-before, .plan-card--drop-after')).toHaveCount(0);
    await expect(page.locator('[aria-live="polite"]').filter({ hasText: 'Swap with XBM: Mon 28 20:00 ⇄ Tue 29 23:30.' })).toHaveCount(1);
    const scan = await new AxeBuilder({ page }).include('.planner').withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa']).analyze();
    expect(scan.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => v.id)).toEqual([]);
  });
  await expect(page.getByText('Swapped HFA to Tue 29 23:30, XBM to Mon 28 20:00.')).toBeVisible();
  expect((await slots(page, 'r-fa', 'r-bm')).runs).toEqual([
    { id: 'r-fa', day: 5, time: '23:30' },
    { id: 'r-bm', day: 4, time: '20:00' },
  ]);
  // The drop does not open the sheet.
  await expect(page.getByRole('dialog')).toBeHidden();

  // One step back: the page's Undo control (and Ctrl/Cmd+Z).
  await page.getByRole('button', { name: 'Undo swap' }).click();
  await expect(page.getByText('Swap undone: HFA to Mon 28 20:00, XBM to Tue 29 23:30.')).toBeVisible();
  expect((await slots(page, 'r-fa', 'r-bm')).runs).toEqual([
    { id: 'r-fa', day: 4, time: '20:00' },
    { id: 'r-bm', day: 5, time: '23:30' },
  ]);
});

test('a later move retires the earlier swap toast undo; its own undo only reverts that move', async ({ page }) => {
  await openAdmin(page);
  await pointerDrag(page, 'r-fa', await onCard(page, 'r-bm'));
  const swapped = 'Swapped HFA to Tue 29 23:30, XBM to Mon 28 20:00.';
  await expect(page.getByText(swapped)).toBeVisible();
  await settled(page, 'r-fa', 'r-carling');

  const carling = (await page.locator('[data-run="r-carling"]').boundingBox())!;
  await pointerDrag(page, 'r-fa', { x: carling.x + carling.width / 2, y: carling.y + carling.height * 0.1 });
  await expect(page.getByText('Moved HFA to Tue 29 21:30.')).toBeVisible();
  await expect(page.getByText(swapped)).toHaveCount(0);
  await page.getByRole('button', { name: 'Undo move' }).click();
  await expect(page.getByText('Move undone: HFA to Tue 29 23:30.')).toBeVisible();
  expect((await slots(page, 'r-fa', 'r-bm')).runs).toEqual([
    { id: 'r-fa', day: 5, time: '23:30' },
    { id: 'r-bm', day: 4, time: '20:00' },
  ]);
});

test('pointer: own-time swaps exchange days only, worded as such', async ({ page }) => {
  await openAdmin(page);
  await pointerDrag(page, 'r-bellona', await onCard(page, 'r-carling'), async () => {
    await expect(page.locator('.dnd-ghost--on')).toContainText('Swap days with HCarling + HStar: Mon 28 ⇄ Tue 29, times kept');
  });
  await expect(page.getByText(/^Swapped NBellona to Tue 29/)).toBeVisible();
  expect((await slots(page, 'r-bellona', 'r-carling')).runs).toEqual([
    { id: 'r-bellona', day: 5, time: null },
    { id: 'r-carling', day: 4, time: '22:00' },
  ]);
});

test('pointer: between two cards still moves (no swap)', async ({ page }) => {
  await openAdmin(page);
  const box = (await page.locator('[data-run="r-bm"]').boundingBox())!;
  await pointerDrag(page, 'r-fa', { x: box.x + box.width / 2, y: box.y + box.height * 0.1 }, async () => {
    await expect(page.locator('.dnd-ghost--on')).toContainText('→ 23:00');
    await expect(page.locator('.plan-card--swap-target')).toHaveCount(0);
  });
  await expect(page.getByText('Moved HFA to Tue 29 23:00.')).toBeVisible();
  expect((await slots(page, 'r-bm')).runs[0]).toEqual({ id: 'r-bm', day: 5, time: '23:30' });
});

test.describe('touch', () => {
  test.use({ hasTouch: true, viewport: { width: 1280, height: 800 } });

  test('a long press dropped on a card swaps the two', async ({ page }) => {
    await openAdmin(page);
    const cdp = await page.context().newCDPSession(page);
    const touch = (type: string, x?: number, y?: number) =>
      cdp.send('Input.dispatchTouchEvent', { type, touchPoints: x === undefined ? [] : [{ x, y: y! }] });
    const from = (await page.locator('[data-run="r-fa"]').boundingBox())!;
    const to = await onCard(page, 'r-bm');
    const [x0, y0] = [from.x + from.width / 2, from.y + from.height / 2];
    await touch('touchStart', x0, y0);
    await page.waitForTimeout(400);
    for (let i = 1; i <= 12; i++) await touch('touchMove', x0 + ((to.x - x0) * i) / 12, y0 + ((to.y - y0) * i) / 12);
    await touch('touchMove', to.x + 1, to.y + 1);
    await expect(page.locator('.dnd-ghost--on')).toContainText('Swap with XBM');
    await touch('touchEnd');
    await expect(page.getByText('Swapped HFA to Tue 29 23:30, XBM to Mon 28 20:00.')).toBeVisible();
    await expect(page.getByRole('dialog')).toBeHidden();
  });
});

test('keyboard: S on another run’s slot swaps; S with nobody there says so', async ({ page }) => {
  await openAdmin(page);
  const handle = page.locator('[data-handle="r-fa"]');
  await handle.focus();
  await page.keyboard.press('m');
  await page.keyboard.press('s');
  await expect(page.locator('[aria-live="assertive"]')).toContainText("Nothing to swap with on Mon 28, 20:00: move onto another run's slot first.");
  // Tue 20:00, then down four half-hours onto HCarling + HStar at 22:00.
  await page.keyboard.press('ArrowRight');
  for (let i = 0; i < 4; i++) await page.keyboard.press('ArrowDown');
  await expect(page.locator('[aria-live="assertive"]')).toContainText('HFA: Tue 29, 22:00. HCarling + HStar is here: S swaps with it');
  await expect(page.locator('section.board__col[data-day="5"] .plan-preview')).toContainText('S: Swap with HCarling + HStar: Mon 28 20:00 ⇄ Tue 29 22:00');
  await page.keyboard.press('s');
  await expect(page.getByText('Swapped HFA to Tue 29 22:00, HCarling + HStar to Mon 28 20:00.')).toBeVisible();
  expect((await slots(page, 'r-fa', 'r-carling')).runs).toEqual([
    { id: 'r-fa', day: 5, time: '22:00' },
    { id: 'r-carling', day: 4, time: '20:00' },
  ]);
  await expect(handle).toBeFocused();
});

test('run sheet: "Swap timing with…" picks a run, previews both slots, confirms and saves', async ({ page }) => {
  await openAdmin(page);
  await page.locator('[data-run="r-fa"] .plan-card__open').click();
  const sheet = page.getByRole('complementary', { name: 'HFA' });
  const toggle = sheet.getByRole('button', { name: 'Swap timing with…' });
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  const picker = sheet.getByRole('combobox', { name: 'Swap with' });
  await expect(picker).toBeFocused();
  // Only live runs of this week, never itself.
  expect((await optionLabels(picker)).filter((l) => l.includes('HFA'))).toEqual([]);
  await choose(picker, { label: 'Tue 29 23:30 · XBM' });
  const group = sheet.getByRole('group', { name: /Swap HFA's timing/ });
  await expect(group).toContainText('HFA → Tue 29 23:30; XBM → Mon 28 20:00.');
  // Cancel closes it and returns focus to the toggle.
  await group.getByRole('button', { name: 'Cancel' }).click();
  await expect(toggle).toBeFocused();
  await toggle.click();
  await choose(picker, { label: 'Tue 29 23:30 · XBM' });
  await sheet.getByRole('button', { name: 'Swap', exact: true }).click();
  // The pane stays on the run; the picker folds away.
  await expect(group).toBeHidden();
  await expect(page.getByText('Swapped HFA to Tue 29 23:30, XBM to Mon 28 20:00.')).toBeVisible();
  expect((await slots(page, 'r-fa', 'r-bm')).runs).toEqual([
    { id: 'r-fa', day: 5, time: '23:30' },
    { id: 'r-bm', day: 4, time: '20:00' },
  ]);
});

test('a change by someone else during the drag: the swap conflicts and both cards stay put', async ({ page, request }) => {
  await openAdmin(page);
  const { version: start } = await slots(page);
  const sent: { with: string; version: number }[] = [];
  page.on('request', (r) => {
    if (r.method() === 'POST' && r.url().endsWith('/api/admin/runs/r-fa/swap')) sent.push(r.postDataJSON() as { with: string; version: number });
  });
  await pointerDrag(page, 'r-fa', await onCard(page, 'r-bm'), async () => {
    const moved = await request.post(`${ADMIN}/api/admin/runs/r-limbo/move`, { headers: await csrf(request), data: { day: 1, time: '23:45', version: start } });
    expect(moved.ok()).toBe(true);
    const polled = page.waitForResponse((r) => r.url().includes('/api/admin/week') && r.request().method() === 'GET');
    await page.evaluate(() => window.dispatchEvent(new Event('online')));
    await polled;
    await expect(page.locator('.dnd-ghost--on')).toContainText('Swap with XBM');
  });
  await expect(page.getByText(/^Couldn't swap HFA with XBM/)).toBeVisible();
  expect(sent).toEqual([{ with: 'r-bm', version: start }]);
  // Both cards back where they were, and the other admin's change shows.
  await expect(page.locator('section.board__col[data-day="4"] [data-run="r-fa"]')).toBeVisible();
  await expect(page.locator('section.board__col[data-day="5"] [data-run="r-bm"]')).toBeVisible();
  await expect(page.locator('[data-run="r-limbo"]')).toContainText('23:45');
  expect((await slots(page, 'r-fa', 'r-bm')).runs).toEqual([
    { id: 'r-fa', day: 4, time: '20:00' },
    { id: 'r-bm', day: 5, time: '23:30' },
  ]);
});

test('a swap the server refuses (it would leave the boss week) says why and puts both back', async ({ page }) => {
  await openAdmin(page);
  // The mock's week starts at midnight, so the backend's refusal is played here.
  await page.route('**/api/admin/runs/r-fa/swap', (route) =>
    route.fulfill({ status: 422, json: { error: 'invalid', message: 'that slot swap would move a run outside its boss week' } }),
  );
  await pointerDrag(page, 'r-fa', await onCard(page, 'r-bm'));
  await expect(page.getByText("Couldn't swap HFA with XBM: that slot swap would move a run outside its boss week")).toBeVisible();
  await expect(page.locator('section.board__col[data-day="4"] [data-run="r-fa"]')).toBeVisible();
  await expect(page.locator('section.board__col[data-day="5"] [data-run="r-bm"]')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Undo move' })).toBeDisabled();
});
