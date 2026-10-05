import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Live Limits (B_LimitsLive and its tab boards): one row card per model
// group, the Queue / Admission / Allowances tabs, the phone order and
// sideways tab strip, the footer's server time and the stale chip. The mock
// seeds three groups with `POST /__mock/limits {groups: "three"}` (cloud full
// and queueing, local half-open at 2/4, legacy open); the fixture's reset
// restores its one gateway group.

async function seedThree(): Promise<void> {
  const res = await fetch(`${ADMIN}/__mock/limits`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ groups: 'three' }),
  });
  expect(res.status).toBe(204);
}

async function open(page: Page, query = ''): Promise<void> {
  await page.goto(`${ADMIN}/limits?sw=off${query}`);
  await expect(page.getByRole('tabpanel')).toBeVisible();
}

const cards = (page: Page) => page.getByRole('tabpanel').getByRole('article');

test('Backends: a row card per group in the configured order, breaker in words, waits and the open breaker sentence', async ({ page }) => {
  await seedThree();
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
  await seedThree();
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
  await seedThree();
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

test('footer: the server clock at the snapshot, never the browser clock', async ({ page }) => {
  // A browser clock a day and a few hours away from the mock's.
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
    await seedThree();
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
