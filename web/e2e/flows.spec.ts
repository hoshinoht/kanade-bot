import type { Page } from '@playwright/test';
import { ADMIN, HEADING, PUBLIC, csrf, expect, test } from './support';

// Every test here also asserts, via the auto `csp` fixture, zero enforced or
// report-only (Trusted Types) violations: console, DOM events and server reports.

const column = (page: Page, dow: string) => page.locator('section.board__col').filter({ has: page.locator(`h2 .board__dow:text-is("${dow}")`) });

async function openAdmin(page: Page) {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.getByRole('tab', { name: 'Planner' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
}

test('public: board, list and theme switching', async ({ page }) => {
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 runs');
  await expect(column(page, 'Tue').locator('.runcard')).toHaveCount(2);

  await page.getByRole('tab', { name: /List/ }).click();
  await expect(page.getByRole('table')).toBeVisible();
  await expect(page.getByRole('rowheader').first()).toContainText('Baldrix');

  await page.getByRole('tab', { name: 'Appearance' }).click();
  await page.getByText('Blossom', { exact: true }).click();
  await page.getByText('Dark', { exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'blossom');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  // Stored and applied before first paint on reload by the external theme-boot script.
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'blossom');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

test('public: arrow keys move between tabs and only the panel scrolls', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 700 });
  await page.goto(`${PUBLIC}/?sw=off`);
  await page.getByRole('tab', { name: 'Week' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: /List/ })).toBeFocused();
  await expect(page.getByRole('tab', { name: /List/ })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('ArrowLeft');
  const metrics = await page.evaluate(() => {
    const panel = document.querySelector<HTMLElement>('.tabs__panel:not([hidden])')!;
    return {
      doc: document.scrollingElement!.scrollHeight - innerHeight,
      panel: panel.scrollHeight - panel.clientHeight,
      overflow: getComputedStyle(panel).overflowY,
    };
  });
  expect(metrics.doc).toBeLessThanOrEqual(0);
  expect(metrics.panel).toBeGreaterThan(0);
  expect(metrics.overflow).toBe('auto');
});

test('admin: modal opens from a card and restores focus on Escape', async ({ page }) => {
  await openAdmin(page);
  const open = page.locator('[data-run="r-carling"] .plan-card__open');
  await open.focus();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText('Radiant Malefic Star')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(open).toBeFocused();
});

test('admin: run sheet Move field moves a run and keeps input after a rejected time', async ({ page }) => {
  await openAdmin(page);
  await page.locator('[data-run="r-limbo"] .plan-card__open').click();
  const dialog = page.getByRole('dialog', { name: 'HLimbo' });
  const field = dialog.getByRole('textbox', { name: /Move HLimbo/ });
  await field.fill('25:99');
  await dialog.getByRole('button', { name: 'Move', exact: true }).click();
  await expect(dialog.getByRole('alert')).toHaveText(/Times run from|Give minutes|Write a day/);
  await expect(field).toHaveValue('25:99');
  await field.fill('sat 20:30');
  await dialog.getByRole('button', { name: 'Move', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(column(page, 'Sat').locator('[data-run="r-limbo"]')).toContainText('20:30');
});

test('admin: command palette searches and runs commands', async ({ page }) => {
  await openAdmin(page);
  await page.keyboard.press('ControlOrMeta+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await expect(palette).toBeVisible();
  const search = palette.getByRole('combobox', { name: 'Search commands' });
  await expect(search).toBeFocused();
  await search.fill('twilight');
  await expect(palette.getByRole('option')).toHaveCount(1);
  await page.keyboard.press('Enter');
  await expect(palette).toBeHidden();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'twilight');

  await page.keyboard.press('ControlOrMeta+k');
  await search.fill('open kalos');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('dialog', { name: 'XKalos' })).toBeVisible();
  await page.keyboard.press('Escape');

  await page.keyboard.press('ControlOrMeta+k');
  await search.fill('show answers');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('tab', { name: 'Answers' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.chart__canvas canvas')).toBeVisible();
  await expect(page.getByRole('table', { name: 'Answers by day' })).toContainText('Answered');
});

test('admin: keyboard-only move with announcements, then undo from the toast', async ({ page }) => {
  await openAdmin(page);
  const live = page.locator('[role="alert"][aria-live="assertive"]');
  const card = page.locator('[data-handle="r-carling"]');
  await expect(card).toHaveAttribute('aria-keyshortcuts', 'M');
  await card.focus();
  await page.keyboard.press('m');
  await expect(live).toContainText('Picked up HCarling + HStar, Tue 29, 22:00');
  await expect(page.locator('[data-run="r-carling"]')).toHaveClass(/plan-card--lifted/);
  await page.keyboard.press('ArrowLeft');
  await expect(live).toContainText('HCarling + HStar: Mon 28, 22:00.');
  await page.keyboard.press('ArrowDown');
  await expect(live).toContainText('Mon 28, 22:30.');
  await page.keyboard.press('Enter');
  await expect(live).toContainText('Dropped HCarling + HStar on Mon 28, 22:30.');
  // The drop key does not also open the sheet.
  await expect(page.getByRole('dialog')).toBeHidden();

  await expect(column(page, 'Mon').locator('[data-run="r-carling"]')).toContainText('22:30');
  await expect(page.locator('[data-handle="r-carling"]')).toBeFocused();
  const toast = page.getByRole('group', { name: 'Notification' }).filter({ hasText: 'Moved HCarling + HStar' });
  await expect(toast).toBeVisible();

  await toast.getByRole('button', { name: 'Undo' }).click();
  await expect(column(page, 'Tue').locator('[data-run="r-carling"]')).toContainText('22:00');
  await expect(page.getByText('Move undone: HCarling + HStar')).toBeVisible();
  // The toast (and its focused button) is gone; focus lands on the moved card's handle.
  await expect(page.locator('[data-handle="r-carling"]')).toBeFocused();

  // Escape cancels without a request.
  await page.locator('[data-handle="r-bm"]').focus();
  await page.keyboard.press('m');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Escape');
  await expect(live).toContainText('Move cancelled. XBM stays on Tue 29, 23:30.');
  await expect(column(page, 'Tue').locator('[data-run="r-bm"]')).toBeVisible();
});

// The idle callback exists but never fires: any hydration then has to come
// from the pointer, since the 1.5 s timer is only the no-rIC fallback.
async function neverIdle(page: Page) {
  await page.addInitScript(() => {
    window.requestIdleCallback = (() => 0) as never;
  });
}

async function dragTo(page: Page, from: { x: number; y: number; width: number; height: number }, to: { x: number; y: number; width: number }) {
  for (let i = 1; i <= 12; i++) {
    await page.mouse.move(
      from.x + from.width / 2 + ((to.x + to.width / 2 - from.x - from.width / 2) * i) / 12,
      from.y + from.height / 2 + ((to.y + 120 - from.y - from.height / 2) * i) / 12,
    );
  }
  await page.mouse.move(to.x + to.width / 2 + 1, to.y + 121);
}

test('admin: pointer drag of a whole card to another day, Ctrl/Cmd+Z undoes', async ({ page }) => {
  await neverIdle(page);
  await openAdmin(page);
  // Well past the 2.5 s idle timeout and the 1.5 s fallback, still cold.
  await page.waitForTimeout(3000);
  await expect(page.locator('.board:not([data-hydrated])')).toBeVisible();
  const card = page.locator('[data-run="r-fa"]');
  const target = column(page, 'Wed');
  const from = (await card.boundingBox())!;
  const to = (await target.boundingBox())!;
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await expect(page.locator('.board[data-hydrated]')).toBeVisible();
  await page.mouse.down();
  await dragTo(page, from, to);
  await expect(page.locator('.dnd-ghost--on')).toContainText('HFA');
  // Moves are applied on the next frame: drop only once the column says so.
  await expect(target).toHaveClass(/board__col--target/);
  await page.mouse.up();
  await expect(target.locator('[data-run="r-fa"]')).toBeVisible();
  await expect(page.getByText('Moved HFA to Wed 30 20:00.')).toBeVisible();
  // The drop does not also open the sheet.
  await expect(page.getByRole('dialog')).toBeHidden();

  await page.locator('main').click({ position: { x: 5, y: 5 } });
  await page.keyboard.press('ControlOrMeta+z');
  await expect(column(page, 'Mon').locator('[data-run="r-fa"]')).toBeVisible();
});

test('admin: the very first press drags even before the engine hydrates', async ({ page }) => {
  await neverIdle(page);
  // Hold the engine chunk until the press is already down, as on a cold phone.
  let release: () => void = () => {};
  const gate = new Promise<void>((r) => (release = r));
  await page.route('**/assets/pointerDrag-*', async (route) => {
    await gate;
    await route.continue();
  });
  await openAdmin(page);
  const card = page.locator('[data-run="r-fa"]');
  const target = column(page, 'Wed');
  const from = (await card.boundingBox())!;
  const to = (await target.boundingBox())!;
  // Pointer-over is dispatched on the board before the press, like touch.
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await expect(page.locator('.board:not([data-hydrated])')).toBeVisible();
  release();
  await expect(page.locator('.board[data-hydrated]')).toBeVisible();
  await dragTo(page, from, to);
  await expect(page.locator('.dnd-ghost--on')).toContainText('HFA');
  // Moves are applied on the next frame: drop only once the column says so.
  await expect(target).toHaveClass(/board__col--target/);
  await page.mouse.up();
  await expect(target.locator('[data-run="r-fa"]')).toBeVisible();
});

test('admin: a click on a card opens it; it does not start a drag', async ({ page }) => {
  await openAdmin(page);
  await expect(page.locator('.board[data-hydrated]')).toBeVisible();
  await page.locator('[data-run="r-limbo"] .plan-card__open').click();
  await expect(page.getByRole('dialog', { name: 'HLimbo' })).toBeVisible();
  await expect(page.locator('.dnd-ghost--on')).toHaveCount(0);
});

test.describe('touch', () => {
  test.use({ hasTouch: true, viewport: { width: 1280, height: 800 } });

  test('admin: a tap opens the card; a long press drags it', async ({ page }) => {
    await openAdmin(page);
    await expect(page.locator('.board[data-hydrated]')).toBeVisible();
    await page.locator('[data-run="r-limbo"] .plan-card__open').tap();
    await expect(page.getByRole('dialog', { name: 'HLimbo' })).toBeVisible();
    await page.keyboard.press('Escape');

    const cdp = await page.context().newCDPSession(page);
    const touch = (type: string, x?: number, y?: number) =>
      cdp.send('Input.dispatchTouchEvent', { type, touchPoints: x === undefined ? [] : [{ x, y: y! }] });
    const from = (await page.locator('[data-run="r-fa"]').boundingBox())!;
    const target = column(page, 'Wed');
    const to = (await target.boundingBox())!;
    const [x0, y0] = [from.x + from.width / 2, from.y + from.height / 2];
    await touch('touchStart', x0, y0);
    await page.waitForTimeout(400);
    for (let i = 1; i <= 12; i++)
      await touch('touchMove', x0 + ((to.x + to.width / 2 - x0) * i) / 12, y0 + ((to.y + 120 - y0) * i) / 12);
    await touch('touchMove', to.x + to.width / 2 + 1, to.y + 121);
    await expect(target).toHaveClass(/board__col--target/);
    await touch('touchEnd');
    await expect(target.locator('[data-run="r-fa"]')).toBeVisible();
    await expect(page.getByRole('dialog')).toBeHidden();
  });
});

test('admin: a rejected move rolls back and says why', async ({ page }) => {
  await openAdmin(page);
  // Make the page's week version stale by moving through the API behind its back.
  const { version } = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number };
  await page.request.post(`${ADMIN}/api/admin/runs/r-bm/move`, { headers: await csrf(page.request), data: { day: 6, time: '23:30', version } });
  const handle = page.locator('[data-handle="r-limbo"]');
  await handle.focus();
  await page.keyboard.press('m');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  await expect(page.getByText(/Couldn't move HLimbo: The week changed since it was loaded\./)).toBeVisible();
  await expect(column(page, 'Fri').locator('[data-run="r-limbo"]')).toBeVisible();
});

test('admin: a lazy chunk that fails after a deploy offers a reload', async ({ page }) => {
  await page.route('**/assets/AnswersChart-*', (route) => route.abort());
  await openAdmin(page);
  await page.getByRole('tab', { name: 'Answers' }).click();
  const alert = page.getByRole('alert').filter({ hasText: "The chart didn't load" });
  await expect(alert).toBeVisible();
  await expect(alert.getByRole('button', { name: 'Reload' })).toBeVisible();
});

test('admin: duplicate display names each render, keyed by member id', async ({ page }) => {
  await openAdmin(page);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const dialog = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await expect(dialog.locator('.run__people .chip', { hasText: 'Ren' })).toHaveCount(2);
  await page.keyboard.press('Escape');
  await page.getByRole('tab', { name: /Runs/ }).click();
  const row = page.getByRole('row').filter({ has: page.getByRole('rowheader', { name: /Carling/ }) });
  await expect(row.locator('.chip', { hasText: 'Ren' })).toHaveCount(2);
});

test('public: a closed portal keeps polling on the normal cadence and shows the reopening', async ({ page, request }) => {
  await page.clock.install();
  await request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(request), data: { self_service: { public_portal: false } } });
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { name: "The schedule isn't public right now" })).toBeVisible();
  // More than the poller's six-failure limit, each one on the plain 30 s
  // interval (a failure would back off to 60 s and this wait would hang).
  for (let i = 0; i < 8; i++) {
    const polled = page.waitForResponse((r) => r.url().includes('/api/public/week'), { timeout: 5000 });
    await page.clock.runFor(30_000);
    expect((await polled).status()).toBe(503);
  }
  await request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(request), data: { self_service: { public_portal: true } } });
  await page.clock.runFor(30_000);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 runs');
});

