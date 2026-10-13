import { mkdirSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, csrf, expect, settle, test } from './support';

// Who each party member brings to a run (module `roster`; boards B_RosterRunPick and
// B_PhoneRosterRunPick) over the mock's seed on HCarling + HStar (r-carling): Asahi
// picked AsahiDK (of two, his main), Mika brings MikaBish from the timing (f-carling's
// pick; main MikaNL, three to pick from), Ren (1), Yuzu, Tsubame and Hotaru their mains
// (Ren's RenShade is on the done NStar run too: a clash), Ren (2) none.

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
  await expect(sheet.getByRole('button', { name: /^Mika's character on HCarling \+ HStar/ })).toBeVisible();
  return sheet;
}

const slot = (sheet: Locator, name: string) =>
  sheet.locator('.slot').filter({ has: sheet.page().locator('.slot__name', { hasText: new RegExp(`^${name.replace(/[()]/g, '\\$&')}$`) }) });
const notice = (sheet: Locator) => sheet.locator('.sheet__notice');

async function picked(page: Page, member: string): Promise<{ picked: string | null; brings: string | null; basis: string | null }> {
  const response = await page.request.get(`${ADMIN}/api/admin/runs/${RUN}/characters`);
  const slots = (await response.json()).slots as { member_id: string; picked: string | null; brings: string | null; basis: string | null }[];
  return slots.find((s) => s.member_id === member)!;
}

test('roster run: pick from the popover by keyboard, any active character (no level filter)', async ({ page }) => {
  const sheet = await carling(page);
  await sheet.getByRole('button', { name: "Mika's character on HCarling + HStar: MikaBish, from timing. Change" }).click();
  const picker = sheet.getByRole('dialog', { name: "Mika's character for HCarling + HStar" });
  const options = picker.getByRole('option');
  await expect(options).toHaveText([/^\s*MikaNL main Night Lord Lv\. 286$/, /^\s*MikaBish Bishop/, /^\s*Mikadere Lara Lv\. 266$/]);
  // Mikadere (Lv. 266, under HStar's 280 entry) is offered like the others.
  await expect(options.nth(2)).not.toHaveAttribute('aria-disabled', 'true');
  await expect(options.nth(1)).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(picker).toBeHidden();
  const chip = sheet.getByRole('button', { name: "Mika's character on HCarling + HStar: Mikadere. Change" });
  await expect(chip).toBeFocused();
  await expect(notice(sheet)).toContainText('Mika brings Mikadere to HCarling + HStar.');
  expect(await picked(page, '1003')).toMatchObject({ picked: 'ch-s4', basis: 'run_pick' });

  // Escape closes the popover only, focus back on the chip.
  await chip.click();
  await expect(picker.getByRole('option', { name: /Mikadere/ })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Escape');
  await expect(picker).toBeHidden();
  await expect(sheet).toBeVisible();
  await expect(chip).toBeFocused();
});

test('roster run: clear a pick, and undo it', async ({ page }) => {
  const sheet = await carling(page);
  await sheet.getByRole('button', { name: "Asahi's character on HCarling + HStar: AsahiDK. Change" }).click();
  await sheet.getByRole('button', { name: 'Clear pick' }).click();
  // Cleared, he brings his main again.
  await expect(notice(sheet)).toContainText("Cleared Asahi's pick: back to AsahiDK (main).");
  await expect(slot(sheet, 'Asahi').getByRole('button', { name: "Asahi's character on HCarling + HStar: AsahiDK, main. Change" })).toBeVisible();
  expect(await picked(page, '1001')).toMatchObject({ picked: null, brings: 'ch-s5', basis: 'main' });
  await notice(sheet).getByRole('button', { name: 'Undo' }).click();
  await expect(slot(sheet, 'Asahi').getByRole('button', { name: "Asahi's character on HCarling + HStar: AsahiDK. Change" })).toBeVisible();
  expect((await picked(page, '1001')).picked).toBe('ch-s5');
});

test('roster run: the main until picked; picking stores it, Clear goes back to the main', async ({ page }) => {
  const sheet = await carling(page);
  const yuzu = slot(sheet, 'Yuzu');
  await expect(yuzu.locator('.slot__basis')).toHaveText('main');
  await yuzu.getByRole('button', { name: "Yuzu's character on HCarling + HStar: YuzuPala, main. Change" }).click();
  await page.keyboard.press('Enter');
  await expect(notice(sheet)).toContainText('Yuzu brings YuzuPala to HCarling + HStar.');
  await expect(yuzu.locator('.slot__basis')).toHaveCount(0);
  expect(await picked(page, '1004')).toMatchObject({ picked: 'ch-s8', brings: 'ch-s8', basis: 'run_pick' });

  await yuzu.getByRole('button', { name: /YuzuPala\. Change$/ }).click();
  await sheet.getByRole('button', { name: 'Clear pick · back to the default' }).click();
  await expect(notice(sheet)).toContainText("Cleared Yuzu's pick: back to YuzuPala (main).");
  await expect(yuzu.locator('.slot__basis')).toHaveText('main');
  expect(await picked(page, '1004')).toMatchObject({ picked: null, brings: 'ch-s8', basis: 'main' });
});

test('roster run: a clash names the other run and never blocks the pick', async ({ page }) => {
  const sheet = await carling(page);
  const ren = slot(sheet, 'Ren (1)');
  const warning = ren.getByRole('note');
  await expect(warning).toHaveText(/^\s*RenShade is also on \w{3} \d{1,2} 21:00 NStar this boss week \(main there\)\.$/);
  await ren.getByRole('button', { name: /RenShade, main\. Change$/ }).click();
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
  await expect(sheet.locator('.slotchar, .slot__warn, .slot__basis')).toHaveCount(0);
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
  await sheet.getByRole('button', { name: /^Mika's character on HCarling/ }).click();
  const picker = sheet.getByRole('dialog', { name: "Mika's character for HCarling + HStar" });
  await expect(picker.getByRole('option', { name: /MikaBish/ })).toBeFocused();
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
  // Presence check-ins (the open sheet, editing while picking) write nothing; the rest must be the one pick.
  page.on('request', (r) => r.method() !== 'GET' && r.url().includes('/api/') && !r.url().includes('/api/admin/presence') && writes.push(`${r.method()} ${new URL(r.url()).pathname}`));

  await pane.getByRole('button', { name: 'Open in a larger view' }).click();
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await expect(sheet.getByText('Staging into Weekend reshuffle')).toBeVisible();
  await sheet.getByRole('button', { name: /^Mika's character on HCarling/ }).click();
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
  const pick = sheet.getByRole('button', { name: /^Mika's character on HCarling/ });
  await expect(pick).toHaveText('MikaBish281');
  await expect(slot(sheet, 'Mika').locator('.slot__basis')).toHaveText('from timing');
  await pick.click();
  const picker = page.getByRole('dialog', { name: "Mika's character" });
  await expect(picker.getByRole('radio', { name: /MikaBish/ })).toBeChecked();
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

/** The notice line (and its Undo) never lies over the status group or any of its buttons. */
async function clearOfStatus(sheet: Locator) {
  const line = notice(sheet);
  await expect(line).toBeVisible();
  await settle(sheet.page());
  const boxes = [(await line.boundingBox())!];
  for (const undo of await line.getByRole('button', { name: 'Undo' }).all()) boxes.push((await undo.boundingBox())!);
  const status = sheet.locator('.seg--status');
  const targets = [(await status.boundingBox())!];
  for (const button of await status.locator('.seg__btn').all()) targets.push((await button.boundingBox())!);
  expect(targets.length).toBeGreaterThan(1);
  for (const a of boxes)
    for (const b of targets) {
      const overlap = a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
      expect(overlap, `notice ${JSON.stringify(a)} over status ${JSON.stringify(b)}`).toBe(false);
    }
}

// The pick notice used to sit over the hero's status group on laptop sheets (1566 px).
for (const [width, height] of [
  [1566, 900],
  [1280, 800],
  [390, 844],
] as const) {
  test(`roster run: the pick and status notices keep clear of the status group at ${width}x${height}`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    const sheet = await carling(page);
    const before = (await picked(page, '1003')).picked;
    await sheet.getByRole('button', { name: /^Mika's character on HCarling/ }).click();
    if (width < 900) {
      const picker = page.getByRole('dialog', { name: "Mika's character" });
      await picker.getByRole('radio', { name: /Mikadere/ }).check();
      await picker.getByRole('button', { name: 'Use Mikadere' }).click();
    } else {
      await page.keyboard.press('ArrowDown');
      await page.keyboard.press('Enter');
    }
    await expect(notice(sheet)).toContainText('Mika brings Mikadere to HCarling + HStar.');
    await clearOfStatus(sheet);

    // Undo still works and hands focus back to the slot's character button.
    await notice(sheet).getByRole('button', { name: 'Undo' }).click();
    await expect.poll(async () => (await picked(page, '1003')).picked).toBe(before);
    await expect(slot(sheet, 'Mika').getByRole('button', { name: /^Mika's character/ })).toBeFocused();

    // A status change's notice (with its own Undo) uses the same line.
    await sheet.locator('.seg--status').getByRole('button', { name: 'Confirmed' }).click();
    await expect(notice(sheet).getByRole('button', { name: 'Undo' })).toBeVisible();
    await clearOfStatus(sheet);
  });
}

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
    await shot('desktop-1-picker-closed-basis-warning');
    await sheet.getByRole('button', { name: /^Mika's character on HCarling/ }).click();
    await shot('desktop-2-picker-open');
    await page.keyboard.press('Enter');
    await expect(notice(sheet)).toContainText('Mika brings MikaBish');
    await shot('desktop-3-picked');

    await page.request.post(`${ADMIN}/api/admin/reset`);
    await page.setViewportSize({ width: 390, height: 844 });
    sheet = await carling(page);
    await shot('phone-1-picker-closed-basis-warning');
    await sheet.getByRole('button', { name: /^Mika's character on HCarling/ }).click();
    await expect(page.getByRole('dialog', { name: "Mika's character" })).toBeVisible();
    await shot('phone-2-picker-open');
  });
}
