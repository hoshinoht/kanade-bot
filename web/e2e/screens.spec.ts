import type { Page } from '@playwright/test';
import { ADMIN, csrf, expect, test, choose, expectValue, optionLabels, unconditional } from './support';

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
  // The weekday strip runs in boss-week order from the reset day.
  const days = editor.getByRole('radiogroup', { name: 'Day' }).getByRole('radio');
  await expect(days.first()).toHaveAccessibleName('Thursday');
  await editor.getByRole('radio', { name: 'Wednesday' }).click();
  await expect(editor.getByRole('radio', { name: 'Wednesday' })).toHaveAttribute('aria-checked', 'true');
  await editor.getByLabel('Time').fill('20:30');
  await choose(editor.getByLabel('Home channel'), { label: '#limbo-trio' });
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
  // The first save's toast may still be showing, so wait for this save's reply.
  const saved = page.waitForResponse((r) => r.request().method() === 'PATCH' && r.url().includes('/api/admin/fixed/'));
  await editor.getByRole('button', { name: 'Save changes' }).click();
  expect((await saved).ok()).toBe(true);
  await expect(toast(page, 'Saved Tuesday 23:30 — XBM.').last()).toBeVisible();
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

// User decision 2026-10-09: the first in the party owns a timing unless an
// owner is picked (pinned); "Default" sends "" and unpins.
test('fixed: the owner defaults to the first in the party, a pick pins it, Default unpins', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/fixed');
  const editButton = page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' });
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  await editButton.click();
  const owner = editor.getByLabel('Owner');
  await expect(owner.locator('.dd__value')).toHaveText('Default: first in party (Minato)');
  // Only the roster is offered (Kohane has chatbot access but no bossing role).
  expect((await optionLabels(owner)).filter((l) => l.includes('Kohane'))).toEqual([]);
  // The weekday strip takes its own line (P_MoveStates "Reuse"); Owner sits on the Time line.
  const tops = await editor.locator('.fixedsheet__fields > .field').evaluateAll((fields) => fields.map((f) => Math.round(f.getBoundingClientRect().top)));
  expect(tops).toHaveLength(3);
  expect(tops[0]).toBeLessThan(tops[1]!);
  expect(tops[1]).toBe(tops[2]);
  await expect(editor.getByRole('radio', { name: 'Tuesday' })).toHaveAttribute('aria-checked', 'true');

  await choose(owner, { label: 'Kaito' });
  let sent = page.waitForRequest((r) => r.method() === 'PATCH' && r.url().includes('/api/admin/fixed/'));
  await editor.getByRole('button', { name: 'Save changes' }).click();
  expect((await sent).postDataJSON()).toMatchObject({ owner_id: '1009' });
  await expect(toast(page, 'Saved Tuesday 23:30 — XBM.')).toBeVisible();
  const xbm = async () => ((await (await page.request.get(`${ADMIN}/api/admin/fixed`)).json()) as { owner_id: string; owner: string; owner_pinned: boolean; bosses: { token: string }[] }[]).find((r) => r.bosses.some((b) => b.token === 'XBM'));
  expect(await xbm()).toMatchObject({ owner_id: '1009', owner: 'Kaito', owner_pinned: true });
  // The list's search reads the owner: Kaito now finds XBM.
  await page.getByRole('searchbox', { name: 'Search weekly timings' }).fill('kaito');
  await expect(page.getByRole('row', { name: /Black Mage/ })).toBeVisible();
  await page.getByRole('searchbox', { name: 'Search weekly timings' }).fill('');
  await editButton.click();
  await expectValue(owner, '1009');

  await choose(owner, { label: /^Default: first in party/ });
  sent = page.waitForRequest((r) => r.method() === 'PATCH' && r.url().includes('/api/admin/fixed/'));
  await editor.getByRole('button', { name: 'Save changes' }).click();
  expect((await sent).postDataJSON()).toMatchObject({ owner_id: '' });
  await expect(toast(page, 'Saved Tuesday 23:30 — XBM.')).toBeVisible();
  expect(await xbm()).toMatchObject({ owner_id: '1012', owner: 'Minato', owner_pinned: false });
});