test('public: the public week carries no people, party or version', async ({ page }) => {
  const response = await page.request.get(`${PUBLIC}/api/public/week`);
  const body = (await response.json()) as { runs: Record<string, unknown>[] } & Record<string, unknown>;
  expect(Object.keys(body).sort()).toEqual(['days', 'generated_at', 'reset', 'runs', 'starts', 'timezone']);
  for (const run of body.runs) expect(Object.keys(run).sort()).toEqual(['bosses', 'day', 'id', 'status', 'tally', 'time']);
  await page.goto(`${PUBLIC}/?sw=off`);
  await page.getByRole('tab', { name: /List/ }).click();
  await expect(page.getByRole('columnheader', { name: 'Party' })).toHaveCount(0);
});

test('public: a closed portal says so instead of showing a stale week', async ({ page, request }) => {
  await request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(request), data: { self_service: { public_portal: false } } });
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { name: "The schedule isn't public right now" })).toBeVisible();
  // The masthead says closed, neutrally: not green Live, not an error.
  const fresh = page.locator('.fresh');
  await expect(fresh).toHaveAttribute('data-fresh', 'closed');
  await expect(fresh).toHaveText(/Closed\s·\schecked\s\d\d:\d\d/);
  // Closed means shell, status and identity only: data and art answer closed.
  expect(await (await request.get(`${PUBLIC}/api/public/status`)).json()).toEqual({ portal: 'closed' });
  expect((await request.get(`${PUBLIC}/api/identity`)).status()).toBe(200);
  expect((await request.get(`${PUBLIC}/identity/avatar`)).status()).toBe(200);
  for (const path of ['/api/public/week', '/art/entry/Carling', '/art/portraits/Carling']) {
    const closed = await request.get(`${PUBLIC}${path}`);
    expect(closed.status(), path).toBe(503);
    expect(((await closed.json()) as { error: string }).error).toBe('closed');
  }
  // The admin origin is unaffected.
  expect((await request.get(`${ADMIN}/art/portraits/Carling`)).status()).toBe(200);

  await request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(request), data: { self_service: { public_portal: true } } });
  await page.getByRole('button', { name: 'Check again' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 runs');
});

