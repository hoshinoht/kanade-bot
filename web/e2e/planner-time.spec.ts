import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, csrf, expect, test, choose } from './support';

// Workplan step planner-time-drops (user decisions 2026-10-01): a pointer drop
// sets the time from the runs around it (Config → Run lengths), the keyboard
// steps by 30 minutes, and a drop that double-books somebody
// warns (icon + words) but still saves. Seed: Tue has HCarling + HMaleficStar
// 22:00 (two bosses, 40 min) and XBM 23:30; Mon has HFA 20:00 and HJupiter
// 21:00; every boss takes the 20-minute default. The server clock stands at
// Mon 28 12:00 (guild time), before Monday's runs: at the pinned Tuesday they
// have ended and the server refuses to move them.

const MONDAY = '2026-09-28T04:00:00Z';

test.beforeEach(async ({ request }) => {
  expect((await request.post(`${ADMIN}/__e2e/clock`, { data: { at: MONDAY } })).ok()).toBe(true);
});

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
    // Carling + Star: 22:00 plus two 20-minute bosses.
    await expect(ghost).toContainText('→ 22:40');
    await expect(page.locator('[data-run="r-bm"]')).toHaveClass(/plan-card--drop-before/);
    // Said too, politely, for screen readers.
    await expect(page.locator('[aria-live="polite"]').filter({ hasText: 'Drop on Tue 29, 22:40.' })).toHaveCount(1);
  });
  await expect(page.getByText('Moved HFA to Tue 29 22:40.')).toBeVisible();
  expect((await weekRun(page, 'r-fa')).run).toMatchObject({ day: 5, time: '22:40' });
});

test('pointer drop at the top of a day ends right before the run below', async ({ page }) => {
  await openAdmin(page);
  await pointerDrag(page, 'r-fa', await above(page, 'r-carling'), async () => {
    await expect(page.locator('.dnd-ghost--on')).toContainText('→ 21:40');
  });
  await expect(page.getByText('Moved HFA to Tue 29 21:40.')).toBeVisible();
  expect((await weekRun(page, 'r-fa')).run.time).toBe('21:40');
});

async function extendExtremeBlackMage(request: Page['request']) {
  const result = await request.patch(`${ADMIN}/api/admin/config`, {
    headers: await csrf(request),
    data: { run_lengths: { overrides: [{ boss: 'BM', difficulty: 'x', minutes: 30 }] } },
  });
  expect(result.ok()).toBe(true);
}

test('a drop that double-books a member warns on the indicator and the card, and still saves', async ({ page, request }) => {
  // The seed puts Kaito on both HFA and XBM (Tue 23:30).
  await extendExtremeBlackMage(request);
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
  await extendExtremeBlackMage(request);
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
    // Another admin moves HFA itself while the card is held (the server checks the fields a move
    // touches, so a change to another run would not conflict); the page polls and buffers it.
    const moved = await request.post(`${ADMIN}/api/admin/runs/r-fa/move`, { headers: await csrf(request), data: { day: 4, time: '20:30', version: start } });
    expect(moved.ok()).toBe(true);
    const polled = page.waitForResponse((r) => r.url().includes('/api/admin/week') && r.request().method() === 'GET');
    await page.evaluate(() => window.dispatchEvent(new Event('online')));
    await polled;
    await expect(page.locator('.dnd-ghost--on')).toContainText('→ 22:40');
  });
  // Sent against the week the drag began on, so the server refuses it.
  await expect(page.getByText(/^Couldn't move HFA/)).toBeVisible();
  expect(sent).toEqual([expect.objectContaining({ version: start })]);
  expect((await weekRun(page, 'r-fa')).run).toMatchObject({ day: 4, time: '20:30' });
  // The other admin's change is on the board afterwards.
  await expect(page.locator('[data-run="r-fa"]')).toContainText('20:30');
});