test('fixed: a new timing defaults to the first party member; a picked owner stays put', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/fixed');
  await page.getByRole('button', { name: 'Add a weekly timing' }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  const owner = editor.getByLabel('Owner');
  // Even signed in with Discord, the admin does not own what they create.
  await expect(owner.locator('.dd__value')).toHaveText('Default: first in party');
  await editor.getByRole('checkbox', { name: 'Mika' }).check();
  await editor.getByRole('checkbox', { name: 'Nagi' }).check();
  await expect(owner.locator('.dd__value')).toHaveText('Default: first in party (Mika)');
  // Picked by hand, it stays put while the party changes.
  await choose(owner, { label: 'Yuzu' });
  await editor.getByRole('checkbox', { name: 'Mika' }).uncheck();
  await expectValue(owner, '1004');
  await editor.locator('.bossrow', { hasText: 'Limbo' }).locator('label', { hasText: 'HARD' }).click();
  await editor.getByLabel('Time').fill('19:15');
  const sent = page.waitForRequest((r) => r.method() === 'POST' && r.url().endsWith('/api/admin/fixed'));
  await editor.getByRole('button', { name: 'Add timing' }).click();
  expect((await sent).postDataJSON()).toMatchObject({ owner_id: '1004', participants: ['1007'] });
  await expect(toast(page, /Added .* 19:15 — HLimbo/)).toBeVisible();
});

