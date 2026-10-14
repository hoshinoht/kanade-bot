import type { Page } from '@playwright/test';
import { ADMIN, PUBLIC, csrf, expect, settle, signInPublic, test } from './support';

// The member's Move of their own run (member-writes contract Phase A,
// boards Week-RunMine, Move, Move-NotIn, Move-WeekOver, PhoneMove*) against
// the mock's member (Asahi): HCarling + HStar (Tue 29 22:00) is the one run
// of hers this boss week that has not started (the clock is Tue 29 12:00,
// the week ends Wed 30 23:59). Every test ends with zero CSP/TT reports.

test.describe.configure({ mode: 'parallel' });

/** A Discord link's slot: Wed 30 Sep 21:30 in the guild's time (GMT+8). */
const MOVE_TO = '2026-09-30T13:30:00Z';
const toast = (page: Page) => page.getByRole('group', { name: 'Notification' });
const pane = (page: Page) => page.getByRole('complementary', { name: /^Your run · .*Carling/ });
const form = (page: Page) => page.getByRole('region', { name: 'Move HCarling + HStar' });

/** The run's slot as the server holds it now. */
async function slot(page: Page, run = 'r-carling'): Promise<{ day: number; time: string | null }> {
  const week = (await (await page.request.get(`${PUBLIC}/api/public/week`)).json()) as { runs: { id: string; day: number; time: string | null }[] };
  const found = week.runs.find((r) => r.id === run)!;
  return { day: found.day, time: found.time };
}

/** An admin moves a run, as the admin planner does. */
async function adminMove(page: Page, run: string, day: number, time: string): Promise<void> {
  const week = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number };
  const moved = await page.request.post(`${ADMIN}/api/admin/runs/${run}/move`, { headers: await csrf(page.request), data: { day, time, version: week.version } });
  expect(moved.ok()).toBe(true);
}

async function link(page: Page, run: string, moveTo = MOVE_TO): Promise<void> {
  await page.goto(`${PUBLIC}/runs/${run}?move_to=${encodeURIComponent(moveTo)}&sw=off`);
}

test('the run pane moves this week only: pick Wednesday, Move, the toast and the new slot', async ({ page }) => {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  const move = pane(page).getByRole('region', { name: 'Move HCarling + HStar' });
  await expect(move).toContainText('this week only, your weekly timing stays the same');
  // The run's own slot cannot be "moved to".
  await expect(move.getByRole('button', { name: 'Move', exact: true })).toBeDisabled();
  await move.getByRole('radio', { name: /^Wed 30/ }).click();
  await expect(move.locator('[data-fid="move-result"], .movepick__result')).toContainText('Wed 30 22:00');
  await move.getByRole('button', { name: 'Move', exact: true }).click();
  await expect(toast(page).filter({ hasText: 'Moved HCarling + HStar to Wed 30 22:00. The party is told on Discord.' })).toBeVisible();
  expect(await slot(page)).toEqual({ day: 6, time: '22:00' });
  // The pane follows the run to its new day.
  await expect(pane(page)).toContainText('WED 30', { ignoreCase: true });
});

test('a clash with your own run blocks the move; a teammate’s clash only warns', async ({ page }) => {
  await signInPublic(page);
  // HJupiter (Asahi's) to Wed 30 21:00: moving HCarling + HStar onto it would double-book her.
  await adminMove(page, 'r-jupiter', 6, '21:00');
  // HFA (Kaito, Yuzu, Rin, Hotaru) to Wed 30 20:00: Yuzu and Hotaru play HCarling + HStar too.
  await adminMove(page, 'r-fa', 6, '20:00');
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  const move = pane(page).getByRole('region', { name: 'Move HCarling + HStar' });
  const typed = move.getByRole('textbox', { name: 'Type a day and time' });
  await typed.fill('wed 21:00');
  await typed.press('Enter');
  await expect(move.getByRole('list', { name: 'Checks' })).toContainText('Clashes with your HJupiter 21:00: pick another time.');
  await expect(move.getByRole('button', { name: 'Move', exact: true })).toBeDisabled();
  // Wed 20:00 is HFA's: Asahi is not in it, so her teammates' clash warns and the move stays open.
  await typed.fill('wed 20:00');
  await typed.press('Enter');
  const checks = move.getByRole('list', { name: 'Checks' });
  await expect(checks).toContainText('Clash: Yuzu, Hotaru in HFA 20:00. You can still move; they will be double-booked.');
  await expect(checks).not.toContainText('Clashes with your');
  await expect(move.getByRole('button', { name: 'Move', exact: true })).toBeEnabled();
});

