import { mkdirSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, csrf, expect, settle, test } from './support';

// Who each party member brings to a run (module `roster`; boards B_RosterRunPick and
// B_PhoneRosterRunPick) over the mock's seed on HCarling + HStar (r-carling): Asahi
// picked AsahiDK (of two), Ren (1) has only RenShade (inferred, and inferred on the
// done NStar run too: a clash), Mika three to pick from, Yuzu, Tsubame and Hotaru one
// each (inferred), Ren (2) none.

const RUN = 'r-carling';

async function showWeekEnd(page: Page) {
  await page.locator('.board').evaluate((board) => (board.scrollLeft = board.scrollWidth));
  await expect(page.locator('.board.scroll-more-end')).toHaveCount(0);
}

/** A run in the full sheet: the pane's larger view on a laptop, the sheet on a phone. */
async function sheetOf(page: Page, run = RUN, title = 'HCarling + HStar'): Promise<Locator> {
  const phone = page.viewportSize()!.width < 900;
  if (!phone) await showWeekEnd(page);
  await page.locator(`[data-run="${run}"] .plan-card__open`).click();
  if (!phone) await page.getByRole('complementary', { name: title }).getByRole('button', { name: 'Open in a larger view' }).click();
  const sheet = page.getByRole('dialog', { name: title });
  await expect(sheet.getByRole('list', { name: 'Party' })).toBeVisible();
  return sheet;
}

