import type { Limits } from '@kanade/api-types';
import type { Page } from '@playwright/test';
import { ADMIN, expect, setLimitGroups, test, unconditional } from './support';

// Live Limits (B_LimitsLive and its tab boards): one row card per model
// group, the Queue / Admission / Allowances tabs, the phone order and
// sideways tab strip, the footer's server time and the stale chip. The e2e
// server's canned governor shows three groups after `setLimitGroups('three')`
// (cloud full and queueing, local half-open at 2/4, legacy open); the
// fixture's reset restores its one gateway group. Allowance windows are the
// chat runtime's own, filled with `/__e2e/chat-allowance`.

/** The seed's member snowflakes. */
const ASAHI = '100000000000001001';
const REN = '100000000000001002';
const RIN = '100000000000001010';
const KOHANE = '100000000000001014';

/** The chatbot answers `answers` for `member`, as if asked in a watched channel. */
async function spend(member: string, answers: number): Promise<void> {
  const spent = await fetch(`${ADMIN}/__e2e/chat-allowance`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ member, answers }) });
  expect(spent.status, await spent.text()).toBe(204);
}

/** `seconds` after an ISO instant, as the server writes them. */
const after = (at: string, seconds: number) => new Date(Date.parse(at) + seconds * 1000).toISOString().replace('.000Z', 'Z');

/**
 * Rewrites each Limits read the page makes (the real answer, fetched): the
 * real server records no admission refusals yet (`AdmissionWindow::last_hour`
 * is always empty), so the Admission tab gets the board's four.
 */
async function withRefusals(page: Page): Promise<void> {
  await page.route(`${ADMIN}/api/admin/limits`, async (route) => {
    const response = await route.fetch(unconditional(route));
    const body = (await response.json()) as Limits;
    const named = (at: number) => (body.groups[at] ?? body.groups[0])?.name ?? 'gateway';
    body.admission.refusals = [
      { kind: 'rate', scope: 'group', target: named(1), count: 3, last_at: after(body.generated_at, -6 * 60) },
      { kind: 'concurrency', scope: 'group', target: named(0), count: 1, last_at: after(body.generated_at, -41 * 60) },
      { kind: 'quota', scope: 'key', target: 'gateway key …7f2a', count: 1, last_at: after(body.generated_at, -22 * 60) },
      { kind: 'key_rate', scope: 'key', target: 'gateway key …7f2a', count: 2, last_at: after(body.generated_at, -9 * 60) },
    ];
    await route.fulfill({ response, json: body });
  });
}

async function open(page: Page, query = ''): Promise<void> {
  await page.goto(`${ADMIN}/limits?sw=off${query}`);
  await expect(page.getByRole('tabpanel')).toBeVisible();
}

const cards = (page: Page) => page.getByRole('tabpanel').getByRole('article');

