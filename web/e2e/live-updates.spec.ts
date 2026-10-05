import type { Page } from '@playwright/test';
import { ADMIN, csrf, expect, test } from './support';

// Live updates (stage 1): the mock's `GET /api/admin/events` hints that
// something changed and the open page re-reads it in place, with no reload.
// `POST /__mock/arrive` stands in for a change made outside the portal (a
// Discord reaction, the extractor, the chatbot). With the stream blocked the
// polls still bring the change, at their normal cadence.

/** Something made outside this page; the mock emits the matching hint. */
async function arrive(page: Page, kind: 'reaction' | 'proposal' | 'chat' | 'extraction') {
  const reply = await page.request.post(`${ADMIN}/__mock/arrive`, { data: { kind } });
  expect(reply.status()).toBe(204);
}

/** Marks the document; `stillSame` fails if the page reloaded or navigated away. */
async function mark(page: Page) {
  await page.evaluate(() => ((window as unknown as { __kept: boolean }).__kept = true));
  return async () => expect(await page.evaluate(() => (window as unknown as { __kept?: boolean }).__kept)).toBe(true);
}

/** Well under the 15 s poll: only a hint can explain an update this soon. */
const BY_HINT = { timeout: 6_000 };

test('Week: a reaction made in Discord reaches the open board without a reload', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  const kalos = page.locator('[data-run="r-kalos"]');
  await expect(kalos).toBeVisible();
  // Ren answered no: three of four are on.
  await expect(kalos).toContainText('3/4');
  const stillSame = await mark(page);
  // The stream is open (the mock answers it), so the reaction is hinted.
  await expect.poll(() => page.evaluate(() => performance.getEntriesByType('resource').some((e) => e.name.includes('/api/admin/events')))).toBe(true);
  await arrive(page, 'reaction');
  await expect(kalos).toContainText('4/4', BY_HINT);
  await stillSame();
});

test('Inbox: a new proposal joins the list and the badge, and the open item stays open', async ({ page }) => {
  await page.goto(`${ADMIN}/inbox?sw=off`);
  const list = page.getByRole('listbox', { name: 'Extractor items' });
  await expect(list.getByRole('option').first()).toBeVisible();
  const count = await list.getByRole('option').count();
  const first = list.getByRole('option').first();
  await first.click();
  await expect(first).toHaveAttribute('aria-selected', 'true');
  const selected = (await first.textContent()) ?? '';
  const nav = page.locator('.navrail').getByRole('navigation', { name: 'Sections' });
  await expect(nav.getByRole('link', { name: 'Inbox 9 waiting', exact: true })).toBeVisible();
  const stillSame = await mark(page);
  await arrive(page, 'proposal');
  await expect(nav.getByRole('link', { name: 'Inbox 10 waiting', exact: true })).toBeVisible(BY_HINT);
  await expect(list.getByRole('option')).toHaveCount(count + 1, BY_HINT);
  await expect(list.getByRole('option').last()).toContainText('Mon 28 Sep 21:00');
  // The selection did not move to the newcomer.
  await expect(list.getByRole('option', { selected: true })).toHaveText(selected);
  await stillSame();
});

test('Chat: a new interaction appears at the top of the log without a reload', async ({ page }) => {
  await page.goto(`${ADMIN}/chat?sw=off`);
  const first = page.locator('.chat-row').first();
  await expect(first).toBeVisible();
  const opened = (await page.locator('.chat__detail').textContent()) ?? '';
  const stillSame = await mark(page);
  await arrive(page, 'chat');
  await expect(first).toContainText('is limbo still on tonight?', BY_HINT);
  // The turn open by default stays open.
  await expect(page.locator('.chat__detail')).toHaveText(opened);
  await stillSame();
});

test('Extractions: a new call appears without a reload', async ({ page }) => {
  await page.goto(`${ADMIN}/extractions?sw=off`);
  const rows = page.getByRole('listbox', { name: /Extraction calls/ }).getByRole('option');
  await expect(rows.first()).toBeVisible();
  const opened = await rows.first().innerText();
  const stillSame = await mark(page);
  await arrive(page, 'extraction');
  // Newest first; the call open by default stays open rather than jumping to it.
  await expect(rows.first()).toContainText('#limbo-trio', BY_HINT);
  await expect(rows.first()).toContainText('no change');
  await expect(rows.filter({ hasText: opened.split('\n')[0]! }).first()).toHaveAttribute('aria-selected', 'true');
  await stillSame();
});

test('with the stream blocked, the Week poll still brings the change at its normal cadence', async ({ page }) => {
  await page.clock.install();
  await page.route(`${ADMIN}/api/admin/events`, (route) => route.abort());
  await page.goto(`${ADMIN}/?sw=off`);
  const kalos = page.locator('[data-run="r-kalos"]');
  await expect(kalos).toBeVisible();
  await expect(kalos).toContainText('3/4');
  const stillSame = await mark(page);
  await arrive(page, 'reaction');
  // No hint can arrive: nothing changes until the 15 s poll.
  await page.clock.runFor(5_000);
  await expect(kalos).toContainText('3/4');
  await expect(async () => {
    await page.clock.runFor(5_000);
    await expect(kalos).toContainText('4/4', { timeout: 500 });
  }).toPass({ timeout: 20_000 });
  await stillSame();
});