for (const [name, origin] of [
  ['admin', ADMIN],
  ['public', PUBLIC],
] as const) {
  test(`${name}: the phone rail shows the week's shape and jumps inside the panel`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`${origin}/?sw=off`);
    const rail = page.getByRole('navigation', { name: 'Days of this boss week' });
    await expect(rail.getByRole('button')).toHaveCount(7);
    await expect(rail.getByRole('button', { name: /^Thu 24, .*the boss week starts$/ })).toBeVisible();
    await expect(rail.getByRole('button', { name: /^Tue 29, / })).toHaveAttribute('aria-current', 'date');
    const wed = page.locator('section.board__col[data-day="6"]');
    await rail.getByRole('button', { name: /^Wed 30, / }).click();
    await expect(wed.locator('.board__head')).toBeFocused();
    await expect(wed).toBeInViewport();
    await expect(rail).toBeInViewport();
    // The document itself can never scroll, even when asked to.
    expect(await page.evaluate(() => (window.scrollTo(0, 500), document.scrollingElement!.scrollTop))).toBe(0);
  });

  test(`${name}: wide screens hide the rail; the board is the rail grown up`, async ({ page }) => {
    await page.goto(`${origin}/?sw=off`);
    await expect(page.locator('section.board__col').first()).toBeVisible();
    await expect(page.getByRole('navigation', { name: 'Days of this boss week' })).toBeHidden();
  });
}