test('Backends: a row card per group in the configured order, breaker in words, waits and the open breaker sentence', async ({ page }) => {
  await setLimitGroups('three');
  await withRefusals(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('cloud is at capacity');
  await expect(cards(page).getByRole('heading', { level: 3 })).toHaveText(['cloud', 'local', 'legacy']);

  const cloud = page.getByRole('article', { name: 'cloud' });
  await expect(cloud).toContainText('closed — calls flow');
  await expect(cloud).toContainText(/2\/2\s*permits in use\s*· requests in flight/);
  await expect(cloud.getByRole('listitem')).toHaveCount(2);
  await expect(cloud).not.toContainText('Waiting none');
  await expect(cloud.locator('dd').filter({ hasText: 'kanata/rewrite-cloud' })).toBeVisible();

  const local = page.getByRole('article', { name: 'local' });
  await expect(local).toContainText('half-open — probing');
  await expect(local.getByRole('term').filter({ hasText: 'Waiting' })).toBeVisible();
  await expect(local.getByRole('definition').filter({ hasText: /^none$/ })).toBeVisible();
  await expect(local.getByRole('term').filter({ hasText: 'Next probe' })).toHaveCount(0);

  const legacy = page.getByRole('article', { name: 'legacy' });
  await expect(legacy).toContainText('open — calls refused');
  await expect(legacy).toContainText(/0\/2\s*permits in use\s*· idle/);
  await expect(legacy).toContainText(/Calls to legacy are refused\suntil the next probe at Tue 29 Sep 12:03\./);
  await expect(legacy.getByRole('term').filter({ hasText: 'Next probe' })).toBeVisible();

  // Refusals live only in the Admission tab.
  await expect(page.getByRole('tabpanel')).not.toContainText(/rate limit|key quota|refused\s*$/);
  await expect(page.getByRole('link', { name: 'Capacity in Config' })).toHaveAttribute('href', '/config?section=models');
  await expect(page.getByRole('tab')).toHaveText([/Backends\s*3/, /Queue\s*2/, /Admission\s*7/, /Allowances\s*\d+/]);
});

test('Backends: two bars wave at most, the open breaker stays flat, and reduced motion draws every bar flat', async ({ page }) => {
  await setLimitGroups('three');
  await open(page);
  const bar = (name: string) => page.getByRole('progressbar', { name: `${name} permits in use` });
  await expect(bar('cloud')).toHaveAttribute('aria-valuetext', '2 of 2 in use, requests in flight');
  await expect(bar('cloud')).not.toHaveClass(/wavy--flat/);
  await expect(bar('local')).not.toHaveClass(/wavy--flat/);
  await expect(bar('legacy')).toHaveClass(/wavy--flat/);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  for (const name of ['cloud', 'local', 'legacy']) {
    const animated = await bar(name).evaluate((el) => el.getAnimations({ subtree: true }).filter((a) => a.playState === 'running').length);
    expect(animated, `${name} moves under reduced motion`).toBe(0);
  }
});

test('tabs: arrow keys move between tabs and each keeps its own panel', async ({ page }) => {
  await setLimitGroups('three');
  await withRefusals(page);
  await open(page);
  const backends = page.getByRole('tab', { name: /Backends/ });
  await backends.focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: /Queue/ })).toBeFocused();
  await expect(page.getByRole('tab', { name: /Queue/ })).toHaveAttribute('aria-selected', 'true');
  const queue = page.getByRole('table', { name: /Waiting for a permit/ });
  await expect(queue.getByRole('row', { name: /rescan/ })).toContainText('42 s');
  await expect(page.getByRole('tabpanel')).toContainText('Nothing is waiting for local or legacy.');
  await expect(page.locator('footer')).toContainText(/Oldest\s*42 s rescan · cloud/);

  await page.keyboard.press('ArrowRight');
  const admission = page.getByRole('table', { name: 'Admission refusals by kind' });
  await expect(admission.getByRole('rowheader')).toHaveText(['rate limit', 'too many in flight', 'key quota spent', 'key rate limit']);
  await expect(admission.locator('th[scope="colgroup"]')).toHaveText([/^Backend groups\s*4 refused$/, /^Gateway key\s*3 refused$/]);
  await expect(admission.getByRole('row', { name: /^rate limit/ })).toContainText('local');

  await page.keyboard.press('End');
  await expect(page.getByRole('tab', { name: /Allowances/ })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Home');
  await expect(backends).toHaveAttribute('aria-selected', 'true');
});

