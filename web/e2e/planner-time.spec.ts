import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, csrf, expect, test, choose } from './support';

// Workplan step planner-time-drops (user decisions 2026-10-01): a pointer drop
// sets the time from the runs around it (Config → Run lengths), the keyboard
// steps by the configured default, and a drop that double-books somebody
// warns (icon + words) but still saves. Seed, under the pinned clock: Tue has
// HCarling + HStar 22:00 (two bosses, 60 min) and XBM 23:30; Mon has HFA
// 20:00 and HJupiter 21:00; every boss takes the 30-minute default.

const column = (page: Page, dow: string) => page.locator('section.board__col').filter({ has: page.locator(`h2 .board__dow:text-is("${dow}")`) });

async function openAdmin(page: Page) {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.getByRole('tab', { name: 'Planner' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  await expect(page.locator('.board[data-hydrated]')).toBeVisible();
}

async function weekRun(page: Page, id: string) {
  const week = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number; runs: { id: string; day: number; time: string | null; minutes: number }[] };
  return { version: week.version, run: week.runs.find((r) => r.id === id)! };
}

/** A point in a card's top edge band (above its middle half, which swaps): a drop there lands before that card. */
async function above(page: Page, runId: string) {
  const box = (await page.locator(`[data-run="${runId}"]`).boundingBox())!;
  return { x: box.x + box.width / 2, y: box.y + box.height * 0.12 };
}

async function below(page: Page, runId: string) {
  const box = (await page.locator(`[data-run="${runId}"]`).boundingBox())!;
  return { x: box.x + box.width / 2, y: box.y + box.height + 12 };
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

test('pointer drop between two runs starts right after the one above, with a live indicator', async ({ page }) => {
  await openAdmin(page);
  const to = await above(page, 'r-bm');
  await pointerDrag(page, 'r-fa', to, async () => {
    const ghost = page.locator('.dnd-ghost--on');
    // Carling + Star: 22:00 plus two 30-minute bosses.
    await expect(ghost).toContainText('→ 23:00');
    await expect(page.locator('[data-run="r-bm"]')).toHaveClass(/plan-card--drop-before/);
    // Said too, politely, for screen readers.
    await expect(page.locator('[aria-live="polite"]').filter({ hasText: 'Drop on Tue 29, 23:00.' })).toHaveCount(1);
  });
  await expect(page.getByText('Moved HFA to Tue 29 23:00.')).toBeVisible();
  expect((await weekRun(page, 'r-fa')).run).toMatchObject({ day: 5, time: '23:00' });
});

test('pointer drop at the top of a day ends right before the run below', async ({ page }) => {
  await openAdmin(page);
  await pointerDrag(page, 'r-fa', await above(page, 'r-carling'), async () => {
    await expect(page.locator('.dnd-ghost--on')).toContainText('→ 21:30');
  });
  await expect(page.getByText('Moved HFA to Tue 29 21:30.')).toBeVisible();
  expect((await weekRun(page, 'r-fa')).run.time).toBe('21:30');
});

test('a drop that double-books a member warns on the indicator and the card, and still saves', async ({ page }) => {
  // The seed puts Kaito on both HFA and XBM (Tue 23:30).
  await openAdmin(page);
  // After XBM (23:30 + 30) is past midnight: held at 23:59, inside XBM's half hour.
  await pointerDrag(page, 'r-fa', await below(page, 'r-bm'), async () => {
    const ghost = page.locator('.dnd-ghost--on');
    await expect(ghost).toContainText('→ 23:59');
    await expect(ghost).toContainText('latest the day allows');
    await expect(ghost.locator('.plan-clash')).toContainText('Clash: Kaito in XBM 23:30');
  });
  await expect(page.getByText('Moved HFA to Tue 29 23:59.')).toBeVisible();
  const card = page.locator('[data-run="r-fa"]');
  await expect(card.locator('.plan-card__clash')).toHaveText(/Clash/);
  await expect(card.getByRole('button', { name: /Clash: Kaito in XBM 23:30\./ })).toBeVisible();
  // XBM is double-booked too, from its side.
  await expect(page.locator('[data-run="r-bm"] .plan-card__clash')).toBeVisible();
  const scan = await new AxeBuilder({ page }).include('.planner').withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa']).analyze();
  expect(scan.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => v.id)).toEqual([]);
});

test('an overlap without a shared member is no clash', async ({ page, request }) => {
  // Take the players HFA shares with XBM off HFA this week.
  const { version, run: bm } = await weekRun(page, 'r-bm');
  const week = (await (await request.get(`${ADMIN}/api/admin/week`)).json()) as { runs: { id: string; participants: { id: string }[] }[] };
  const fa = week.runs.find((r) => r.id === 'r-fa')!;
  const onBm = new Set(week.runs.find((r) => r.id === bm.id)!.participants.map((p) => p.id));
  let at = version;
  for (const shared of fa.participants.filter((p) => onBm.has(p.id))) {
    const removed = await request.patch(`${ADMIN}/api/admin/runs/r-fa/participants`, { headers: await csrf(request), data: { remove: shared.id, version: at } });
    expect(removed.ok()).toBe(true);
    at = ((await removed.json()) as { version: number }).version;
  }
  await openAdmin(page);
  // HFA's players are not on XBM: held at 23:59 inside XBM, and nothing to warn about.
  await pointerDrag(page, 'r-fa', await below(page, 'r-bm'), async () => {
    await expect(page.locator('.dnd-ghost--on')).toContainText('→ 23:59');
    await expect(page.locator('.dnd-ghost--on .plan-clash')).toHaveCount(0);
  });
  await expect(page.getByText('Moved HFA to Tue 29 23:59.')).toBeVisible();
  await expect(page.locator('.plan-card__clash')).toHaveCount(0);
});

test('a change by someone else during the drag is not overwritten: the drop conflicts', async ({ page, request }) => {
  await openAdmin(page);
  const { version: start } = await weekRun(page, 'r-fa');
  const sent: { version: number }[] = [];
  page.on('request', (r) => {
    if (r.method() === 'POST' && r.url().endsWith('/api/admin/runs/r-fa/move')) sent.push(r.postDataJSON() as { version: number });
  });
  await pointerDrag(page, 'r-fa', await above(page, 'r-bm'), async () => {
    // Another admin moves HLimbo while the card is held; the page polls and buffers it.
    const moved = await request.post(`${ADMIN}/api/admin/runs/r-limbo/move`, { headers: await csrf(request), data: { day: 1, time: '23:45', version: start } });
    expect(moved.ok()).toBe(true);
    const polled = page.waitForResponse((r) => r.url().includes('/api/admin/week') && r.request().method() === 'GET');
    await page.evaluate(() => window.dispatchEvent(new Event('online')));
    await polled;
    await expect(page.locator('.dnd-ghost--on')).toContainText('→ 23:00');
  });
  // Sent against the week the drag began on, so the server refuses it.
  await expect(page.getByText(/^Couldn't move HFA/)).toBeVisible();
  expect(sent).toEqual([expect.objectContaining({ version: start })]);
  expect((await weekRun(page, 'r-fa')).run).toMatchObject({ day: 4, time: '20:00' });
  // The other admin's change is on the board afterwards.
  await expect(page.locator('[data-run="r-limbo"]')).toContainText('23:45');
});

test('the keyboard drop conflicts the same way after a change made during the lift', async ({ page, request }) => {
  await openAdmin(page);
  const { version: start } = await weekRun(page, 'r-fa');
  await page.locator('[data-handle="r-fa"]').focus();
  await page.keyboard.press('m');
  await page.keyboard.press('ArrowRight');
  const moved = await request.post(`${ADMIN}/api/admin/runs/r-limbo/move`, { headers: await csrf(request), data: { day: 1, time: '23:45', version: start } });
  expect(moved.ok()).toBe(true);
  const polled = page.waitForResponse((r) => r.url().includes('/api/admin/week') && r.request().method() === 'GET');
  await page.evaluate(() => window.dispatchEvent(new Event('online')));
  await polled;
  const sent = page.waitForRequest((r) => r.method() === 'POST' && r.url().endsWith('/api/admin/runs/r-fa/move'));
  await page.keyboard.press('Enter');
  expect((await sent).postDataJSON()).toMatchObject({ version: start, day: 5 });
  await expect(page.getByText(/^Couldn't move HFA/)).toBeVisible();
});

test('own-time runs keep no time when dropped: only the day changes', async ({ page }) => {
  await openAdmin(page);
  await pointerDrag(page, 'r-bellona', await above(page, 'r-bm'), async () => {
    await expect(page.locator('.dnd-ghost--on')).toContainText('own time');
    await expect(page.locator('.dnd-ghost--on')).not.toContainText('→');
  });
  await expect(page.getByText(/Moved NBellona to Tue 29/)).toBeVisible();
  expect((await weekRun(page, 'r-bellona')).run).toMatchObject({ day: 5, time: null });
});

test.describe('touch', () => {
  test.use({ hasTouch: true, viewport: { width: 1280, height: 800 } });

  test('a long press drags, and the drop sets the time the same way', async ({ page }) => {
    await openAdmin(page);
    const cdp = await page.context().newCDPSession(page);
    const touch = (type: string, x?: number, y?: number) =>
      cdp.send('Input.dispatchTouchEvent', { type, touchPoints: x === undefined ? [] : [{ x, y: y! }] });
    const from = (await page.locator('[data-run="r-fa"]').boundingBox())!;
    const to = await above(page, 'r-bm');
    const [x0, y0] = [from.x + from.width / 2, from.y + from.height / 2];
    await touch('touchStart', x0, y0);
    await page.waitForTimeout(400);
    for (let i = 1; i <= 12; i++) await touch('touchMove', x0 + ((to.x - x0) * i) / 12, y0 + ((to.y - y0) * i) / 12);
    await touch('touchMove', to.x + 1, to.y + 1);
    await expect(page.locator('.dnd-ghost--on')).toContainText('→ 23:00');
    await touch('touchEnd');
    await expect(page.getByText('Moved HFA to Tue 29 23:00.')).toBeVisible();
    await expect(page.getByRole('dialog')).toBeHidden();
  });
});

test('keyboard M-move steps by the Run lengths default, and Shift jumps next to a run', async ({ page }) => {
  // Set the default to 20 in Config, then plan by keyboard on the Week.
  await page.goto(`${ADMIN}/config?section=run-lengths&sw=off`);
  const panel = page.getByRole('tabpanel', { name: 'Run lengths' });
  await panel.getByRole('spinbutton', { name: 'Each boss' }).fill('20');
  await panel.getByRole('button', { name: 'Save run lengths' }).click();
  await expect(page.getByText(/Run lengths saved/)).toBeVisible();
  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Week' }).click();
  await expect(page.locator('[data-run="r-fa"]')).toBeVisible();

  const handle = page.locator('[data-handle="r-fa"]');
  await handle.focus();
  await page.keyboard.press('m');
  await expect(page.locator('[aria-live="assertive"]')).toContainText('up and down change the time by 20 minutes');
  await page.keyboard.press('ArrowDown');
  await expect(column(page, 'Mon').locator('.plan-preview')).toContainText('20:20 HFA');
  // HJupiter starts 21:00; HFA (one boss) now lasts 20 minutes: just before it is 20:40.
  await page.keyboard.press('Shift+ArrowDown');
  await expect(column(page, 'Mon').locator('.plan-preview')).toContainText('20:40 HFA');
  await page.keyboard.press('Enter');
  await expect(page.getByText('Moved HFA to Mon 28 20:40.')).toBeVisible();
  expect((await weekRun(page, 'r-fa')).run).toMatchObject({ time: '20:40', minutes: 20 });
});

test('Config Run lengths: overrides from the catalog save whole; bad values are refused inline', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=run-lengths&sw=off`);
  const panel = page.getByRole('tabpanel', { name: 'Run lengths' });
  await expect(panel.getByRole('heading', { name: 'Run lengths' })).toBeVisible();
  await expect(panel.getByRole('spinbutton', { name: 'Each boss' })).toHaveValue('30');
  // The seeded Hard Black Mage override.
  await expect(panel.getByRole('combobox', { name: 'Boss', exact: true })).toHaveCount(1);

  // Client check: out of range, nothing sent.
  await panel.getByRole('button', { name: 'Add an override' }).click();
  const boss = panel.getByRole('combobox', { name: 'Boss', exact: true }).last();
  await expect(boss).toBeFocused();
  await choose(boss, { label: 'Carling' });
  await panel.getByRole('spinbutton', { name: 'Minutes' }).last().fill('600');
  await panel.getByRole('button', { name: 'Save run lengths' }).click();
  await expect(panel.getByRole('alert')).toHaveText('Override 2: a run length is 5–480 whole minutes.');

  // A refusal from the server stays inline, the edits kept.
  await panel.getByRole('spinbutton', { name: 'Minutes' }).last().fill('45');
  await page.route('**/api/admin/config', async (route) => {
    if (route.request().method() !== 'PATCH') return route.continue();
    await route.fulfill({ status: 422, json: { error: 'invalid', message: 'Pick a catalog boss key and difficulty.' } });
  });
  await panel.getByRole('button', { name: 'Save run lengths' }).click();
  await expect(panel.getByRole('alert')).toContainText('Pick a catalog boss key and difficulty.');
  await expect(panel.getByRole('spinbutton', { name: 'Minutes' }).last()).toHaveValue('45');
  await page.unroute('**/api/admin/config');

  // Saved for real: the PATCH carries the whole section.
  const sent = page.waitForRequest((r) => r.method() === 'PATCH' && r.url().endsWith('/api/admin/config'));
  await panel.getByRole('button', { name: 'Save run lengths' }).click();
  expect((await sent).postDataJSON()).toEqual({
    run_lengths: {
      default_minutes: 30,
      overrides: [
        { boss: 'BM', difficulty: 'h', minutes: 60 },
        { boss: 'Carling', difficulty: expect.any(String), minutes: 45 },
      ],
    },
  });
  await expect(page.getByText(/Run lengths saved/)).toBeVisible();
  await expect(panel.getByRole('alert')).toHaveCount(0);

  // Remove returns focus to "Add an override".
  await panel.getByRole('button', { name: /^Remove the Carling override/ }).click();
  await expect(panel.getByRole('button', { name: 'Add an override' })).toBeFocused();
  await expect(panel.getByRole('combobox', { name: 'Boss', exact: true })).toHaveCount(1);
});
