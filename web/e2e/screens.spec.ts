import type { Page } from '@playwright/test';
import { ADMIN, csrf, expect, test } from './support';

// Fixed, Bosses, Members, Reminders and the run sheet's weekly-timing tools,
// against the mock pinned to Tue 29 Sep 2026 12:00 (playwright.config.ts).

async function go(page: Page, path: string) {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

const toast = (page: Page, text: string | RegExp) => page.getByRole('group', { name: 'Notification' }).filter({ hasText: text });

test('fixed: table, bosscheck, add a timing, and its runs reach the board', async ({ page }) => {
  await go(page, '/fixed');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('8 weekly timings');
  const bmRow = page.getByRole('row', { name: /Black Mage/ });
  await expect(bmRow.getByText('not watched')).toBeVisible();
  await expect(page.getByRole('row', { name: /Gatekeeper Kalos/ }).getByText('1 run amended')).toBeVisible();

  await page.getByRole('button', { name: 'Add a weekly timing' }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  await editor.getByRole('textbox', { name: '…or type them' }).fill('hstar, cfoo');
  await expect(editor.getByRole('status').filter({ hasText: 'is not a boss' })).toBeVisible();
  await editor.getByRole('textbox', { name: '…or type them' }).fill('');
  // The pill's input is visually hidden (v4 pill-toggle); a pointer presses the pill itself.
  await editor.locator('.bossrow', { hasText: 'Limbo' }).locator('label', { hasText: 'HARD' }).click();
  await expect(editor.getByRole('checkbox', { name: 'Hard Limbo' })).toBeChecked();
  await editor.getByLabel('Day').selectOption({ label: 'Wednesday' });
  await editor.getByLabel('Time').fill('20:30');
  await editor.getByLabel('Home channel').selectOption({ label: '#limbo-trio' });
  await editor.getByRole('checkbox', { name: 'Mika' }).check();
  await editor.getByRole('checkbox', { name: 'Nagi' }).check();
  await editor.getByRole('button', { name: 'Add timing' }).click();
  await expect(editor).toBeHidden();
  await expect(toast(page, 'Added Wednesday 20:30 — HLimbo')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 weekly timings');

  await page.getByRole('link', { name: 'Week' }).click();
  const wed = page.locator('section.board__col').filter({ has: page.locator('h2 .board__dow:text-is("Wed")') });
  await expect(wed.locator('.runcard', { hasText: '20:30' })).toContainText('HLimbo');
});

test('fixed: editing a timing with an amended run asks update or keep', async ({ page }) => {
  await go(page, '/fixed');
  await page.getByRole('button', { name: 'Edit Friday 21:30 — XKalos' }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  await expect(editor.getByRole('checkbox', { name: 'Extreme Gatekeeper Kalos' })).toBeChecked();
  await editor.getByLabel('Time').fill('21:00');
  await editor.getByRole('button', { name: 'Save…' }).click();
  const choice = editor.getByRole('group', { name: /^#5a6b7c8d · Fri 25 22:00/ });
  await expect(choice.getByRole('radio', { name: "Keep this week's change" })).toBeChecked();
  await choice.getByRole('radio', { name: 'Update to the new timing' }).check();
  await editor.getByRole('button', { name: 'Save changes' }).click();
  await expect(toast(page, 'Saved Friday 21:00 — XKalos.')).toBeVisible();
  await page.getByRole('link', { name: 'Week' }).click();
  await page.getByRole('button', { name: 'Show them' }).click();
  await expect(page.locator('[data-run="r-kalos"]')).toContainText('21:00');
});

type FixedApiRow = { id: string; weekday: number; time: string; bosses: { token: string }[]; participants: { id: string }[]; channel_id: string; note: string | null };

/** Another admin edits XBM's timing through the API, as the server would see it. */
async function editXbmBehind(page: Page, change: Partial<{ note: string; time: string }>) {
  const { version } = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number };
  const rows = (await (await page.request.get(`${ADMIN}/api/admin/fixed`)).json()) as FixedApiRow[];
  const bm = rows.find((r) => r.bosses.some((b) => b.token === 'XBM'))!;
  const response = await page.request.patch(`${ADMIN}/api/admin/fixed/${encodeURIComponent(bm.id)}`, {
    headers: await csrf(page.request),
    data: {
      weekday: bm.weekday,
      time: bm.time,
      bosses: bm.bosses.map((b) => b.token).join(' '),
      participants: bm.participants.map((p) => p.id),
      channel_id: bm.channel_id,
      note: bm.note,
      version,
      ...change,
    },
  });
  expect(response.ok()).toBe(true);
  return version;
}

test('fixed: an edit sends the version it was loaded at; conflicts are per field, as on the server', async ({ page }) => {
  await go(page, '/fixed');
  const editButton = page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' });
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  await editButton.click();
  await editor.getByLabel('Note').fill('Bring potions');
  // A change to another run moves the week version but touches none of this timing's fields.
  const week = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number; runs: { id: string; day: number }[] };
  const limbo = week.runs.find((r) => r.id === 'r-limbo')!;
  const moved = await page.request.post(`${ADMIN}/api/admin/runs/r-limbo/move`, {
    headers: await csrf(page.request),
    data: { day: limbo.day, time: '23:45', version: week.version },
  });
  expect(moved.ok()).toBe(true);

  const sent = page.waitForRequest((r) => r.method() === 'PATCH' && r.url().includes('/api/admin/fixed/'));
  await editor.getByRole('button', { name: 'Save changes' }).click();
  const patch = await sent;
  expect(patch.postDataJSON()).toMatchObject({ version: week.version, note: 'Bring potions' });
  const headers = await patch.allHeaders();
  expect(headers['x-kanade-csrf']).toBeTruthy();
  expect(headers['idempotency-key']).toMatch(/^[A-Za-z0-9._:-]{1,128}$/);
  await expect(toast(page, 'Saved Tuesday 23:30 — XBM.')).toBeVisible();

  // Another admin changes this timing's note while the form is open: the
  // form would resend the old note, so the save is refused, not a revert.
  await editButton.click();
  await expect(editor.getByLabel('Note')).toHaveValue('Bring potions');
  await editor.getByLabel('Note').fill('Bring snacks');
  await editXbmBehind(page, { note: 'Starts late' });
  await editor.getByRole('button', { name: 'Save changes' }).click();
  await expect(editor.getByRole('alert')).toContainText('The week changed since it was loaded. Close and reopen');
  await expect(editor.getByLabel('Note')).toHaveValue('Bring snacks');

  await editor.getByRole('button', { name: 'Cancel' }).click();
  // The 409 re-reads the list; reopen only once the other admin's note is on screen.
  await expect(page.getByRole('row', { name: /Black Mage/ })).toContainText('Starts late');
  await editButton.click();
  await expect(editor.getByLabel('Note')).toHaveValue('Starts late');
  await editor.getByLabel('Note').fill('Starts late; bring snacks');
  await editor.getByRole('button', { name: 'Save changes' }).click();
  await expect(toast(page, 'Saved Tuesday 23:30 — XBM.')).toBeVisible();
});

test('fixed: a 409 busy keeps the form valid to retry, without the out-of-date advice', async ({ page }) => {
  await go(page, '/fixed');
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  await editor.getByLabel('Note').fill('Bring potions');
  await page.route('**/api/admin/fixed/*', (route) =>
    route.request().method() === 'PATCH'
      ? route.fulfill({ status: 409, contentType: 'application/json', body: '{"error":"busy","message":"Another change landed at the same moment; try again."}' })
      : route.continue(),
  );
  await editor.getByRole('button', { name: 'Save changes' }).click();
  await expect(editor.getByRole('alert')).toHaveText('Another change landed at the same moment; try again.');
  await page.unroute('**/api/admin/fixed/*');
  await editor.getByRole('button', { name: 'Save changes' }).click();
  await expect(toast(page, 'Saved Tuesday 23:30 — XBM.')).toBeVisible();
});

test('admin writes: a refused CSRF token is refreshed once and the same action retried', async ({ page }) => {
  await go(page, '/fixed');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('8 weekly timings');
  // Stand-in for signing in again elsewhere: the token the page holds stops working.
  await page.request.post(`${ADMIN}/__mock/csrf/rotate`);
  const writes: { status: number; key: string | undefined }[] = [];
  page.on('response', async (response) => {
    const request = response.request();
    if (request.method() === 'DELETE') writes.push({ status: response.status(), key: (await request.allHeaders())['idempotency-key'] });
  });
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  await page.getByRole('complementary', { name: 'Weekly timing details' }).getByRole('button', { name: 'Retire…' }).click();
  await page.getByRole('button', { name: 'Retire timing' }).click();
  await expect(toast(page, 'Retired Tuesday 23:30 — XBM; 1 upcoming run cancelled.')).toBeVisible();
  expect(writes.map((w) => w.status)).toEqual([403, 200]);
  expect(writes[1]!.key).toBe(writes[0]!.key);
});

test('fixed: retiring names its consequence and cancels upcoming runs', async ({ page }) => {
  await go(page, '/fixed');
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  await page.getByRole('complementary', { name: 'Weekly timing details' }).getByRole('button', { name: 'Retire…' }).click();
  const confirm = page.getByRole('dialog', { name: 'Retire Tuesday 23:30 — XBM?' });
  await expect(confirm).toContainText('1 upcoming run is cancelled');
  await confirm.getByRole('button', { name: 'Keep it' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('8 weekly timings');
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  await page.getByRole('complementary', { name: 'Weekly timing details' }).getByRole('button', { name: 'Retire…' }).click();
  await page.getByRole('button', { name: 'Retire timing' }).click();
  await expect(toast(page, 'Retired Tuesday 23:30 — XBM; 1 upcoming run cancelled.')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('7 weekly timings');
});

test('fixed: wide editor is aligned beside the list, returns focus, and becomes a phone sheet', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/fixed');
  const row = page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' });
  await row.click();
  const pane = page.getByRole('complementary', { name: 'Weekly timing details' });
  await expect(pane).toBeVisible();
  const geometry = await page.locator('.fixed-list').evaluate((list) => {
    const pane = document.querySelector<HTMLElement>('.side-pane')!;
    const left = list.getBoundingClientRect();
    const right = pane.getBoundingClientRect();
    return { listRight: left.right, paneLeft: right.left, listTop: left.top, paneTop: right.top };
  });
  expect(geometry.paneLeft).toBeGreaterThanOrEqual(geometry.listRight);
  expect(Math.abs(geometry.paneTop - geometry.listTop)).toBeLessThanOrEqual(1);
  await pane.getByRole('button', { name: 'Close weekly timing details' }).click();
  await expect(row).toBeFocused();

  await page.setViewportSize({ width: 390, height: 844 });
  await row.click();
  const sheet = page.getByRole('dialog', { name: 'Tuesday 23:30 — XBM' });
  await expect(sheet).toBeVisible();
  await expect(sheet).toHaveCSS('height', '844px');
});

test('fixed: direct wide load gives the editor pane its own width and scroll owner', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  const pane = page.getByRole('complementary', { name: 'Weekly timing details' });
  await expect(pane).toBeVisible();
  const paneMetrics = await pane.evaluate((element) => {
    const filler = document.createElement('div');
    filler.style.height = '2000px';
    element.append(filler);
    element.scrollTop = 1;
    const style = getComputedStyle(element);
    const result = { width: element.getBoundingClientRect().width, overflowY: style.overflowY, scrollHeight: element.scrollHeight, clientHeight: element.clientHeight, scrollTop: element.scrollTop };
    filler.remove();
    return result;
  });
  expect(paneMetrics.width).toBeGreaterThanOrEqual(419);
  expect(paneMetrics.width).toBeLessThanOrEqual(421);
  expect(paneMetrics.overflowY).toBe('auto');
  expect(paneMetrics.scrollHeight).toBeGreaterThan(paneMetrics.clientHeight);
  expect(paneMetrics.scrollTop).toBe(1);
});

test('history: direct wide load keeps its timeline and change pane as aligned scroll-owning siblings', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/history?sw=off`);
  const pane = page.getByRole('complementary', { name: 'Change details' });
  await expect(pane).toBeVisible();
  const frame = await page.evaluate(() => {
    const body = document.querySelector<HTMLElement>('.history-window__body')!;
    const list = body.querySelector<HTMLElement>(':scope > .history-list-region')!;
    const pane = body.querySelector<HTMLElement>(':scope > .side-pane')!;
    const fill = (element: HTMLElement) => {
      const filler = document.createElement('div');
      filler.style.height = '2000px';
      element.append(filler);
      element.scrollTop = 1;
      const result = { overflow: getComputedStyle(element).overflowY, scrollTop: element.scrollTop, scrollHeight: element.scrollHeight, clientHeight: element.clientHeight };
      filler.remove();
      return result;
    };
    const left = list.getBoundingClientRect();
    const right = pane.getBoundingClientRect();
    return {
      direct: body.children.length === 2 && body.children[0] === list && body.children[1] === pane,
      listRight: left.right,
      paneLeft: right.left,
      listTop: left.top,
      paneTop: right.top,
      paneWidth: right.width,
      list: fill(list.querySelector<HTMLElement>('.history-list-region__scroll')!),
      pane: fill(pane),
      documentScroll: document.scrollingElement!.scrollHeight > innerHeight,
    };
  });
  expect(frame.direct).toBe(true);
  expect(frame.paneLeft).toBeGreaterThanOrEqual(frame.listRight - 2);
  expect(Math.abs(frame.paneTop - frame.listTop)).toBeLessThanOrEqual(3);
  expect(frame.paneWidth).toBeGreaterThanOrEqual(399);
  expect(frame.paneWidth).toBeLessThanOrEqual(401);
  expect(frame.list.overflow).toBe('auto');
  expect(frame.pane.overflow).toBe('auto');
  expect(frame.list.scrollHeight).toBeGreaterThan(frame.list.clientHeight);
  expect(frame.pane.scrollHeight).toBeGreaterThan(frame.pane.clientHeight);
  expect(frame.list.scrollTop).toBe(1);
  expect(frame.pane.scrollTop).toBe(1);
  expect(frame.documentScroll).toBe(false);
});

test('history: wide Close and Escape leave the detail closed and return to its row', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/history');
  const pane = page.getByRole('complementary', { name: 'Change details' });
  const newest = page.locator('[data-history="9"]');
  await pane.getByRole('button', { name: 'Close change details' }).click();
  await expect(pane).toBeHidden();
  await expect(newest).toBeFocused();
  await page.waitForTimeout(100);
  await expect(pane).toBeHidden();

  const row = page.locator('[data-history="2"]');
  await row.click();
  await expect(pane).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(pane).toBeHidden();
  await expect(row).toBeFocused();
  await page.waitForTimeout(100);
  await expect(pane).toBeHidden();
});

test('history: restoring a multi-week record uses the week group it was opened from', async ({ page }) => {
  const secondWeek = '2026-10-07T16:00:00+00:00';
  await page.route('**/api/admin/history?*', async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    const record = body.records.find((item: { seq: number }) => item.seq === 3);
    record.weeks = [...record.weeks, secondWeek];
    await route.fulfill({ response, json: body });
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/history');
  const row = page.locator(`[data-history="3"][data-history-week="${secondWeek}"]`);
  await row.click();
  const request = page.waitForRequest((candidate) => candidate.method() === 'POST' && candidate.url().endsWith('/api/admin/history/restore-week'));
  await page.getByRole('complementary', { name: 'Change details' }).getByRole('button', { name: 'Restore week to here…' }).click();
  expect((await request).postDataJSON()).toMatchObject({ week: secondWeek });
});

test('history: a change listed under two weeks marks only the opened row active', async ({ page }) => {
  const secondWeek = '2026-10-07T16:00:00+00:00';
  await page.route('**/api/admin/history?*', async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    const record = body.records.find((item: { seq: number }) => item.seq === 3);
    record.weeks = [...record.weeks, secondWeek];
    await route.fulfill({ response, json: body });
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/history');
  await page.locator(`[data-history="3"][data-history-week="${secondWeek}"]`).click();
  await expect(page.locator('[data-history="3"]')).toHaveCount(2);
  await expect(page.locator('.history-row[aria-current="true"]')).toHaveCount(1);
  await expect(page.locator('.history-row--active')).toHaveCount(1);
  await expect(page.locator(`[data-history="3"][data-history-week="${secondWeek}"]`)).toHaveAttribute('aria-current', 'true');
});

for (const [width, height] of [
  [1280, 800],
  [390, 844],
] as const) {
  test(`history: opening a lower row never scrolls the fixed shell (${width}x${height})`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await go(page, '/history');
    const rows = page.locator('.history-row');
    await expect(rows.first()).toBeVisible();
    // Grow the timeline so the last row sits below the fold, then open it.
    await page.evaluate(() => {
      const list = document.querySelector<HTMLElement>('.history-list-region__scroll')!;
      const pad = document.createElement('div');
      pad.className = 'e2e-pad';
      pad.setAttribute('aria-hidden', 'true');
      pad.style.height = '1500px';
      list.querySelector('.history__week')!.before(pad);
    });
    await rows.last().click();
    await page.waitForTimeout(150);
    const scrolls = await page.evaluate(() =>
      ['html', 'body', '.frame', '.shell', '.window-fill', '.history-window__body'].map((selector) => {
        const element = selector === 'html' ? document.scrollingElement! : document.querySelector<HTMLElement>(selector)!;
        return [selector, element.scrollTop];
      }),
    );
    expect(Object.fromEntries(scrolls)).toEqual({ html: 0, body: 0, '.frame': 0, '.shell': 0, '.window-fill': 0, '.history-window__body': 0 });
    await expect(page.locator('.pageline, .page-head').first()).toBeInViewport();
  });
}

test('history: rows read as field diffs and the raw JSON opens in a viewer', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/history');
  await page.locator('[data-history="2"]').first().click();
  const pane = page.getByRole('complementary', { name: 'Change details' });
  await expect(pane.locator('.history-diff__field').first()).toBeVisible();
  await expect(pane.locator('pre')).toHaveCount(0);
  const trigger = pane.getByRole('button', { name: 'Show raw JSON' });
  await trigger.click();
  const viewer = page.getByRole('dialog', { name: 'Change #2 raw JSON' });
  await expect(viewer).toContainText('"before"');
  await page.keyboard.press('Escape');
  await expect(viewer).toBeHidden();
  await expect(pane).toBeVisible();
  await expect(trigger).toBeFocused();
});

test('history: phone detail is a sheet, Escape closes the topmost dialog and restores focus', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/history');
  const row = page.locator('[data-history="2"]');
  await row.click();
  const detail = page.getByRole('dialog', { name: 'Change #2' });
  await expect(detail).toBeVisible();
  await expect(detail).toHaveCSS('height', '844px');
  await detail.getByRole('button', { name: 'Revert…' }).click();
  const confirm = page.getByRole('dialog', { name: 'Revert #2?' });
  await expect(confirm).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(confirm).toBeHidden();
  await expect(detail).toBeVisible();
  await expect(detail.getByRole('button', { name: 'Revert…' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(detail).toBeHidden();
  await expect(row).toBeFocused();
});

test('fixed: Add and Retire restore focus, while Escape leaves the editor behind its confirmation', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/fixed');
  const add = page.getByRole('button', { name: 'Add a weekly timing' });
  await add.click();
  await page.getByRole('complementary', { name: 'Weekly timing details' }).getByRole('button', { name: 'Close weekly timing details' }).click();
  await expect(add).toBeFocused();
  await add.click();
  await page.keyboard.press('Escape');
  await expect(add).toBeFocused();

  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  const pane = page.getByRole('complementary', { name: 'Weekly timing details' });
  await pane.getByRole('button', { name: 'Retire…' }).click();
  const confirm = page.getByRole('dialog', { name: 'Retire Tuesday 23:30 — XBM?' });
  await page.keyboard.press('Escape');
  await expect(confirm).toBeHidden();
  await expect(pane).toBeVisible();

  await pane.getByRole('button', { name: 'Retire…' }).click();
  await confirm.getByRole('button', { name: 'Retire timing' }).click();
  await expect(pane).toBeHidden();
  await expect(add).toBeFocused();
});

test('run sheet: this-week roster line and reset to fixed', async ({ page }) => {
  await go(page, '/');
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await expect(sheet.getByText('this week: +Ren')).toBeVisible();
  // An amended run shows every action (Move, Swap, Preview ping, Reset to
  // fixed); they sit on their own row under the details instead of squeezing them.
  const details = await sheet.locator('.run__bosses').boundingBox();
  const actions = await sheet.locator('.run__actions').boundingBox();
  expect(details!.width).toBeGreaterThan(400);
  expect(actions!.y).toBeGreaterThan(details!.y);
  await sheet.getByRole('button', { name: 'Reset to fixed' }).click();
  await expect(sheet.locator('.sheet__notice')).toContainText('HCarling + HStar is back on its weekly timing.');
  await expect(sheet.getByText(/this week:/)).toHaveCount(0);
  await expect(sheet.getByRole('button', { name: 'Reset to fixed' })).toHaveCount(0);
  await expect(sheet.locator('.run__people .chip', { hasText: 'Ren' })).toHaveCount(1);
  await expect(sheet.getByRole('button', { name: 'Reset to fixed' })).toHaveCount(0);

  await sheet.getByRole('link', { name: /^Cards/ }).click();
  await expect(page).toHaveURL(`${ADMIN}/reminders?run=r-carling`);
});

test('bosses: the in-game list, ticked by timings, with knowledge pages', async ({ page }) => {
  await go(page, '/bosses');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(/^11 bosses, \d+ difficulties$/);
  const star = page.locator('.bossrow', { hasText: 'Radiant Malefic Star' });
  await expect(star.locator('.pill-toggle--on')).toHaveText(/HARD/);
  await expect(star.getByText('(has a weekly timing)')).toHaveCount(1);
  await page.getByRole('link', { name: 'Radiant Malefic Star' }).click();
  await expect(page).toHaveURL(`${ADMIN}/bosses/MaleficStar/knowledge`);
  await expect(page.getByRole('heading', { level: 1 })).toContainText('Radiant Malefic Star');
  await expect(page.getByText('boss/knowledge/maleficstar.yaml')).toBeVisible();
  await page.goto(`${ADMIN}/bosses/Nobody/knowledge?sw=off`);
  await expect(page.getByRole('alert')).toContainText('No knowledge for “Nobody”');
});

test('members: roster side pane edits for pings, reply style and aliases', async ({ page }) => {
  await go(page, '/members');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 bossers');
  // Two members share "Ren": told apart by place, never by id.
  await expect(page.getByRole('button', { name: /^Ren \(2\)/ })).toBeVisible();
  await expect(page.getByRole('list', { name: 'Members' })).not.toContainText('1013');
  await expect(page.getByRole('button', { name: /^Kohane/ })).toContainText('chat only');
  await page.getByRole('searchbox', { name: 'Search members' }).fill('tsu');
  await expect(page.getByRole('list', { name: 'Members' }).getByRole('button')).toHaveCount(1);
  await page.getByRole('button', { name: /^Tsubame/ }).click();

  const sheet = page.getByRole('complementary', { name: 'Member details' });
  await expect(sheet).toBeVisible();
  await expect(sheet.getByRole('button', { name: 'Off' })).toHaveAttribute('aria-pressed', 'true');
  await sheet.getByRole('button', { name: 'Essential' }).click();
  // The sheet's notice, not the name's copy status.
  const notice = sheet.locator('[role="status"]:not(.vh)');
  await expect(notice).toHaveText('Pings set to essential.');
  await sheet.getByLabel('Reply style').selectOption({ label: 'Terse' });
  await expect(notice).toHaveText('Reply style set to Terse.');
  await sheet.getByRole('textbox', { name: 'New alias for Tsubame' }).fill('swallow');
  await sheet.getByRole('button', { name: 'Add' }).click();
  await expect(sheet.locator('.membersheet__aliases')).toContainText('swallow');
  await sheet.getByRole('textbox', { name: 'New alias for Tsubame' }).fill('mika');
  await sheet.getByRole('button', { name: 'Add' }).click();
  await expect(notice).toHaveText('“mika” already names someone.');
  await expect(sheet.getByRole('textbox', { name: 'New alias for Tsubame' })).toHaveValue('mika');

  await page.keyboard.press('Escape');
  await expect(sheet).toBeHidden();
  await expect(page.getByRole('button', { name: /^Tsubame/ })).toBeFocused();
  await page.getByRole('searchbox', { name: 'Search members' }).fill('');
  await expect(page.getByRole('button', { name: /^Rin/ })).toBeVisible();
  await page.getByRole('button', { name: /^Rin/ }).click();
  await expect(page.getByRole('complementary', { name: 'Member details' }).getByText('is no longer offered')).toBeVisible();
});

test('members: phone uses a full-screen sheet and restores the roster row', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/members');
  const row = page.getByRole('button', { name: /^Asahi/ });
  await row.click();
  const sheet = page.getByRole('dialog', { name: 'Asahi' });
  await expect(sheet).toBeVisible();
  await expect(sheet).toHaveCSS('height', '844px');
  await page.keyboard.press('Escape');
  await expect(sheet).toBeHidden();
  await expect(row).toBeFocused();
});

test('members: fixed frame keeps roster and wide detail as the only scroll owners', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/members');
  await page.getByRole('button', { name: /^Asahi/ }).click();
  await expect(page.getByRole('complementary', { name: 'Member details' })).toBeVisible();
  const frame = await page.evaluate(() => {
    const list = document.querySelector<HTMLElement>('.memberlist')!;
    const pane = document.querySelector<HTMLElement>('.side-pane')!;
    const roster = document.querySelector<HTMLElement>('.members-roster')!;
    const pager = roster.querySelector<HTMLElement>('.members-roster__pager')!;
    const listBox = list.getBoundingClientRect();
    const paneBox = pane.getBoundingClientRect();
    const rosterBox = roster.getBoundingClientRect();
    const pagerBox = pager.getBoundingClientRect();
    return {
      documentScroll: document.scrollingElement!.scrollHeight > innerHeight,
      listOverflow: getComputedStyle(list).overflowY,
      paneOverflow: getComputedStyle(pane).overflowY,
      windowHeight: document.querySelector<HTMLElement>('.members-window')!.getBoundingClientRect().height,
      paneLeft: paneBox.left,
      rosterRight: rosterBox.right,
      paneTop: paneBox.top,
      rosterTop: rosterBox.top,
      pagerTop: pagerBox.top,
      listBottom: listBox.bottom,
    };
  });
  expect(frame.documentScroll).toBe(false);
  expect(frame.listOverflow).toBe('auto');
  expect(frame.paneOverflow).toBe('auto');
  expect(frame.windowHeight).toBeGreaterThan(400);
  expect(frame.paneLeft).toBeGreaterThanOrEqual(frame.rosterRight - 2);
  expect(Math.abs(frame.paneTop - frame.rosterTop)).toBeLessThanOrEqual(3);
  expect(frame.pagerTop).toBeGreaterThanOrEqual(frame.listBottom - 2);
});

test('reminders: queued, due, sent and stale, all runs or one', async ({ page }) => {
  await go(page, '/reminders');
  const queued = page.getByRole('table', { name: 'Queued reminders' });
  const sent = page.getByRole('table', { name: 'Sent reminders' });
  await expect(queued.getByRole('row').nth(1)).toContainText('Tue 29 Sep 21:00');
  // Sent is paged 15 at a time; the stale card is on a later page.
  const older = page.getByRole('button', { name: 'Older →' });
  while ((await sent.getByText('stale — retired without posting').count()) === 0 && (await older.isEnabled())) await older.click();
  await expect(sent.getByText('stale — retired without posting')).toBeVisible();
  await expect(sent.getByRole('link', { name: 'open in Discord' }).first()).toBeVisible();
  await page.getByRole('searchbox', { name: 'Search reminders' }).fill('xbm');
  await expect(queued.locator('tbody tr')).not.toHaveCount(0);
  for (const row of await queued.locator('tbody tr').all()) await expect(row).toContainText('XBM');
  await page.getByRole('searchbox', { name: 'Search reminders' }).fill('');
  await queued.getByRole('link', { name: '#630b3544' }).first().click();
  await expect(page).toHaveURL(`${ADMIN}/reminders?run=r-carling`);
  await expect(page.getByText('run #630b3544')).toBeVisible();
  for (const row of await queued.locator('tbody tr').all()) await expect(row).toContainText('#630b3544');
  await page.getByRole('link', { name: 'Show every run' }).click();
  await expect(page).toHaveURL(`${ADMIN}/reminders`);
});