test('fixed: an owner refusal (422) reads out on the Owner field and keeps the form', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/fixed');
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  await choose(editor.getByLabel('Owner'), { label: 'Rin' });
  await page.route('**/api/admin/fixed/*', (route) =>
    route.request().method() === 'PATCH'
      ? route.fulfill({ status: 422, contentType: 'application/json', body: '{"error":"invalid","message":"Pick an owner from the roster."}' })
      : route.continue(),
  );
  await editor.getByRole('button', { name: 'Save changes' }).click();
  const owner = editor.getByLabel('Owner');
  await expect(owner).toHaveAttribute('aria-invalid', 'true');
  await expect(owner).toHaveAccessibleDescription('Pick an owner from the roster.');
  await expect(owner).toBeFocused();
  await expectValue(owner, '1010');
  await page.unroute('**/api/admin/fixed/*');
  // Picking again clears the refusal; the save goes through.
  await choose(owner, { label: 'Kaito' });
  await expect(owner).not.toHaveAttribute('aria-invalid', 'true');
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
  // The editor's fields scroll inside the pane; its head and foot (Retire,
  // Save) stay put and never sit over a field.
  const paneMetrics = await pane.evaluate((element) => {
    const fields = element.querySelector<HTMLElement>('.fixedsheet__content')!;
    const filler = document.createElement('div');
    filler.style.height = '2000px';
    fields.append(filler);
    fields.scrollTop = 1;
    const style = getComputedStyle(fields);
    const foot = element.querySelector('.fixedsheet__foot')!.getBoundingClientRect();
    const box = element.getBoundingClientRect();
    const result = {
      width: box.width,
      overflowY: style.overflowY,
      scrollHeight: fields.scrollHeight,
      clientHeight: fields.clientHeight,
      scrollTop: fields.scrollTop,
      footInside: foot.bottom <= box.bottom + 0.5,
      paneScrolls: element.scrollHeight > element.clientHeight,
    };
    filler.remove();
    return result;
  });
  expect(paneMetrics.width).toBeGreaterThanOrEqual(419);
  expect(paneMetrics.width).toBeLessThanOrEqual(421);
  expect(paneMetrics.overflowY).toBe('auto');
  expect(paneMetrics.scrollHeight).toBeGreaterThan(paneMetrics.clientHeight);
  expect(paneMetrics.scrollTop).toBe(1);
  expect(paneMetrics.footInside).toBe(true);
  expect(paneMetrics.paneScrolls).toBe(false);
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
      filler.style.flexShrink = '0';
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
    const response = await route.fetch(unconditional(route));
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
    const response = await route.fetch(unconditional(route));
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
  const sheet = page.getByRole('complementary', { name: 'HCarling + HStar' });
  // B_WeekSel: one line, "This week: +Ren · cards …".
  await expect(sheet.locator('.week-pane__week')).toContainText(/^\s*This week: \+Ren\s*·\s*cards\s+morning/);
  // An amended run shows every action (Swap, Preview ping, Reset to fixed) on one row of pills.
  const tops = await sheet.locator('.week-pane__actions .btn').evaluateAll((els) => els.map((el) => Math.round(el.getBoundingClientRect().top)));
  expect(tops).toHaveLength(3);
  expect(new Set(tops).size).toBe(1);
  await sheet.getByRole('button', { name: 'Reset to fixed' }).click();
  await expect(sheet.locator('.sheet__notice')).toContainText('HCarling + HStar is back on its weekly timing.');
  await expect(sheet.getByText(/this week:/i)).toHaveCount(0);
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
  await expect(star.locator('.row-content__compact .boss-tick--h')).toContainText('HARD');
  await expect(star.locator('.row-content__compact .boss-tick--more')).toContainText(/^\+\d+/);
  await page.getByRole('link', { name: 'Radiant Malefic Star' }).click();
  await expect(page).toHaveURL(`${ADMIN}/bosses/MaleficStar/knowledge`);
  await expect(page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' })).toBeVisible();
  await expect(page.getByText('boss/knowledge/MaleficStar.yaml')).toBeVisible();
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
  await choose(sheet.getByLabel('Reply style'), { label: 'Terse' });
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

test('members: an alias chip’s × removes it from the sheet and the roster, by keyboard, focus following', async ({ page }) => {
  await go(page, '/members');
  const row = page.locator('[data-member="1003"]');
  await expect(row).toContainText('mika · mk');
  await row.click();
  const sheet = page.getByRole('complementary', { name: 'Member details' });
  const aliases = sheet.locator('.membersheet__aliases');
  const notice = sheet.locator('[role="status"]:not(.vh)');
  const first = sheet.getByRole('button', { name: 'Remove alias mika from Mika' });
  const second = sheet.getByRole('button', { name: 'Remove alias mk from Mika' });
  const input = sheet.getByRole('textbox', { name: 'New alias for Mika' });

  // Reachable in tab order: the add field is one Shift+Tab past the last ×.
  await input.focus();
  await page.keyboard.press('Shift+Tab');
  await expect(second).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(first).toBeFocused();
  const box = (await first.boundingBox())!;
  expect(box.width).toBeGreaterThanOrEqual(24);
  expect(box.height).toBeGreaterThanOrEqual(24);

  await page.keyboard.press('Enter');
  await expect(notice).toHaveText('Alias “mika” removed.');
  await expect(aliases).not.toContainText('mika');
  await expect(row).toContainText('mk');
  await expect(row).not.toContainText('mika ·');
  // Focus moves to the chip that took its place.
  await expect(second).toBeFocused();

  await page.keyboard.press('Space');
  await expect(notice).toHaveText('Alias “mk” removed.');
  await expect(aliases).toHaveText('none');
  await expect(row).not.toContainText('mk');
  // The last one gone: focus goes to the add field.
  await expect(input).toBeFocused();
});

test('members: "Sort: runs" orders by runs this week, A–Z between equals, and switches to A–Z', async ({ page }) => {
  await go(page, '/members');
  const sort = page.getByRole('combobox', { name: 'Sort members' });
  await expect(page.locator('.members-window__sort')).toHaveText(/^\s*Sort\s*runs\s*$/);
  await expectValue(sort, 'runs');
  await expect(sort.locator('.dd__value')).toHaveText('runs');
  const list = page.getByRole('list', { name: 'Members' });
  const roster = () =>
    list.locator('.memberlist__row').evaluateAll((rows) =>
      rows.map((row) => ({ name: row.querySelector('.memberlist__name strong')!.textContent!, runs: Number(row.querySelector('.memberlist__stat')!.lastChild!.textContent) })),
    );
  const byRuns = await roster();
  expect(byRuns.length).toBeGreaterThan(5);
  expect(byRuns[0]).toEqual({ name: 'Asahi', runs: 4 });
  for (let i = 1; i < byRuns.length; i++) {
    const [a, b] = [byRuns[i - 1]!, byRuns[i]!];
    expect(a.runs > b.runs || (a.runs === b.runs && a.name.localeCompare(b.name) <= 0), `${a.name} before ${b.name}`).toBe(true);
  }
  expect(byRuns.at(-1)!.runs).toBe(0);

  await choose(sort, 'name');
  await expect(sort.locator('.dd__value')).toHaveText('A–Z');
  const names = (await roster()).map((r) => r.name);
  expect(names).toEqual(names.toSorted((a, b) => a.localeCompare(b)));
  expect(names).not.toEqual(byRuns.map((r) => r.name));
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
  const rows = queued.locator('tbody tr:has(td)');
  await expect(rows.first()).toContainText('Tue 29 Sep 21:00');
  // Sent and stale cards each have their own tab.
  await page.getByRole('tab', { name: /^Sent/ }).click();
  await expect(page.getByRole('table', { name: 'Sent reminders' }).getByRole('link', { name: /open in Discord/ }).first()).toBeVisible();
  await page.getByRole('tab', { name: /^Stale & other/ }).click();
  await expect(page.getByRole('table', { name: 'Stale and other reminders' }).getByText('stale — retired without posting').first()).toBeAttached();
  await page.getByRole('tab', { name: /^Queued/ }).click();
  await page.getByRole('searchbox', { name: 'Search reminders' }).fill('xbm');
  await expect(rows).not.toHaveCount(0);
  for (const row of await rows.all()) await expect(row).toContainText('XBM');
  await page.getByRole('searchbox', { name: 'Search reminders' }).fill('');
  await queued.getByRole('link', { name: '#630b3544' }).first().click();
  await expect(page).toHaveURL(`${ADMIN}/reminders?run=r-carling`);
  await expect(page.getByText('run #630b3544')).toBeVisible();
  for (const row of await rows.all()) await expect(row).toContainText('#630b3544');
  await page.getByRole('link', { name: 'Show every run' }).click();
  await expect(page).toHaveURL(`${ADMIN}/reminders`);
});

// User feedback 2026-10-02: a hovered row took the pane's colour and seemed
// to vanish. The row state layer must differ from the resting row, the pane
// and the selected fill (and leave the selected row as it is).
for (const [path, rows, pane, active] of [
  ['/history', '.history-row', '.history-list-region', '.history-row--active'],
  ['/members', '.memberlist__row', '.memberlist', '.memberlist__row--active'],
] as const) {
  test(`${path}: a hovered row stands apart from the row, the pane and the selection`, async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem('colorway', 'blossom');
      localStorage.setItem('theme', 'light');
    });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${ADMIN}${path}?sw=off`);
    await page.locator(rows).first().click();
    const selected = page.locator(active);
    await expect(selected).toHaveCount(1);
    const bg = (selector: string, nth = 0) => page.locator(selector).nth(nth).evaluate((el) => getComputedStyle(el).backgroundColor);
    const target = page.locator(`${rows}:not(${active})`).nth(1);
    const resting = await target.evaluate((el) => getComputedStyle(el).backgroundColor);
    const selectedBg = await bg(active);
    const paneBg = await bg(pane);
    await target.hover();
    await expect.poll(() => target.evaluate((el) => getComputedStyle(el).backgroundColor)).not.toBe(resting);
    const hovered = await target.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(hovered).not.toBe(paneBg);
    expect(hovered).not.toBe(selectedBg);
    // Hovering the selected row keeps its selected fill.
    await selected.hover();
    await expect.poll(() => bg(active)).toBe(selectedBg);
  });
}

// Fidelity (B_Fixed): the whole row is the card -- one grid row whose own
// fill shows selection and hover, its cells drawing none -- with one button
// per row; flags in their own case; Add has its plus.
test('fixed: whole-row selection and hover, quiet flags, the Add key with its plus', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('colorway', 'blossom');
    localStorage.setItem('theme', 'light');
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await expect(page.locator('[data-fixed-add] svg[data-icon="plus"]')).toHaveCount(1);
  const rows = page.locator('.fixed-list tbody tr');
  await expect(rows.first()).toBeVisible();
  // Still a table to assistive tech, laid out as grid rows (B_Fixed).
  await expect(page.getByRole('table', { name: 'Weekly timings, by weekday' })).toBeVisible();
  await expect(rows.first()).toHaveCSS('display', 'grid');
  // One control per row.
  await expect(rows.first().getByRole('button', { name: /^Edit / })).toHaveCount(1);
  const rowBg = (row: ReturnType<typeof rows.nth>) => row.evaluate((el) => getComputedStyle(el).backgroundColor);
  const cellBgs = (row: ReturnType<typeof rows.nth>) => row.locator(':scope > *').evaluateAll((cells) => cells.map((c) => getComputedStyle(c).backgroundColor));
  const resting = await rowBg(rows.nth(2));
  // A click past the party (not on a name), on the row's own edge, opens the row: the hit area spans it.
  const party = rows.nth(0).locator('.fixed-list__party');
  const box = (await party.boundingBox())!;
  await page.mouse.click(box.x + box.width + 6, box.y + box.height / 2);
  await expect(page.getByRole('complementary', { name: 'Weekly timing details' })).toBeVisible();
  const selected = await rowBg(rows.nth(0));
  expect(selected).not.toBe(resting);
  expect(new Set(await cellBgs(rows.nth(0)))).toEqual(new Set(['rgba(0, 0, 0, 0)']));
  await rows.nth(2).locator('td').first().hover({ position: { x: 40, y: 30 } });
  await expect.poll(() => rowBg(rows.nth(2))).not.toBe(resting);
  const hovered = await rowBg(rows.nth(2));
  expect(hovered).not.toBe(selected);
  expect(new Set(await cellBgs(rows.nth(2)))).toEqual(new Set(['rgba(0, 0, 0, 0)']));
  // Hovering the selected row keeps its selected fill (review finding 1), and
  // its day and time are bold -- the non-colour cue (finding 5).
  await rows.nth(0).locator('td').first().hover({ position: { x: 40, y: 20 } });
  await expect.poll(() => rowBg(rows.nth(0))).toBe(selected);
  await expect(rows.nth(0).locator('.fixed-list__day')).toHaveCSS('font-weight', '700');
  await expect(page.locator('.fixed-list .status').first()).toHaveCSS('text-transform', 'none');
});

// Fidelity (B_History, P_SelectSpec): "WEEK every week" in one title-bar dropdown; no "History" header row in the change pane.
test('history: filter dropdowns read label and value, and the change pane has no extra header row', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/history?sw=off`);
  const week = page.getByRole('combobox', { name: 'Week' });
  await expect(week.locator('.dd__label')).toHaveText('Week');
  await expect(week.locator('.dd__value')).toHaveText('every week');
  await expect(week).toHaveCSS('height', '32px');
  await page.locator('.history-row').nth(1).click();
  const pane = page.getByRole('complementary', { name: 'Change details' });
  await expect(pane).toBeVisible();
  await expect(pane.locator('.cap').first()).toContainText(/^Change #/);
  await expect(pane.getByText('History', { exact: true })).toHaveCount(0);
  await expect(pane.getByRole('button', { name: 'Close change details' })).toBeVisible();
});