test('admin week header: view switch, move help, and compact filters as chips', async ({ page }) => {
  await openAdmin(page);
  const views = page.getByRole('tablist', { name: 'Week views' });
  await views.getByRole('tab', { name: 'Planner' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(views.getByRole('tab', { name: /Runs/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('tabpanel')).toHaveAttribute('aria-labelledby', /tab-runs$/);
  await views.getByRole('tab', { name: 'Planner' }).click();

  // The move help is a disclosure; every movable card is described by it either way.
  const help = page.getByRole('button', { name: 'How to move runs' });
  await expect(help).toHaveAttribute('aria-expanded', 'false');
  const helpId = (await help.getAttribute('aria-controls'))!;
  await expect(page.locator(`[data-handle="r-carling"]`)).toHaveAttribute('aria-describedby', helpId);
  await expect(page.locator(`#${helpId}`)).toBeHidden();
  await help.click();
  await expect(page.locator(`#${helpId}`)).toContainText('press M');
  // Open, it is a contained card, never bare text on the ground.
  await expect(page.locator(`#${helpId}`)).not.toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
  await expect(page.locator(`#${helpId}`)).toHaveCSS('border-top-style', 'solid');
  // The help belongs to the Planner: it leaves with its toggle and returns with it.
  await views.getByRole('tab', { name: /Runs/ }).click();
  await expect(page.locator(`#${helpId}`)).toBeHidden();
  await views.getByRole('tab', { name: 'Planner' }).click();
  await expect(page.locator(`#${helpId}`)).toBeVisible();
  await help.click();

  // Phones: the filter card folds into "Filters (n)" with removable chips.
  await page.setViewportSize({ width: 390, height: 844 });
  const toggle = page.getByRole('button', { name: 'Filters (0)' });
  await expect(toggle).toBeVisible();
  await expect(page.getByRole('search', { name: 'Filter the week' })).toHaveCount(0);
  await toggle.click();
  await page.getByRole('search', { name: 'Filter the week' }).getByLabel('Boss').fill('bm');
  await expect(page.getByRole('button', { name: 'Filters (1)' })).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 run, filtered');
  await page.getByRole('button', { name: 'Filters (1)' }).click();
  await page.getByRole('button', { name: /Boss: bm/ }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);
});