test('a Discord link opens the Move page with its slot picked; Move it lands on the Week with a toast', async ({ page }) => {
  await signInPublic(page);
  await link(page, 'r-carling');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Move HCarling + HStar to Wednesday 21:30?');
  const checks = page.getByRole('list', { name: 'Checks' });
  await expect(checks).toContainText('Inside this boss week (ends Wed 30 Sep 23:59)');
  await expect(checks).toContainText('No clash with your other runs');
  await expect(checks).toContainText("Tsubame hasn't answered yet");
  await expect(form(page).locator('.movepick__result')).toContainText('Wed 30 21:30');
  await page.getByRole('button', { name: 'Move it' }).click();
  await expect(page).toHaveURL(`${PUBLIC}/?run=r-carling`);
  await expect(toast(page).filter({ hasText: 'Moved HCarling + HStar to Wed 30 21:30. The party is told on Discord.' })).toBeVisible();
  await expect(pane(page)).toContainText('21:30');
  expect(await slot(page)).toEqual({ day: 6, time: '21:30' });
});

test('a stale move keeps the pick in the form and says what changed', async ({ page }) => {
  await signInPublic(page);
  await link(page, 'r-carling');
  await expect(page.getByRole('button', { name: 'Move it' })).toBeEnabled();
  // An admin moves it after the page read the week.
  await adminMove(page, 'r-carling', 5, '23:00');
  await page.getByRole('button', { name: 'Move it' }).click();
  await expect(toast(page).filter({ hasText: 'HCarling + HStar changed meanwhile: it moved to Tue 29 23:00. Nothing was moved; your pick (Wed 30 21:30) is still in the form.' })).toBeVisible();
  await expect(form(page).locator('.movepick__result')).toContainText('Wed 30 21:30');
  expect(await slot(page)).toEqual({ day: 5, time: '23:00' });
  // The pick still moves it, against the week as it is now.
  await page.getByRole('button', { name: 'Move it' }).click();
  await expect(page).toHaveURL(`${PUBLIC}/?run=r-carling`);
  expect(await slot(page)).toEqual({ day: 6, time: '21:30' });
});

test("a move older than the fresh window: Confirm it's you, back on the Move page with the slot, Move it again", async ({ page }) => {
  await signInPublic(page);
  // Aged before the page reads the session and devices, so both carry the same sign-in time.
  expect((await page.request.post(`${PUBLIC}/__mock/public/unfresh`)).ok()).toBe(true);
  await link(page, 'r-carling');
  await page.getByRole('button', { name: 'Move it' }).click();
  const confirm = page.getByRole('dialog', { name: "Confirm it's you" });
  await expect(confirm).toContainText('Moving a run needs a Discord sign-in from the last 15 minutes.');
  await expect(confirm).toContainText("Not saved yet: HCarling + HStar to Wed 30 21:30. After signing in you're back on its Move page with that time picked; press Move it once more.");
  await confirm.getByRole('link', { name: 'Sign in again' }).click();
  await expect(page).toHaveURL(`${PUBLIC}/runs/r-carling?move_to=${encodeURIComponent(MOVE_TO)}`);
  await expect(form(page).locator('.movepick__result')).toContainText('Wed 30 21:30');
  await page.getByRole('button', { name: 'Move it' }).click();
  await expect(page).toHaveURL(`${PUBLIC}/?run=r-carling`);
  expect(await slot(page)).toEqual({ day: 6, time: '21:30' });
});