test('Config: a settings hint refreshes the page but keeps an unsaved draft', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=pings&sw=off`);
  const field = page.getByRole('textbox', { name: 'Morning ping' });
  await expect(field).toBeVisible();
  await field.fill('08:45');
  const stillSame = await mark(page);
  // Another admin turns quiet mode on: the hint re-reads Config and the shell.
  const reread = page.waitForResponse((r) => r.url().endsWith('/api/admin/config') && r.request().method() === 'GET');
  const reply = await page.request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(page.request), data: { notifications: { quiet_mode: true } } });
  expect(reply.status()).toBe(200);
  await reread;
  await expect(page.locator('.pageline .quiet')).toBeVisible(BY_HINT);
  await expect(field).toHaveValue('08:45');
  await expect(page.getByRole('tab', { name: /^Pings/ })).toBeVisible();
  await stillSame();
});

/** The Run lengths default, saved by another admin through the API. */
async function saveDefaultMinutes(page: Page, minutes: number) {
  const config = (await (await page.request.get(`${ADMIN}/api/admin/config`)).json()) as { run_lengths: { default_minutes: number; overrides: unknown[] } };
  const reply = await page.request.patch(`${ADMIN}/api/admin/config`, {
    headers: await csrf(page.request),
    data: { run_lengths: { default_minutes: minutes, overrides: config.run_lengths.overrides } },
  });
  expect(reply.status()).toBe(200);
  return config.run_lengths.default_minutes;
}

test('Config: an untouched section follows a settings hint; an edited one keeps the edit', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=run-lengths&sw=off`);
  const panel = page.getByRole('tabpanel', { name: /Run lengths/ });
  const each = panel.locator('.minutes__value').first();
  await expect(each).toBeVisible();
  const was = Number(await each.inputValue());
  const savebar = panel.locator('.savebar');
  await expect(savebar).not.toHaveClass(/savebar--dirty/);

  // Untouched: the form shows the other admin's value and nothing reads as unsaved.
  await saveDefaultMinutes(page, was + 10);
  await expect(each).toHaveValue(String(was + 10), BY_HINT);
  await expect(savebar).not.toHaveClass(/savebar--dirty/);
  await expect(savebar).toContainText('All changes saved');

  // Edited: the edit stays (and still reads as unsaved) when another save lands.
  await each.fill(String(was + 30));
  await each.blur();
  await expect(savebar).toHaveClass(/savebar--dirty/);
  const reread = page.waitForResponse((r) => r.url().endsWith('/api/admin/config') && r.request().method() === 'GET');
  await saveDefaultMinutes(page, was + 20);
  await reread;
  await expect(each).toHaveValue(String(was + 30));
  await expect(savebar).toHaveClass(/savebar--dirty/);
});

test('Members: a roster change reaches the open list and sheet without a reload', async ({ page }) => {
  await page.goto(`${ADMIN}/members?sw=off`);
  // Mika's aliases, as the list prints them.
  const mika = page.getByRole('main').locator('span.id', { hasText: /^mika · mk/ }).first();
  await expect(mika).toBeVisible();
  const stillSame = await mark(page);
  await arrive(page, 'member');
  await expect(mika).toContainText('mikan', BY_HINT);
  await stillSame();
});

test('a hidden tab closes the stream and reads nothing until it is shown again', async ({ page }) => {
  // Counts the page's EventSources and whether each is still open.
  await page.addInitScript(() => {
    const Native = window.EventSource;
    const opened: EventSource[] = [];
    (window as unknown as { __streams: EventSource[] }).__streams = opened;
    window.EventSource = class extends Native {
      constructor(url: string | URL, init?: EventSourceInit) {
        super(url, init);
        opened.push(this);
      }
    } as typeof EventSource;
  });
  await page.clock.install();
  await page.goto(`${ADMIN}/?sw=off`);
  const kalos = page.locator('[data-run="r-kalos"]');
  await expect(kalos).toContainText('3/4');
  const streams = () => page.evaluate(() => (window as unknown as { __streams: EventSource[] }).__streams.map((s) => s.readyState));
  // Live (the mock's stream reconnects between hints, so open or connecting), never closed.
  const live = async () => (await streams()).some((state) => state !== 2);
  await expect.poll(live).toBe(true);
  const before = (await streams()).length;

  const setHidden = (hidden: boolean) =>
    page.evaluate((h) => {
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (h ? 'hidden' : 'visible') });
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => h });
      document.dispatchEvent(new Event('visibilitychange'));
    }, hidden);
  await setHidden(true);
  await expect.poll(live).toBe(false);
  const reads: string[] = [];
  page.on('request', (r) => {
    if (r.url().includes('/api/admin/') && r.method() === 'GET') reads.push(r.url());
  });
  // Something changes in Discord while the tab is hidden, and minutes pass.
  await arrive(page, 'reaction');
  await page.clock.fastForward(3 * 60_000);
  expect(reads).toEqual([]);
  expect((await streams()).every((state) => state === 2)).toBe(true);
  await expect(kalos).toContainText('3/4');

  // Shown again: a new stream, and the change arrives.
  await setHidden(false);
  await expect.poll(live).toBe(true);
  expect((await streams()).length).toBeGreaterThan(before);
  await expect(async () => {
    await page.clock.runFor(1_000);
    await expect(kalos).toContainText('4/4', { timeout: 500 });
  }).toPass({ timeout: 15_000 });
});