async function carling(page: Page): Promise<Locator> {
  await page.goto(`${ADMIN}/?sw=off`);
  const sheet = await sheetOf(page);
  await expect(sheet.getByRole('button', { name: /^Pick Mika's character/ })).toBeVisible();
  return sheet;
}

const slot = (sheet: Locator, name: string) =>
  sheet.locator('.slot').filter({ has: sheet.page().locator('.slot__name', { hasText: new RegExp(`^${name.replace(/[()]/g, '\\$&')}$`) }) });
const notice = (sheet: Locator) => sheet.locator('.sheet__notice');

async function picked(page: Page, member: string): Promise<{ picked: string | null; inferred: string | null }> {
  const response = await page.request.get(`${ADMIN}/api/admin/runs/${RUN}/characters`);
  const slots = (await response.json()).slots as { member_id: string; picked: string | null; inferred: string | null }[];
  return slots.find((s) => s.member_id === member)!;
}

test('roster run: pick from the popover by keyboard, any active character (no level filter)', async ({ page }) => {
  const sheet = await carling(page);
  await sheet.getByRole('button', { name: /^Pick Mika's character on HCarling \+ HStar: 3 to choose from/ }).click();
  const picker = sheet.getByRole('dialog', { name: "Mika's character for HCarling + HStar" });
  const options = picker.getByRole('option');
  await expect(options).toHaveText([/^\s*MikaNL main Night Lord Lv\. 286$/, /^\s*MikaBish Bishop/, /^\s*Mikadere Lara Lv\. 266$/]);
  // Mikadere (Lv. 266, under HStar's 280 entry) is offered like the others.
  await expect(options.nth(2)).not.toHaveAttribute('aria-disabled', 'true');
  await expect(options.first()).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(picker).toBeHidden();
  const chip = sheet.getByRole('button', { name: "Mika's character on HCarling + HStar: MikaBish. Change" });
  await expect(chip).toBeFocused();
  await expect(notice(sheet)).toContainText('Mika brings MikaBish to HCarling + HStar.');
  expect((await picked(page, '1003')).picked).toBe('ch-s3');

  // Escape closes the popover only, focus back on the chip.
  await chip.click();
  await expect(picker.getByRole('option', { name: /MikaBish/ })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Escape');
  await expect(picker).toBeHidden();
  await expect(sheet).toBeVisible();
  await expect(chip).toBeFocused();
});

test('roster run: clear a pick, and undo it', async ({ page }) => {
  const sheet = await carling(page);
  await sheet.getByRole('button', { name: "Asahi's character on HCarling + HStar: AsahiDK. Change" }).click();
  await sheet.getByRole('button', { name: 'Clear pick' }).click();
  await expect(notice(sheet)).toContainText("Cleared Asahi's pick.");
  await expect(slot(sheet, 'Asahi').getByRole('button', { name: /^Pick Asahi's character.*2 to choose from/ })).toBeVisible();
  expect((await picked(page, '1001')).picked).toBeNull();
  await notice(sheet).getByRole('button', { name: 'Undo' }).click();
  await expect(slot(sheet, 'Asahi').getByRole('button', { name: /AsahiDK\. Change$/ })).toBeVisible();
  expect((await picked(page, '1001')).picked).toBe('ch-s5');
});

test('roster run: inferred until picked; picking stores it, Clear goes back to inferred', async ({ page }) => {
  const sheet = await carling(page);
  const yuzu = slot(sheet, 'Yuzu');
  await expect(yuzu.getByText('inferred', { exact: true })).toBeVisible();
  await yuzu.getByRole('button', { name: "Yuzu's character on HCarling + HStar: YuzuPala, inferred. Change" }).click();
  await page.keyboard.press('Enter');
  await expect(notice(sheet)).toContainText('Yuzu brings YuzuPala to HCarling + HStar.');
  await expect(yuzu.getByText('inferred', { exact: true })).toHaveCount(0);
  expect(await picked(page, '1004')).toMatchObject({ picked: 'ch-s8', inferred: null });

  await yuzu.getByRole('button', { name: /YuzuPala\. Change$/ }).click();
  await sheet.getByRole('button', { name: 'Clear pick · back to inferred' }).click();
  await expect(notice(sheet)).toContainText("Cleared Yuzu's pick: YuzuPala is inferred again.");
  await expect(yuzu.getByText('inferred', { exact: true })).toBeVisible();
  expect(await picked(page, '1004')).toMatchObject({ picked: null, inferred: 'ch-s8' });
});

test('roster run: a clash names the other run and never blocks the pick', async ({ page }) => {
  const sheet = await carling(page);
  const ren = slot(sheet, 'Ren (1)');
  const warning = ren.getByRole('note');
  await expect(warning).toHaveText(/^\s*RenShade is also on \w{3} \d{1,2} 21:00 NStar this boss week \(inferred there\)\.$/);
  await ren.getByRole('button', { name: /RenShade, inferred\. Change$/ }).click();
  await page.keyboard.press('Enter');
  await expect(notice(sheet)).toContainText('Ren (1) brings RenShade to HCarling + HStar.');
  expect((await picked(page, '1002')).picked).toBe('ch-s7');
  await expect(warning).toBeVisible();
});

test('roster run: a member with no characters links to Members', async ({ page }) => {
  const sheet = await carling(page);
  const link = slot(sheet, 'Ren (2)').getByRole('link', { name: 'Ren (2) has no characters yet: add them on Members' });
  await expect(link).toHaveAttribute('href', '/members?open=1013');
  await link.click();
  await expect(page).toHaveURL(/\/members\?open=1013/);
  await expect(page.getByRole('complementary', { name: 'Member details' })).toBeVisible();
});

test('roster run: module off in the session, no character UI and no character requests', async ({ page }) => {
  await page.request.post(`${ADMIN}/__mock/modules`, { data: { roster: false } });
  const asked: string[] = [];
  page.on('request', (r) => /\/api\/admin\/.*characters/.test(r.url()) && asked.push(r.url()));
  await page.goto(`${ADMIN}/?sw=off`);
  const sheet = await sheetOf(page);
  await expect(slot(sheet, 'Mika')).toBeVisible();
  await expect(sheet.locator('.slotchar, .slot__warn, .slot__inferred')).toHaveCount(0);
  expect(asked).toEqual([]);
});

test('roster run: a 404 turns the character UI off for the session, no further requests', async ({ page }) => {
  const asked: string[] = [];
  page.on('request', (r) => /\/api\/admin\/.*characters/.test(r.url()) && asked.push(new URL(r.url()).pathname));
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator(`[data-run="${RUN}"]`)).toBeVisible();
  // The session said on; the module went off since (the server answers the generic 404).
  await page.request.post(`${ADMIN}/__mock/modules`, { data: { roster: false } });
  let sheet = await sheetOf(page);
  await expect.poll(() => asked).toEqual([`/api/admin/runs/${RUN}/characters`]);
  await expect(sheet.locator('.slotchar')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(sheet).toBeHidden();
  await page.getByRole('complementary', { name: 'HCarling + HStar' }).getByRole('button', { name: 'Close HCarling + HStar' }).click();
  sheet = await sheetOf(page, 'r-bm', 'XBM');
  await expect(sheet.locator('.slot').first()).toBeVisible();
  // "Nothing changes" check: real time must pass for a request that should never come.
  await page.waitForTimeout(300);
  expect(asked).toHaveLength(1);
  await expect(sheet.locator('.slotchar')).toHaveCount(0);
});

test('roster run: a stale pick (409) reloads the slot and says so', async ({ page }) => {
  const sheet = await carling(page);
  await sheet.getByRole('button', { name: /^Pick Mika's character/ }).click();
  const picker = sheet.getByRole('dialog', { name: "Mika's character for HCarling + HStar" });
  await expect(picker.getByRole('option').first()).toBeFocused();
  // Another admin picks MikaNL meanwhile.
  const other = await page.request.put(`${ADMIN}/api/admin/runs/${RUN}/characters/1003`, {
    headers: await csrf(page.request),
    data: { character_id: 'ch-s2', version: 0 },
  });
  expect(other.status()).toBe(200);
  await picker.getByRole('option', { name: /MikaBish/ }).click();
  await expect(notice(sheet)).toContainText("Mika's character on HCarling + HStar changed meanwhile; the slot shows what is saved now.");
  await expect(slot(sheet, 'Mika').getByRole('button', { name: /MikaNL\. Change$/ })).toBeVisible();
  expect((await picked(page, '1003')).picked).toBe('ch-s2');
});

test('roster run: while staging into a draft, a pick saves live and stages nothing', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await showWeekEnd(page);
  await page.locator(`[data-run="${RUN}"] .plan-card__open`).click();
  const pane = page.getByRole('complementary', { name: 'HCarling + HStar' });
  await pane.getByRole('button', { name: 'Stage in draft…' }).click();
  await pane.getByRole('dialog', { name: 'Stage in draft' }).getByRole('button', { name: 'Start staging' }).click();
  await expect(pane.getByText('Staging into Weekend reshuffle')).toBeVisible();
  const before = (await (await page.request.get(`${ADMIN}/api/admin/drafts/d-weekend`)).json()).ops.length;
  const writes: string[] = [];
  page.on('request', (r) => r.method() !== 'GET' && r.url().includes('/api/') && writes.push(`${r.method()} ${new URL(r.url()).pathname}`));

  await pane.getByRole('button', { name: 'Open in a larger view' }).click();
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await expect(sheet.getByText('Staging into Weekend reshuffle')).toBeVisible();
  await sheet.getByRole('button', { name: /^Pick Mika's character/ }).click();
  const picker = sheet.getByRole('dialog', { name: "Mika's character for HCarling + HStar" });
  await expect(picker).toContainText('Saves live: character picks are not staged in the draft.');
  await picker.getByRole('option', { name: /MikaNL/ }).click();
  await expect(notice(sheet)).toContainText('Mika brings MikaNL to HCarling + HStar. Saved live, not in the draft.');
  expect(writes).toEqual([`PUT /api/admin/runs/${RUN}/characters/1003`]);
  expect((await picked(page, '1003')).picked).toBe('ch-s2');
  expect((await (await page.request.get(`${ADMIN}/api/admin/drafts/d-weekend`)).json()).ops).toHaveLength(before);
});

test('roster run: the phone picks from a bottom sheet, 44 px rows', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const sheet = await carling(page);
  const pick = sheet.getByRole('button', { name: /^Pick Mika's character/ });
  await expect(pick).toHaveText('Pick3');
  await pick.click();
  const picker = page.getByRole('dialog', { name: "Mika's character" });
  await expect(picker.getByRole('radio', { name: /MikaNL/ })).toBeChecked();
  await picker.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  for (const row of await picker.locator('.charpick__option').all()) expect((await row.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  await picker.getByRole('radio', { name: /Mikadere/ }).check();
  await picker.getByRole('button', { name: 'Use Mikadere' }).click();
  await expect(picker).toBeHidden();
  const chip = sheet.getByRole('button', { name: "Mika's character on HCarling + HStar: Mikadere. Change" });
  await expect(chip).toBeFocused();
  await expect(chip).toHaveText('Mikadere266');
  await expect(slot(sheet, 'Ren (1)').getByRole('note')).toHaveText(/^\s*Also on \w{3} \d{1,2} 21:00 NStar this boss week/);
  await expect(slot(sheet, 'Ren (2)').getByRole('link', { name: /add them on Members/ })).toHaveText('add');
});

// For user approval (not assertions): desktop and phone, blossom and marigold.
const CAPTURES = '/private/tmp/roster-run/captures';
for (const colorway of ['blossom', 'marigold']) {
  test(`roster run capture: ${colorway}`, async ({ page }) => {
    test.skip(!process.env.KANADE_ROSTER_RUN_CAPTURES, 'set KANADE_ROSTER_RUN_CAPTURES=1 to write the review captures');
    mkdirSync(CAPTURES, { recursive: true });
    await page.addInitScript((c) => {
      localStorage.setItem('colorway', c);
      localStorage.setItem('theme', 'light');
    }, colorway);
    const shot = async (name: string) => {
      await settle(page);
      await page.screenshot({ path: `${CAPTURES}/${colorway}-${name}.png`, animations: 'disabled' });
    };
    await page.setViewportSize({ width: 1280, height: 800 });
    let sheet = await carling(page);
    await shot('desktop-1-picker-closed-inferred-warning');
    await sheet.getByRole('button', { name: /^Pick Mika's character/ }).click();
    await shot('desktop-2-picker-open');
    await page.keyboard.press('Enter');
    await expect(notice(sheet)).toContainText('Mika brings MikaNL');
    await shot('desktop-3-picked');

    await page.request.post(`${ADMIN}/api/admin/reset`);
    await page.setViewportSize({ width: 390, height: 844 });
    sheet = await carling(page);
    await shot('phone-1-picker-closed-inferred-warning');
    await sheet.getByRole('button', { name: /^Pick Mika's character/ }).click();
    await expect(page.getByRole('dialog', { name: "Mika's character" })).toBeVisible();
    await shot('phone-2-picker-open');
  });
}