// Fidelity (B_InboxSelf): mono overlines for "Would change" and the conflict, a tonal source chip.
test('inbox: overline section labels and a tonal source chip', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/inbox?tab=self_service&item=p-fa-request&sw=off`);
  const detail = page.locator('.inbox__detail');
  for (const name of [/^Would change/, /^Changed since the member asked$/]) {
    const heading = detail.getByRole('heading', { name });
    await expect(heading).toHaveCSS('text-transform', 'uppercase');
    expect(await heading.evaluate((el) => getComputedStyle(el).fontFamily)).toMatch(/mono/i);
  }
  const chip = detail.locator('.proposal__head .chip', { hasText: 'Member request' });
  await expect(chip).toHaveCSS('border-top-color', 'rgba(0, 0, 0, 0)');
  expect(await chip.evaluate((el) => getComputedStyle(el).backgroundColor)).not.toBe('rgba(0, 0, 0, 0)');
});

// B_Fixed picker: each boss's difficulties sit on one line in a compact row;
// an existing timing shows its own bosses until "All n bosses…".
test('fixed editor: one line of difficulty pills per boss, own bosses first', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: /^Edit Wednesday/ }).first().click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  const rows = editor.locator('.bossrow');
  await expect(rows).toHaveCount(1);
  await editor.getByRole('button', { name: /^All \d+ bosses…$/ }).click();
  expect(await rows.count()).toBeGreaterThan(1);
  for (const row of (await rows.all()).slice(0, 6)) {
    const tops = await row.locator('.pill-toggle').evaluateAll((pills) => pills.map((p) => Math.round(p.getBoundingClientRect().top)));
    expect(new Set(tops).size, await row.innerText()).toBe(1);
    expect((await row.boundingBox())!.height).toBeLessThanOrEqual(48);
  }
  // The party: the picked members and the first few others, then "+n" for the
  // rest; pressing it shows them and moves focus to the first one it showed.
  const more = editor.getByRole('button', { name: /^Show \d+ more members?$/ });
  await expect(more).toHaveText(/^\+\d+$/);
  const hidden = Number((await more.textContent())!.slice(1));
  const people = editor.locator('.run__people input[type="checkbox"]');
  const values = () => people.evaluateAll((inputs) => inputs.map((input) => (input as HTMLInputElement).value));
  const before = await values();
  await more.click();
  await expect(more).toHaveCount(0);
  await expect(people).toHaveCount(before.length + hidden);
  const first = (await values()).find((value) => !before.includes(value))!;
  await expect(editor.locator(`.run__people input[value="${first}"]`)).toBeFocused();
});

// B_InboxSelf / B_PhoneInbox: the boss art beside the title; the thread's
// rows with no stray "open" links (the time opens the message instead).
test('inbox: boss art, and a thread without stray open links', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/inbox?tab=extractor&item=p-bm-move&sw=off`);
  const detail = page.locator('.inbox__detail');
  await expect(detail.locator('.proposal__head .proposal__art .portrait')).toHaveCount(1);
  const thread = detail.getByLabel('Evidence');
  await expect(thread.getByRole('link', { name: 'open', exact: true })).toHaveCount(0);
  await expect(thread.getByRole('link', { name: /open in Discord/ }).first()).toBeVisible();
  await expect(thread.locator('.msg--used').first()).toBeVisible();
  await expect(detail.getByRole('heading', { name: /^Thread · \d+ messages?$/ })).toBeVisible();
  // The decision pane runs the detail's height, Reject at its foot.
  const decision = detail.getByRole('complementary', { name: 'Decide this change' });
  const reject = (await decision.getByRole('button', { name: 'Reject…' }).boundingBox())!;
  const box = (await decision.boundingBox())!;
  expect(box.y + box.height - (reject.y + reject.height)).toBeLessThan(80);

  await page.setViewportSize({ width: 390, height: 844 });
  // The phone frame takes over after the resize: wait for the compact item.
  await expect(page.locator('.inbox--compact')).toHaveCount(1);
  await expect(detail.locator('.proposal__head .proposal__art .portrait')).toHaveCount(1);
  // The facts wrap to at most two lines rather than being cut (user decision
  // 2026-10-05); the clipping audit checks that no fact is cut.
  const meta = detail.locator('.proposal__meta');
  await expect.poll(async () => (await meta.boundingBox())!.height).toBeLessThanOrEqual(48);
});

