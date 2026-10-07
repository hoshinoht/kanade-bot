import type { Page } from '@playwright/test';
import { ADMIN, PUBLIC, api, csrf, expect, signIn, test } from './support';

type Week = { version: number; runs: { id: string; day: number; bosses: { token: string }[] }[] };

async function week(page: Page): Promise<Week> {
  const reply = await api<Week>(page, '/api/admin/week?week=next');
  expect(reply.status).toBe(200);
  return reply.body;
}

test.describe.configure({ mode: 'serial' });

test('login/logout: the live server offers only the break-glass token', async ({ page }) => {
  await signIn(page);
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Account: Break-glass token' }).click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await expect(page).toHaveURL(`${ADMIN}/login`);
  expect((await api<unknown>(page, '/api/admin/session')).status).toBe(401);
});

test('week: rejects an API create, then keyboard-moves the fixture run through history and checkpoints', async ({ page }) => {
  await signIn(page);
  const before = await week(page);
  expect(before.runs).toHaveLength(1);
  const run = before.runs[0]!;
  const invalidCreate = await api<unknown>(page, '/api/admin/fixed', {
    method: 'POST',
    headers: { ...(await csrf(page)), 'content-type': 'application/json' },
    body: JSON.stringify({ weekday: 0, time: '12:00', bosses: run.bosses[0]!.token, participants: [], channel_id: '' }),
  });
  expect(invalidCreate.status).toBe(422);

  const historyBefore = await api<{ total: number }>(page, `/api/admin/history?run=${encodeURIComponent(run.id)}`);
  expect(historyBefore.status).toBe(200);
  const totalBefore = historyBefore.body.total;

  await page.goto(`${ADMIN}/?week=next`);
  const handle = page.locator(`[data-run="${run.id}"] [data-handle="${run.id}"]`);
  await expect(handle).toBeVisible();
  await handle.focus();
  await page.keyboard.press('m');
  await page.keyboard.press(run.day < 6 ? 'ArrowRight' : 'ArrowLeft');
  await page.keyboard.press('Enter');

  await expect.poll(async () => (await week(page)).version).toBeGreaterThan(before.version);
  const moved = await week(page);
  expect(moved.runs[0]!.day).not.toBe(run.day);
  const history = await api<{ total: number }>(page, `/api/admin/history?run=${encodeURIComponent(run.id)}`);
  expect(history.status).toBe(200);
  expect(history.body.total).toBeGreaterThan(totalBefore);
  const checkpoints = await api<{ verified: { ok: boolean } }>(page, '/api/admin/history/checkpoints');
  expect(checkpoints.status).toBe(200);
  expect(checkpoints.body.verified.ok).toBe(true);

  await page.goto(`${ADMIN}/history`);
  await expect(page.locator('.history-row').first()).toBeVisible();
  await page.getByRole('tab', { name: 'Checkpoints' }).click();
  await expect(page.locator('.history-verify')).toContainText('Chain verified');
});

test('config: saves one section and rejects an unknown PATCH key with 422', async ({ page }) => {
  await signIn(page);
  const config = await api<{ notifications: { quiet_mode: boolean } }>(page, '/api/admin/config');
  expect(config.status).toBe(200);
  const quiet = config.body.notifications.quiet_mode;
  const headers = await csrf(page);
  const saved = await api<unknown>(page, '/api/admin/config', {
    method: 'PATCH',
    headers: { ...headers, 'content-type': 'application/json' },
    body: JSON.stringify({ notifications: { quiet_mode: !quiet } }),
  });
  expect(saved.status).toBe(200);
  const refused = await api<{ error?: string }>(page, '/api/admin/config', {
    method: 'PATCH',
    headers: { ...headers, 'content-type': 'application/json' },
    body: JSON.stringify({ rust_e2e_unknown: {} }),
  });
  expect(refused.status).toBe(422);
  expect(refused.body.error).toBe('unknown_field');

  await page.goto(`${ADMIN}/config?section=notifications`);
  await expect(page.getByRole('heading', { level: 1, name: 'Config' })).toBeVisible();
});

test('empty pages: Inbox, Chat and Extractions are mounted by the Rust listener', async ({ page }) => {
  await signIn(page);
  for (const [path, endpoint, heading] of [
    ['/inbox', '/api/admin/inbox', 'Nothing waiting'],
    ['/chat', '/api/admin/chat', 'Interactions'],
    ['/extractions', '/api/admin/extractions', 'Calls'],
  ] as const) {
    expect((await api<unknown>(page, endpoint)).status).toBe(200);
    await page.goto(`${ADMIN}${path}`);
    await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible();
    await expect(page.getByText("This isn't available on this server yet")).toHaveCount(0);
  }
});

test('public: closed shell and status stay public while admin routes are absent', async ({ page }) => {
  await page.goto(PUBLIC);
  await expect(page.getByRole('heading', { name: "The schedule isn't public right now" })).toBeVisible();
  const status = await api<{ portal: string }>(page, '/api/public/status');
  expect(status.status).toBe(200);
  expect(status.body).toEqual({ portal: 'closed' });
  expect((await api<unknown>(page, '/api/admin/session')).status).toBe(404);
});

test('offline: the Rust-served service worker restores the signed-in shell after reconnect', async ({ page, context }) => {
  await signIn(page);
  await page.goto(`${ADMIN}/?week=next`);
  await page.evaluate(() => navigator.serviceWorker.ready);
  await page.reload();
  await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);
  await context.setOffline(true);
  await page.reload();
  await expect(page.getByRole('heading', { name: "You're offline" })).toBeVisible();

  let release!: () => void;
  const gate = new Promise<void>((resolveGate) => (release = resolveGate));
  await page.route('**/api/admin/week?*', async (route) => {
    await gate;
    await route.continue();
  });
  await context.setOffline(false);
  try {
    await page.getByRole('button', { name: 'Try again' }).click();
  } finally {
    release();
  }
  await expect(page.locator('[data-run]').first()).toBeVisible();
});