test('the keyboard drop conflicts the same way after a change made during the lift', async ({ page, request }) => {
  await openAdmin(page);
  const { version: start } = await weekRun(page, 'r-fa');
  await page.locator('[data-handle="r-fa"]').focus();
  await page.keyboard.press('m');
  await page.keyboard.press('ArrowRight');
  const moved = await request.post(`${ADMIN}/api/admin/runs/r-fa/move`, { headers: await csrf(request), data: { day: 4, time: '20:30', version: start } });
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
    const touch = (type: 'touchStart' | 'touchMove' | 'touchEnd', x?: number, y?: number) =>
      cdp.send('Input.dispatchTouchEvent', { type, touchPoints: x === undefined ? [] : [{ x, y: y! }] });
    const from = (await page.locator('[data-run="r-fa"]').boundingBox())!;
    const to = await above(page, 'r-bm');
    const [x0, y0] = [from.x + from.width / 2, from.y + from.height / 2];
    await touch('touchStart', x0, y0);
    // Held until the long press picks the card up (not a fixed sleep: a busy runner fires the 250 ms timer late).
    await expect(page.locator('.dnd-ghost--on')).toHaveCount(1);
    for (let i = 1; i <= 12; i++) await touch('touchMove', x0 + ((to.x - x0) * i) / 12, y0 + ((to.y - y0) * i) / 12);
    await touch('touchMove', to.x + 1, to.y + 1);
    await expect(page.locator('.dnd-ghost--on')).toContainText('→ 22:40');
    await touch('touchEnd');
    await expect(page.getByText('Moved HFA to Tue 29 22:40.')).toBeVisible();
    await expect(page.getByRole('dialog')).toBeHidden();
  });
});

test('keyboard M-move keeps the 30-minute step after a duration edit, and Shift jumps next to a run', async ({ page }) => {
  // Change the run duration, then plan by keyboard on the Week.
  await page.goto(`${ADMIN}/config?section=run-lengths&sw=off`);
  const panel = page.getByRole('tabpanel', { name: 'Run lengths' });
  await panel.getByRole('spinbutton', { name: 'Each boss' }).fill('25');
  await panel.getByRole('button', { name: 'Save run lengths' }).click();
  await expect(page.getByText(/Run lengths saved/)).toBeVisible();
  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Week' }).click();
  await expect(page.locator('[data-run="r-fa"]')).toBeVisible();

  const handle = page.locator('[data-handle="r-fa"]');
  await handle.focus();
  await page.keyboard.press('m');
  await expect(page.locator('[aria-live="assertive"]')).toContainText('up and down change the time by 30 minutes');
  await page.keyboard.press('ArrowDown');
  await expect(column(page, 'Mon').locator('.plan-preview')).toContainText('20:30 HFA');
  // HJupiter starts 21:00. A 25-minute HFA fits before it at 20:35.
  await page.keyboard.press('Shift+ArrowDown');
  await expect(column(page, 'Mon').locator('.plan-preview')).toContainText('20:35 HFA');
  await page.keyboard.press('Enter');
  await expect(page.getByText('Moved HFA to Mon 28 20:35.')).toBeVisible();
  expect((await weekRun(page, 'r-fa')).run).toMatchObject({ time: '20:35', minutes: 25 });
});

test('Config Run lengths: overrides from the catalog save whole; bad values are refused inline', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=run-lengths&sw=off`);
  const panel = page.getByRole('tabpanel', { name: 'Run lengths' });
  await expect(panel.getByRole('heading', { name: 'Run lengths' })).toBeVisible();
  await expect(panel.getByRole('spinbutton', { name: 'Each boss' })).toHaveValue('20');
  await expect(panel.getByRole('combobox', { name: 'Boss', exact: true })).toHaveCount(0);

  // Client check: out of range, nothing sent.
  await panel.getByRole('button', { name: 'Add an override' }).click();
  const boss = panel.getByRole('combobox', { name: 'Boss', exact: true }).last();
  await expect(boss).toBeFocused();
  await choose(boss, { label: 'Karling' });
  await panel.getByRole('spinbutton', { name: 'Minutes' }).last().fill('600');
  await panel.getByRole('button', { name: 'Save run lengths' }).click();
  await expect(panel.getByRole('alert')).toHaveText('Override 1: a run length is 5–480 whole minutes.');

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
      default_minutes: 20,
      overrides: [
        { boss: 'Carling', difficulty: expect.any(String), minutes: 45 },
      ],
    },
  });
  await expect(page.getByText(/Run lengths saved/)).toBeVisible();
  await expect(panel.getByRole('alert')).toHaveCount(0);

  // Remove returns focus to "Add an override".
  await panel.getByRole('button', { name: /^Remove the Karling override/ }).click();
  await expect(panel.getByRole('button', { name: 'Add an override' })).toBeFocused();
  await expect(panel.getByRole('combobox', { name: 'Boss', exact: true })).toHaveCount(0);
});