// The Inbox thread from the API's `thread` (each message marked `used`): the
// Used/All toggle starts on Used; All adds the context, used rows lifted.
test('inbox: thread toggle shows the used messages, then all of them', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/inbox?tab=extractor&item=p-bm-move&sw=off`);
  const items = (await (await page.request.get(`${ADMIN}/api/admin/inbox`)).json()) as {
    id: string;
    thread: { id: string; used: boolean }[] | null;
    evidence: { id: string; missing: boolean }[];
  }[];
  const item = items.find((p) => p.id === 'p-bm-move')!;
  // Cited messages that are gone are left out of the thread but shown (used).
  const gone = item.evidence.filter((e) => e.missing && !item.thread!.some((m) => m.id === e.id)).length;
  const thread = [...item.thread!, ...Array.from({ length: gone }, () => ({ used: true }))];
  const used = thread.filter((m) => m.used).length;
  expect(used).toBeGreaterThan(0);
  expect(used).toBeLessThan(thread.length);
  const detail = page.locator('.inbox__detail');
  await expect(detail.getByRole('heading', { name: `Thread · ${thread.length} messages` })).toBeVisible();
  const toggle = detail.getByRole('group', { name: 'Messages shown' });
  const usedButton = toggle.getByRole('button', { name: `Used ${used}` });
  await expect(usedButton).toHaveAttribute('aria-pressed', 'true');
  const messages = detail.getByLabel('Evidence').getByRole('listitem');
  await expect(messages).toHaveCount(used);
  await toggle.getByRole('button', { name: 'All' }).click();
  await expect(messages).toHaveCount(thread.length);
  await expect(detail.locator('.msg--used')).toHaveCount(used);
  await usedButton.click();
  await expect(messages).toHaveCount(used);
});

// Round 5 / B_HistoryCk: with no backups the Checkpoints tab keeps the
// verification card and says why instead of an empty table -- none taken yet,
// or no backup directory on this server; the Timeline's Week/Who filters are
// hidden there.
test('history checkpoints: empty states without backups, and no timeline filters', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  let configured = true;
  await page.route(/\/api\/admin\/history\/checkpoints$/, async (route) => {
    const response = await route.fetch(unconditional(route));
    const json = (await response.json()) as { backups: unknown[]; backup_dir_configured: boolean };
    json.backups = [];
    json.backup_dir_configured = configured;
    await route.fulfill({ response, json });
  });
  await page.goto(`${ADMIN}/history?sw=off`);
  const filters = page.getByRole('search', { name: 'Filter the history' });
  await expect(filters).toBeVisible();
  await page.getByRole('tab', { name: 'Checkpoints' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Chain verified' })).toBeVisible();
  await expect(page.getByText('No backups recorded yet')).toBeVisible();
  await expect(page.getByText('Backups taken with the deploy script appear here.')).toBeVisible();
  await expect(page.getByRole('table', { name: /Backups/ })).toHaveCount(0);
  await expect(filters).toBeHidden();

  configured = false;
  await page.getByRole('button', { name: 'Verify again' }).click();
  await expect(page.getByText(/This server has no backup directory configured/)).toBeVisible();
  await expect(page.getByText('No backups recorded yet')).toHaveCount(0);
  await expect(page.getByText('Chain verified', { exact: true })).toBeVisible();

  await page.getByRole('tab', { name: 'Timeline' }).click();
  await expect(filters).toBeVisible();
});

// User decision 2026-10-09: History › Sign-ins is the audit log. `/audit`
// opens it; member rows show only a tag of the address; the filters narrow
// the list through the server.
test('history sign-ins: /audit opens the log, members show a tag, filters narrow it', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/audit?sw=off`);
  await expect(page).toHaveURL(/\/history\?tab=sign-ins/);
  await expect(page.getByRole('tab', { name: 'Sign-ins' })).toHaveAttribute('aria-selected', 'true');
  const table = page.getByRole('table', { name: /Sign-in events/ });
  const rows = table.locator('tbody tr');
  await expect(rows).toHaveCount(8);
  await expect(rows.first()).toContainText('Session ended');
  const member = rows.filter({ hasText: 'Mikan' }).filter({ hasText: 'Signed in' });
  await expect(member).toContainText('tag 5f2c9a1d');
  await expect(member).toContainText('Members');
  await expect(rows.filter({ hasText: 'Hoshino' }).filter({ hasText: 'Signed in' })).toContainText('100.64.0.7');
  await expect(page.getByRole('search', { name: 'Filter the history' })).toBeHidden();

  await choose(page.getByRole('combobox', { name: 'Portal' }), { label: 'Members' });
  await expect(rows).toHaveCount(4);
  await expect(rows.filter({ hasText: 'Admin' })).toHaveCount(0);
  await choose(page.getByRole('combobox', { name: 'Event' }), { label: 'Refused' });
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('not eligible');
});