test('Allowances: Reset only where the window has use, and it clears that window', async ({ page }) => {
  // Ren has two answers in his window; nobody else has any.
  await spend(REN, 2);
  await open(page);
  await page.getByRole('tab', { name: /Allowances/ }).click();
  const table = page.getByRole('table', { name: 'Chatbot allowances' });
  const rows = table.locator('tbody tr');
  const total = await rows.count();
  const used = rows.filter({ hasText: / used,/ });
  const withUse = await used.count();
  expect(withUse).toBeGreaterThan(0);
  expect(withUse).toBeLessThan(total);
  await expect(table.getByRole('button', { name: /^Reset .*'s window$/ })).toHaveCount(withUse);
  const reset = used.first().getByRole('button', { name: /^Reset .*'s window$/ });
  const name = (await reset.getAttribute('aria-label'))!.replace(/^Reset (.*)'s window$/, '$1');
  await reset.click();
  await expect(page.getByRole('group', { name: 'Notification' }).filter({ hasText: `${name}'s window is reset.` })).toBeVisible();
  await expect(table.getByRole('button', { name: `Reset ${name}'s window` })).toHaveCount(0);
});

test('Allowances: after a Reset from the keyboard, focus lands on that member, not the page', async ({ page }) => {
  await spend(REN, 2);
  await open(page);
  await page.getByRole('tab', { name: /Allowances/ }).click();
  const table = page.getByRole('table', { name: 'Chatbot allowances' });
  const reset = table.getByRole('button', { name: /^Reset .*'s window$/ }).first();
  const name = (await reset.getAttribute('aria-label'))!.replace(/^Reset (.*)'s window$/, '$1');
  await reset.focus();
  await page.keyboard.press('Enter');
  await expect(table.getByRole('button', { name: `Reset ${name}'s window` })).toHaveCount(0);
  const focused = page.locator(':focus');
  await expect(focused).toHaveAttribute('scope', 'row');
  await expect(focused).toContainText(name);
  // The toast region (polite) announces it.
  await expect(page.getByRole('region', { name: 'Notifications' })).toContainText(`${name}'s window is reset.`);
});

test('Allowances: "resets in" counts down on the server clock, only where answers count', async ({ page }) => {
  // A browser clock days away from the server's: the countdown must not follow it.
  await page.clock.install({ time: new Date('2026-10-02T21:13:00Z') });
  await page.clock.pauseAt(new Date('2026-10-02T21:13:01Z'));
  await page.setViewportSize({ width: 1280, height: 800 });
  // The chat runtime counts its windows in monotonic time from real answers, so real
  // windows never sit at a fixed point: the read is the real one with the board's two
  // windows written in (Rin on her own 20 per 6 h override, Ren on 4 per 5 min).
  await page.route(`${ADMIN}/api/admin/limits`, async (route) => {
    const response = await route.fetch(unconditional(route));
    const body = (await response.json()) as Limits;
    for (const row of body.allowances) {
      if (row.member.id === RIN) Object.assign(row, { allowance: { count: 20, per_s: 21_600 }, used: 7, overridden: true, resets_at: after(body.generated_at, 18_720) });
      if (row.member.id === REN) Object.assign(row, { allowance: { count: 4, per_s: 300 }, used: 2, resets_at: after(body.generated_at, 132) });
    }
    await route.fulfill({ response, json: body });
  });
  await open(page);
  await page.getByRole('tab', { name: /Allowances/ }).click();
  const table = page.getByRole('table', { name: 'Chatbot allowances' });
  const row = (id: string) => table.locator('tbody tr', { has: page.locator(`#limits-allowance-${id}`) });
  // Rin runs on her own 20 per 6 h; Ren on the guild's 4 per 5 min.
  await expect(row(RIN)).toContainText('7 used, 13 left · resets in 5 h 12 m');
  await expect(row(REN)).toContainText('2 used, 2 left · resets in 2 m 12 s');
  // The allowance is a bar of answers used against the count, the window in words.
  const rin = row(RIN).getByRole('progressbar', { name: "Rin's answers used" });
  await expect(rin).toHaveAttribute('aria-valuenow', '7');
  await expect(rin).toHaveAttribute('aria-valuemax', '20');
  await expect(rin).toHaveAttribute('aria-valuetext', '7 of 20 answers used, 6 h window');
  await expect(row(RIN)).toContainText('20 per 6 h');
  await expect(row(REN)).toContainText('4 per 5 min');
  await expect(table).not.toContainText(/per \d+s/);
  await expect(row(ASAHI).getByRole('progressbar')).toHaveCount(0);
  await expect(row(ASAHI)).toContainText('exempt');
  // Staff and an idle window have nothing to reset.
  await expect(row(ASAHI)).not.toContainText('resets');
  await expect(row(KOHANE)).toContainText('idle');
  await expect(row(KOHANE)).not.toContainText('resets');
  // Monotonic time since the snapshot moves it; the next poll is 5 s away.
  await page.clock.runFor(3000);
  await expect(row(REN)).toContainText('resets in 2 m 9 s');
});

test('footer: the server clock at the snapshot, never the browser clock', async ({ page }) => {
  // A browser clock a day and a few hours away from the server's.
  await page.clock.setFixedTime('2026-09-30T09:37:00Z');
  await open(page);
  await expect(page.locator('footer').filter({ hasText: 'Updated' })).toContainText('Updated Tue 29 Sep 12:00 · every 5 s');
});

test('a failed refresh swaps the Live chip for a Retrying chip and keeps the last snapshot', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page);
  await expect(page.getByRole('group', { name: 'Status' })).toBeVisible();
  await page.route(`${ADMIN}/api/admin/limits`, (route) => route.fulfill({ status: 503, json: { error: 'unavailable', message: 'Busy.' } }));
  const chip = page.getByRole('status').filter({ hasText: /Retrying\s·\slast updated 12:00/ });
  await expect(chip).toBeVisible({ timeout: 12_000 });
  await expect(page.getByRole('group', { name: 'Status' })).toBeHidden();
  await expect(page.getByRole('article', { name: 'gateway' })).toBeVisible();
  await page.unroute(`${ADMIN}/api/admin/limits`);
  await expect(chip).toHaveCount(0, { timeout: 12_000 });
  await expect(page.getByRole('group', { name: 'Status' })).toBeVisible();
});

test('after six failed refreshes the chip says refreshing stopped, and Try again restarts it', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.clock.install();
  await open(page);
  await page.route(`${ADMIN}/api/admin/limits`, (route) => route.fulfill({ status: 503, json: { error: 'unavailable', message: 'Busy.' } }));
  const stopped = page.getByRole('status').filter({ hasText: /Refreshing stopped\s·\slast updated 12:00/ });
  // Each step passes the longest backoff (40 s), so every pending poll fires.
  await expect(async () => {
    await page.clock.fastForward(45_000);
    await expect(stopped).toBeVisible({ timeout: 500 });
  }).toPass({ timeout: 30_000 });
  await expect(page.getByRole('status').filter({ hasText: 'Retrying' })).toHaveCount(0);
  await page.unroute(`${ADMIN}/api/admin/limits`);
  await page.getByRole('button', { name: 'Try again' }).click();
  await expect(stopped).toHaveCount(0);
  await expect(page.getByRole('group', { name: 'Status' })).toBeVisible();
});

test('a failed first read shows the failed state with Try again, not an endless spinner', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.route(`${ADMIN}/api/admin/limits`, (route) => route.fulfill({ status: 503, json: { error: 'unavailable', message: 'Busy.' } }));
  await page.goto(`${ADMIN}/limits?sw=off`);
  const failed = page.getByRole('alert').filter({ hasText: /Couldn.t load the limits/ });
  await expect(failed).toBeVisible();
  await expect(failed).toContainText('Busy.');
  await expect(failed.getByRole('button', { name: 'Copy details' })).toBeVisible();
  await expect(page.getByText('Loading the limits…')).toHaveCount(0);
  await page.unroute(`${ADMIN}/api/admin/limits`);
  await failed.getByRole('button', { name: 'Try again' }).click();
  await expect(page.getByRole('tabpanel')).toBeVisible();
  await expect(failed).toHaveCount(0);
});

test.describe('phone', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('open, then half-open breakers lead; the tab strip keeps all four counts and scrolls sideways', async ({ page }) => {
    await setLimitGroups('three');
    await withRefusals(page);
    await open(page);
    await expect(cards(page).getByRole('heading', { level: 3 })).toHaveText(['legacy', 'local', 'cloud']);
    const strip = page.getByRole('tablist', { name: 'Limits' });
    await expect(strip.getByRole('tab')).toHaveText([/Backends\s*3/, /Queue\s*2/, /Admission\s*7/, /Allowances\s*\d+/]);
    const overflow = await strip.evaluate((el) => ({ scroll: el.scrollWidth, client: el.clientWidth, x: getComputedStyle(el).overflowX }));
    expect(overflow.x).toBe('auto');
    expect(overflow.scroll).toBeGreaterThan(overflow.client);
    await expect(strip).toHaveClass(/limits-window__tabs--end/);
    // Choosing the last tab brings it fully into the strip; the start fade follows.
    await page.getByRole('tab', { name: /Backends/ }).focus();
    await page.keyboard.press('End');
    const allowances = page.getByRole('tab', { name: /Allowances/ });
    await expect(allowances).toHaveAttribute('aria-selected', 'true');
    await expect(strip).toHaveClass(/limits-window__tabs--start/);
    const inside = await allowances.evaluate((tab) => {
      const s = tab.parentElement!.getBoundingClientRect();
      const t = tab.getBoundingClientRect();
      return t.left >= s.left - 0.5 && t.right <= s.right + 0.5;
    });
    expect(inside).toBe(true);
    // Only the strip moved: the document never scrolls sideways.
    expect(await page.evaluate(() => document.scrollingElement!.scrollLeft)).toBe(0);
  });

  test('Admission reads each refusal as a two-line row, grouped by scope', async ({ page }) => {
    await withRefusals(page);
    await open(page);
    await page.getByRole('tab', { name: /Admission/ }).click();
    const panel = page.getByRole('tabpanel');
    await expect(panel.getByRole('heading', { level: 3 })).toHaveText([/Backend groups\s*4 refused/, /Gateway key\s*3 refused/]);
    await expect(panel.getByRole('listitem').first()).toContainText(/rate limit\s*backend\s*3/);
    await expect(page.locator('footer')).toContainText('7 refusals');
  });

  test('a failed refresh shows the Retrying chip under the tabs, on screen', async ({ page }) => {
    await open(page);
    await page.route(`${ADMIN}/api/admin/limits`, (route) => route.fulfill({ status: 503, json: { error: 'unavailable', message: 'Busy.' } }));
    const window = page.getByRole('region', { name: 'Limits' });
    const chip = window.getByRole('status').filter({ hasText: /Retrying\s·\slast updated 12:00/ });
    await expect(chip).toBeVisible({ timeout: 12_000 });
    const box = (await chip.boundingBox())!;
    expect(box.width).toBeGreaterThan(40);
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.y + box.height).toBeLessThanOrEqual(844);
    // The snapshot stays.
    await expect(window.getByRole('article', { name: 'gateway' })).toBeVisible();
    await page.unroute(`${ADMIN}/api/admin/limits`);
    await expect(chip).toHaveCount(0, { timeout: 12_000 });
  });
});