test('taken off the run since the link: who did it, nothing to move, Ask to join', async ({ page }) => {
  await signInPublic(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/remove`, { data: { run: 'r-carling' } })).ok()).toBe(true);
  await link(page, 'r-carling');
  const notice = page.locator('.move-page__notice');
  await expect(notice).toContainText("You're no longer in this party. Ren removed you from HCarling + HStar on Tue 29 Sep, so you can't move it. The run is unchanged.");
  await expect(page.getByRole('heading', { level: 1 }).locator('s')).toHaveText('Move HCarling + HStar to Wednesday 21:30?');
  await expect(page.getByRole('button', { name: /^Move/ })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Ask to join…' })).toHaveAttribute('href', '/requests/new?run=r-carling');
  await expect(page.getByRole('link', { name: 'Open the run' })).toHaveAttribute('href', '/?run=r-carling');
  await expect(page.getByText('not in this run · view only')).toBeVisible();
});

test("last week's link: the week is over, this week's run and your weekly timing", async ({ page }) => {
  await signInPublic(page);
  await link(page, 'p-kalos', '2026-09-18T14:00:00Z');
  const notice = page.locator('.move-page__notice');
  await expect(notice).toContainText('That boss week is over. The link was for the week of');
  await expect(notice).toContainText('so nothing was moved.');
  await expect(page.locator('.move-page__now .label')).toHaveText("This week's run");
  // The weekly timing (f-kalos, Fri 21:30), not where that week's run happened to be.
  await expect(page.getByText('Your weekly timing is Fri 21:30.')).toBeVisible();
  await expect(page.getByRole('link', { name: "Open this week's runs" })).toHaveAttribute('href', '/?run=r-kalos');
  await expect(page.getByRole('link', { name: 'Ask for a change…' })).toHaveAttribute('href', '/requests/new?fixed=f-kalos&change=edit');
  await expect(page.getByRole('button', { name: /^Move/ })).toHaveCount(0);
});

test('the boss week reset with the link open: the run reads as past, nothing moves', async ({ page }) => {
  await signInPublic(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/end-week`)).ok()).toBe(true);
  await link(page, 'r-carling');
  await expect(page.locator('.move-page__notice')).toContainText('That boss week is over.');
  await expect(page.getByRole('button', { name: /^Move/ })).toHaveCount(0);
});

test('a run that already started: the link says so and opens the run instead', async ({ page }) => {
  await signInPublic(page);
  // XKalos was Fri 25 22:00.
  await link(page, 'r-kalos');
  await expect(page.locator('.move-page__notice')).toContainText("That run has started. XKalos can't move once it is under way, so nothing was moved.");
  await expect(page.getByRole('link', { name: 'Open the run' })).toHaveAttribute('href', '/?run=r-kalos');
  await expect(page.getByRole('button', { name: /^Move/ })).toHaveCount(0);
  // Nor does the pane offer Move on it.
  await page.goto(`${PUBLIC}/?run=r-kalos&sw=off`);
  await expect(page.getByRole('complementary', { name: /^Your run · / }).getByRole('group', { name: /^Your answer/ })).toBeVisible();
  await expect(page.getByRole('region', { name: /^Move / })).toHaveCount(0);
});

test('someone else’s run: the link cannot move it and asks to join', async ({ page }) => {
  await signInPublic(page);
  await link(page, 'r-limbo');
  await expect(page.locator('.move-page__notice')).toContainText("You're not in this party. Only its party can move HLimbo, so nothing changed.");
  await expect(page.getByRole('button', { name: /^Move/ })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Ask to join…' })).toBeVisible();
});

test('phone: the Move bar stays at the bottom while the choices scroll, with 44 px keys', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signInPublic(page);
  await link(page, 'r-carling');
  const go = page.getByRole('button', { name: 'Move', exact: true });
  await expect(go).toBeVisible();
  await settle(page);
  const before = (await go.boundingBox())!;
  for (const name of ['Cancel', 'Move']) expect(Math.round((await page.getByRole('button', { name, exact: true }).boundingBox())!.height)).toBeGreaterThanOrEqual(44);
  expect(before.y + before.height).toBeLessThanOrEqual(844);
  // The choices scroll inside the window; the page itself never does, and the bar keeps its place.
  await page.locator('.member-move__body').evaluate((el) => el.scrollTo(0, el.scrollHeight));
  expect((await go.boundingBox())!.y).toBe(before.y);
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight - innerHeight)).toBeLessThanOrEqual(0);
  await go.click();
  await expect(page).toHaveURL(`${PUBLIC}/?run=r-carling`);
  expect(await slot(page)).toEqual({ day: 6, time: '21:30' });
});

test('phone: a stale link keeps its way on in the bottom bar', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signInPublic(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/remove`, { data: { run: 'r-carling' } })).ok()).toBe(true);
  await link(page, 'r-carling');
  const foot = page.locator('.move-page__foot');
  await expect(foot.getByRole('link', { name: 'Ask to join…' })).toBeVisible();
  await settle(page);
  for (const key of await foot.getByRole('link').all()) expect(Math.round((await key.boundingBox())!.height)).toBeGreaterThanOrEqual(44);
  const box = (await foot.boundingBox())!;
  expect(Math.round(box.y + box.height)).toBeLessThanOrEqual(844);
});
