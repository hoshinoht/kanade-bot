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
  const editor = page.getByRole('dialog', { name: 'Add a weekly timing' });
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
  const editor = page.getByRole('dialog', { name: 'Friday 21:30 — XKalos' });
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

test('fixed: an edit sends the version it was loaded at; one made stale behind it is refused', async ({ page }) => {
  await go(page, '/fixed');
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  const editor = page.getByRole('dialog', { name: 'Tuesday 23:30 — XBM' });
  await editor.getByLabel('Note').fill('Bring potions');
  // Someone else edits the week while the form is open.
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
  await expect(editor.getByRole('alert')).toContainText('The week changed since it was loaded. Close and reopen');
  await expect(editor.getByLabel('Note')).toHaveValue('Bring potions');

  await editor.getByRole('button', { name: 'Cancel' }).click();
  await page.getByRole('button', { name: 'Edit Tuesday 23:30 — XBM' }).click();
  await editor.getByLabel('Note').fill('Bring potions');
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
  await page.getByRole('button', { name: 'Retire Tuesday 23:30 — XBM' }).click();
  await page.getByRole('button', { name: 'Retire timing' }).click();
  await expect(toast(page, 'Retired Tuesday 23:30 — XBM; 1 upcoming run cancelled.')).toBeVisible();
  expect(writes.map((w) => w.status)).toEqual([403, 200]);
  expect(writes[1]!.key).toBe(writes[0]!.key);
});

test('fixed: retiring names its consequence and cancels upcoming runs', async ({ page }) => {
  await go(page, '/fixed');
  await page.getByRole('button', { name: 'Retire Tuesday 23:30 — XBM' }).click();
  const confirm = page.getByRole('dialog', { name: 'Retire Tuesday 23:30 — XBM?' });
  await expect(confirm).toContainText('1 upcoming run is cancelled');
  await confirm.getByRole('button', { name: 'Keep it' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('8 weekly timings');
  await page.getByRole('button', { name: 'Retire Tuesday 23:30 — XBM' }).click();
  await page.getByRole('button', { name: 'Retire timing' }).click();
  await expect(toast(page, 'Retired Tuesday 23:30 — XBM; 1 upcoming run cancelled.')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('7 weekly timings');
});

test('run sheet: this-week roster line and reset to fixed', async ({ page }) => {
  await go(page, '/');
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await expect(sheet.getByText('this week: +Ren')).toBeVisible();
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

test('members: roster rows, sheet edits for pings, reply style and aliases', async ({ page }) => {
  await go(page, '/members');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 bossers');
  await expect(page.getByRole('button', { name: /^Ren #1013/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /^Kohane/ })).toContainText('chat only');
  await page.getByRole('searchbox', { name: 'Search members' }).fill('tsu');
  await expect(page.getByRole('list', { name: 'Members' }).getByRole('button')).toHaveCount(1);
  await page.getByRole('button', { name: /^Tsubame/ }).click();

  const sheet = page.getByRole('dialog', { name: 'Tsubame' });
  await expect(sheet.getByRole('button', { name: 'Off' })).toHaveAttribute('aria-pressed', 'true');
  await sheet.getByRole('button', { name: 'Essential' }).click();
  await expect(sheet.getByRole('status')).toHaveText('Pings set to essential.');
  await sheet.getByLabel('Reply style').selectOption({ label: 'Terse' });
  await expect(sheet.getByRole('status')).toHaveText('Reply style set to Terse.');
  await sheet.getByRole('textbox', { name: 'New alias for Tsubame' }).fill('swallow');
  await sheet.getByRole('button', { name: 'Add' }).click();
  await expect(sheet.locator('.membersheet__aliases')).toContainText('swallow');
  await sheet.getByRole('textbox', { name: 'New alias for Tsubame' }).fill('mika');
  await sheet.getByRole('button', { name: 'Add' }).click();
  await expect(sheet.getByRole('status')).toHaveText('“mika” already names someone.');
  await expect(sheet.getByRole('textbox', { name: 'New alias for Tsubame' })).toHaveValue('mika');

  await page.keyboard.press('Escape');
  await expect(sheet).toBeHidden();
  await page.getByRole('searchbox', { name: 'Search members' }).fill('');
  await expect(page.getByRole('button', { name: /^Rin/ })).toBeVisible();
  await page.getByRole('button', { name: /^Rin/ }).click();
  await expect(page.getByRole('dialog', { name: 'Rin' }).getByText('is no longer offered')).toBeVisible();
});

test('reminders: queued, due, sent and stale, all runs or one', async ({ page }) => {
  await go(page, '/reminders');
  const queued = page.getByRole('table', { name: 'Queued reminders' });
  const sent = page.getByRole('table', { name: 'Sent reminders' });
  await expect(queued.getByRole('row').nth(1)).toContainText('Tue 29 Sep 21:00');
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