// B_HistoryCk: a failed check turns the card to the risk wash and says so in
// words, naming the first bad record the server reports.
test('history checkpoints: a failed chain check', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.route(/\/api\/admin\/history\/checkpoints$/, async (route) => {
    const response = await route.fetch(unconditional(route));
    const json = (await response.json()) as { verified: { ok: boolean; first_broken: number | null } };
    json.verified.ok = false;
    json.verified.first_broken = 2;
    await route.fulfill({ response, json });
  });
  await page.goto(`${ADMIN}/history?sw=off`);
  await page.getByRole('tab', { name: 'Checkpoints' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Chain check failed' })).toBeAttached();
  await expect(page.getByText('Chain check failed', { exact: true })).toBeVisible();
  await expect(page.getByText(/The history no longer matches its hash chain from record #2/).first()).toBeVisible();
  await expect(page.locator('.history-verify--risk')).toBeVisible();
});

// Review-2 finding 1: a cited message deleted from Discord is left out of the
// thread by the server while the evidence marks it missing. The Inbox merges
// it back in (used, in time order) instead of dropping it.
test('inbox: a cited message that is gone still shows in the thread, as used', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.route(/\/api\/admin\/inbox$/, async (route) => {
    const response = await route.fetch(unconditional(route));
    type Line = { id: string; author: string; at: string; content: string | null; url: string | null; missing: boolean; used?: boolean };
    const json = (await response.json()) as { id: string; evidence: Line[]; thread: Line[] | null }[];
    const item = json.find((p) => p.id === 'p-bm-move')!;
    // The first cited message: deleted since -- missing in the evidence, absent from the thread.
    const cited = item.thread!.find((m) => m.used)!;
    item.thread = item.thread!.filter((m) => m.id !== cited.id);
    item.evidence = item.evidence.map((e) => (e.id === cited.id ? { ...e, content: null, url: null, missing: true } : e));
    await route.fulfill({ response, json });
  });
  await page.goto(`${ADMIN}/inbox?tab=extractor&item=p-bm-move&sw=off`);
  const detail = page.locator('.inbox__detail');
  const thread = detail.getByLabel('Evidence');
  const goneRows = thread.locator('.msg--gone');
  await expect(goneRows.first()).toBeVisible();
  await expect(goneRows.first()).toContainText('This message is no longer stored.');
  await expect(goneRows.first()).toHaveClass(/msg--used/);
  // Counted in Used, and shown while the toggle is on Used.
  const usedButton = detail.getByRole('group', { name: 'Messages shown' }).getByRole('button', { name: /^Used \d+$/ });
  await expect(usedButton).toHaveAttribute('aria-pressed', 'true');
  const n = Number((await usedButton.innerText()).replace(/\D/g, ''));
  await expect(thread.getByRole('listitem')).toHaveCount(n);
  await expect(thread.locator('.msg--used')).toHaveCount(n);
});

// A status change in the decision card reads in run-status words, as chips
// under its own field name -- never the raw value beside the slot.
test('inbox: a status change shows as status chips under its field name', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.route(/\/api\/admin\/inbox$/, async (route) => {
    const response = await route.fetch(unconditional(route));
    const json = (await response.json()) as { id: string; preview: { changes: { target: string; field: string; from: string; to: string }[] } }[];
    const item = json.find((p) => p.id === 'p-bm-move')!;
    const slot = item.preview.changes[0]!;
    item.preview.changes.push({ target: slot.target, field: 'status', from: 'at_risk', to: 'planned' });
    await route.fulfill({ response, json });
  });
  await page.goto(`${ADMIN}/inbox?tab=extractor&item=p-bm-move&sw=off`);
  const change = page.locator('.decision-card .proposal__would');
  await expect(change.locator('.proposal__field')).toHaveText(['slot', 'status']);
  const status = change.locator('.proposal__status');
  await expect(status.locator('del .status-chip')).toHaveText('At risk');
  await expect(status.locator('.status-chip--warn')).toHaveText('Unconfirmed');
  await expect(change).not.toContainText('at_risk');
  // The field name sits above its value, not beside it.
  const [label, value] = await Promise.all([change.locator('.proposal__field').nth(1).boundingBox(), status.boundingBox()]);
  expect(value!.y).toBeGreaterThanOrEqual(label!.y + label!.height - 1);
  if (process.env.KANADE_CAPTURE) await page.screenshot({ path: process.env.KANADE_CAPTURE });
});

// Review-2 finding 4: page chunks no longer re-import the M3E primitives, so a
// later page load cannot reorder the cascade under History's filter pills.
test('history filter pills keep their size after visiting the Inbox', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/history?sw=off`);
  const pill = page.locator('.history-window__filters .dd').first();
  const size = () => pill.evaluate((el) => { const c = getComputedStyle(el); return [c.height, c.paddingLeft, c.paddingRight, c.borderRadius].join(' '); });
  await expect(pill).toBeVisible();
  const before = await size();
  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: /^Inbox/ }).click();
  await expect(page.locator('.inbox__detail')).toBeVisible();
  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'History' }).click();
  await expect(pill).toBeVisible();
  expect(await size()).toBe(before);
});
