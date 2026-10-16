import { mkdirSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, PUBLIC, ageEstimate, csrf, expect, setEstimates, settle, signInPublic, test, useLook } from './support';

// Per-boss character picks and the Party grid (boards in docs/research/2026-10-16-per-boss-picks,
// v2; the Party total from 2026-10-16-party-polish) over the e2e server's seed: HCarling +
// HMaleficStar this week (r-carling: Asahi picked AsahiDK, Mika brings MikaBish from the f-carling
// timing) and next week (n-carling: Asahi brings AsahiMech to HMaleficStar, the seeded per-boss pick).

const T = 'HCarling + HMaleficStar';
const MIKA = '100000000000001003';
const FIXED = 'Tuesday 22:00 — HCarling + HMaleficStar';

type Slot = { member_id: string; per_boss: boolean; bosses: { key: string; picked: string | null; brings: string | null; basis: string | null; version: number }[] };
async function slotOf(page: Page, path: string, member: string): Promise<Slot> {
  return ((await (await page.request.get(`${ADMIN}/api/admin/${path}/characters`)).json()).slots as Slot[]).find((s) => s.member_id === member)!;
}

/** The run's full sheet: the pane's larger view on a laptop, the sheet on a phone. */
async function sheetOf(page: Page, run = 'r-carling', next = false): Promise<Locator> {
  await page.goto(`${ADMIN}/${next ? '?week=next&' : '?'}sw=off`);
  const phone = page.viewportSize()!.width < 900;
  if (!phone) await page.locator('.board').evaluate((board) => (board.scrollLeft = board.scrollWidth));
  await page.locator(`[data-run="${run}"] .plan-card__open`).click();
  if (!phone) await page.getByRole('complementary', { name: T }).getByRole('button', { name: 'Open in a larger view' }).click();
  const sheet = page.getByRole('dialog', { name: T });
  await expect(sheet.locator('.bossgrid, .bossstack').first()).toBeVisible();
  return sheet;
}

const row = (sheet: Locator, name: string) =>
  sheet.locator('.bossgrid__row, .slot').filter({ has: sheet.page().locator('.slot__name', { hasText: new RegExp(`^${name.replace(/[()]/g, '\\$&')}$`) }) });
const notice = (sheet: Locator) => sheet.locator('.sheet__notice');

async function lookUp(page: Page, run = 'r-carling') {
  const { job } = await (await page.request.post(`${ADMIN}/api/admin/estimates`, { headers: await csrf(page.request), data: { run_id: run } })).json();
  await expect.poll(async () => (await (await page.request.get(`${ADMIN}/api/admin/estimates/${job.id}`)).json()).state, { timeout: 30_000 }).toBe('done');
}

test('party grid: bosses drawn once, a later boss without its own pick faded "same as" the first', async ({ page }) => {
  const sheet = await sheetOf(page);
  const grid = sheet.getByRole('table', { name: /^Party/ });
  await expect(grid.locator('thead th[scope="col"]')).toHaveText([/Karling\s*HARD/, /Radiant Malefic Star\s*HARD/]);
  await expect(grid.locator('thead')).toHaveCount(1);
  // Every member with characters has a cell per boss; HMaleficStar inherits for all of them this week.
  const yuzu = row(sheet, 'Yuzu');
  await expect(yuzu.locator('td')).toHaveCount(3);
  await expect(yuzu.locator('td.bossgrid--inherited')).toHaveCount(1);
  await expect(yuzu.locator('td.bossgrid--inherited')).toContainText('same as HCarling');
  // Nothing differs yet: no "Same for every boss".
  await expect(sheet.getByRole('button', { name: /same character for every boss/ })).toHaveCount(0);
  // Next week: the seeded per-boss pick is a real cell, not faded.
  const next = await sheetOf(page, 'n-carling', true);
  const asahi = row(next, 'Asahi');
  await expect(asahi.getByRole('button', { name: `Asahi's character for HMaleficStar: AsahiMech. Change` })).toBeVisible();
  await expect(asahi.locator('td.bossgrid--inherited')).toHaveCount(0);
  await expect(asahi.locator('.bossgrid__basis')).toHaveText('HMaleficStar picked on this run');
  await expect(asahi.getByRole('button', { name: 'Asahi: same character for every boss on this run' })).toBeVisible();
});

test('party grid: a per-boss pick saves at once with Undo; "Same for every boss" clears it, Undo puts it back', async ({ page }) => {
  const sheet = await sheetOf(page);
  const mika = row(sheet, 'Mika');
  await mika.getByRole('button', { name: `Mika's character for HMaleficStar: MikaBish, same as HCarling. Change` }).click();
  const picker = sheet.getByRole('dialog', { name: `Mika's character for HMaleficStar on ${T}` });
  await expect(picker.getByRole('option', { name: /MikaBish/ })).toContainText('same as HCarling');
  await expect(picker).toContainText('For this run only.');
  await picker.getByRole('option', { name: /MikaNL/ }).click();
  await expect(notice(sheet)).toContainText('Mika brings MikaNL to HMaleficStar.');
  const chip = mika.getByRole('button', { name: `Mika's character for HMaleficStar: MikaNL. Change` });
  await expect(chip).toBeFocused();
  await expect(mika.locator('td.bossgrid--inherited')).toHaveCount(0);
  expect((await slotOf(page, 'runs/r-carling', MIKA)).bosses[1]).toMatchObject({ picked: 'ch-s2', basis: 'run_pick' });

  await notice(sheet).getByRole('button', { name: 'Undo' }).click();
  await expect(mika.locator('td.bossgrid--inherited')).toHaveCount(1);
  await expect.poll(async () => (await slotOf(page, 'runs/r-carling', MIKA)).bosses[1]!.picked).toBeNull();

  // Pick again, then make every boss the same in one write.
  await mika.getByRole('button', { name: /^Mika's character for HMaleficStar/ }).click();
  await picker.getByRole('option', { name: /MikaNL/ }).click();
  await expect(notice(sheet)).toContainText('Mika brings MikaNL to HMaleficStar.');
  await mika.getByRole('button', { name: 'Mika: same character for every boss on this run' }).click();
  await expect(notice(sheet)).toContainText('Mika brings MikaBish to every boss again.');
  await expect(mika.locator('td.bossgrid--inherited')).toHaveCount(1);
  expect((await slotOf(page, 'runs/r-carling', MIKA)).per_boss).toBe(false);
  await notice(sheet).getByRole('button', { name: 'Undo' }).click();
  await expect(notice(sheet)).toContainText("Mika's per-boss picks are back.");
  await expect(mika.getByRole('button', { name: `Mika's character for HMaleficStar: MikaNL. Change` })).toBeVisible();
});

test('party grid: an Undo of "Same for every boss" that fails keeps an Undo for what was not put back', async ({ page }) => {
  const sheet = await sheetOf(page);
  const mika = row(sheet, 'Mika');
  await mika.getByRole('button', { name: /^Mika's character for HMaleficStar/ }).click();
  await sheet.getByRole('dialog', { name: `Mika's character for HMaleficStar on ${T}` }).getByRole('option', { name: /MikaNL/ }).click();
  await expect(notice(sheet)).toContainText('Mika brings MikaNL to HMaleficStar.');
  await mika.getByRole('button', { name: 'Mika: same character for every boss on this run' }).click();
  await expect(notice(sheet)).toContainText('Mika brings MikaBish to every boss again.');
  // The restore's first PUT is refused as stale (this run has one later boss, so it is the only one).
  let refused = 0;
  await page.route(`**/api/admin/runs/r-carling/characters/${MIKA}/bosses/*`, async (route) => {
    if (route.request().method() === 'PUT' && refused === 0) {
      refused += 1;
      await route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: 'stale', message: 'Someone else changed it.' }) });
    } else await route.continue();
  });
  await notice(sheet).getByRole('button', { name: 'Undo' }).click();
  await expect(notice(sheet)).toContainText("Put back 0 of 1 of Mika's per-boss picks; HMaleficStar failed");
  await expect(notice(sheet)).toContainText('Undo tries the last one again.');
  expect(refused).toBe(1);
  expect((await slotOf(page, 'runs/r-carling', MIKA)).per_boss).toBe(false);
  // The kept Undo retries and puts it back.
  await notice(sheet).getByRole('button', { name: 'Undo' }).click();
  await expect(notice(sheet)).toContainText("Mika's per-boss picks are back.");
  await expect(mika.getByRole('button', { name: `Mika's character for HMaleficStar: MikaNL. Change` })).toBeVisible();
  expect((await slotOf(page, 'runs/r-carling', MIKA)).bosses[1]).toMatchObject({ picked: 'ch-s2', basis: 'run_pick' });
});

test('party grid: a stale per-boss pick (409) re-reads and says so', async ({ page }) => {
  const sheet = await sheetOf(page);
  const mika = row(sheet, 'Mika');
  await mika.getByRole('button', { name: /^Mika's character for HMaleficStar/ }).click();
  const picker = sheet.getByRole('dialog', { name: `Mika's character for HMaleficStar on ${T}` });
  await expect(picker).toBeVisible();
  // Another admin picks Mikadere for HMaleficStar just before this pick arrives.
  let other = 0;
  await page.route(`**/api/admin/runs/r-carling/characters/${MIKA}/bosses/*`, async (route) => {
    if (route.request().method() === 'PUT' && other === 0)
      other = (
        await page.request.put(`${ADMIN}/api/admin/runs/r-carling/characters/${MIKA}/bosses/MaleficStar`, {
          headers: await csrf(page.request),
          data: { character_id: 'ch-s4', version: 0 },
        })
      ).status();
    await route.continue();
  });
  await picker.getByRole('option', { name: /MikaNL/ }).click();
  await expect(notice(sheet)).toContainText("Mika's character for HMaleficStar changed meanwhile; the cell shows what is saved now.");
  expect(other).toBe(200);
  await expect(mika.getByRole('button', { name: `Mika's character for HMaleficStar: Mikadere. Change` })).toBeVisible();
});

test('party total: no estimate yet, fresh, assumed crit, older than n days, and estimates off', async ({ page }) => {
  let sheet = await sheetOf(page);
  const total = sheet.locator('.bossgrid__total');
  await expect(total).toContainText('Party total');
  await expect(total).toContainText('Look up party fills it in.');
  await expect(total.locator('.party-total__none')).toHaveText(['no estimate yet', 'no estimate yet']);

  await setEstimates('assumed', 'MikaBish');
  await lookUp(page);
  sheet = await sheetOf(page);
  await expect(total).toContainText(/as of \w{3} 29 Sep \d{2}:\d{2}/);
  await expect(total.locator('.party-total__num').first()).toHaveText(/^\d+\.\d%\*$/);
  await expect(sheet.locator('.bossgrid-wrap > .share__notes')).toContainText('* MikaBish at an assumed 100% critical rate (actual 89%)');
  await expect(sheet.locator('.bossgrid-wrap > .share__notes')).toContainText('1 with no estimate: Ren (2) (not in the total, never 0%)');
  await expect(total).toContainText('not complete');

  await ageEstimate({ run: 'r-carling' }, 8);
  sheet = await sheetOf(page);
  await expect(total.locator('.party-total__old')).toHaveText('older than 7 days');

  await setEstimates('off');
  sheet = await sheetOf(page);
  await expect(sheet.locator('.bossgrid__total, .estimate-panel')).toHaveCount(0);
  await expect(row(sheet, 'Yuzu').locator('td')).toHaveCount(3);
});

test('party grid: the phone draws the Party total first, then a card per member with one line per boss', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const sheet = await sheetOf(page, 'n-carling', true);
  const list = sheet.getByRole('list', { name: 'Party' });
  const items = list.locator(':scope > li');
  // Readiness answers after the characters: wait for its note before reading the order.
  await expect(list.locator(':scope > li.ready__note')).toBeVisible();
  await expect
    .poll(() => items.evaluateAll((els) => els.map((el) => el.className.split(' ')[0]).slice(0, 3)))
    .toEqual(['ready__note', 'party-total', 'slot']);
  const asahi = row(sheet, 'Asahi');
  await expect(asahi.getByRole('list', { name: "Asahi's character per boss" }).getByRole('listitem')).toHaveCount(2);
  const same = asahi.getByRole('button', { name: 'Asahi: same character for every boss on this run' });
  await expect(same).toBeVisible();
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  expect((await same.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  await asahi.getByRole('button', { name: /^Asahi's character for HMaleficStar/ }).click();
  const picker = page.getByRole('dialog', { name: "Asahi's character for HMaleficStar" });
  await expect(picker.getByRole('radio', { name: /AsahiMech/ })).toBeChecked();
  await picker.getByRole('radio', { name: /AsahiDK/ }).check();
  await picker.getByRole('button', { name: 'Use AsahiDK' }).click();
  await expect(notice(sheet)).toContainText('Asahi brings AsahiDK to HMaleficStar.');
});

test('fixed editor: Per boss opens the list; a per-boss pick saves at once, Same for every boss clears it', async ({ page }) => {
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: `Edit ${FIXED}` }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  await editor.getByRole('tab', { name: /^Characters/ }).click();
  const mika = editor.locator('.fixedready__row').filter({ has: page.locator('.fixedready__name', { hasText: /^Mika$/ }) });
  // One character: nothing new. Several: Per boss.
  const ren = editor.locator('.fixedready__row').filter({ has: page.locator('.fixedready__name', { hasText: /^Ren \(1\)$/ }) });
  await expect(ren.getByRole('button', { name: /per boss/ })).toHaveCount(0);
  await mika.getByRole('button', { name: "Pick Mika's character per boss" }).click();
  const list = mika.getByRole('list', { name: "Mika's character per boss" });
  await expect(list.getByRole('listitem')).toHaveCount(2);
  await expect(list.getByRole('listitem').last()).toContainText('same as HCarling');
  await list.getByRole('button', { name: "Mika's character for HMaleficStar: MikaBish, same as HCarling, change" }).click();
  const picker = editor.getByRole('dialog', { name: `Mika's character for HMaleficStar on ${FIXED}` });
  await expect(picker).toContainText('Runs made from this timing bring it to HMaleficStar unless the run has its own pick.');
  await picker.getByRole('option', { name: /MikaNL/ }).click();
  await expect(editor.locator('.fixedready__status')).toContainText('Mika brings MikaNL to HMaleficStar on this timing.');
  await expect(mika).toContainText('per boss');
  await expect(mika).toContainText('picked for HMaleficStar');
  expect((await slotOf(page, 'fixed/f-carling', MIKA)).bosses[1]).toMatchObject({ picked: 'ch-s2', basis: 'timing_pick' });
  // Runs made from the timing read it live.
  expect((await slotOf(page, 'runs/r-carling', MIKA)).bosses[1]).toMatchObject({ brings: 'ch-s2', basis: 'timing_pick' });

  await mika.getByRole('button', { name: 'Mika: same character for every boss (clears the per-boss picks)' }).click();
  await expect(editor.locator('.fixedready__status')).toContainText('Mika brings MikaBish to every boss again.');
  expect((await slotOf(page, 'fixed/f-carling', MIKA)).per_boss).toBe(false);
});

// For user approval (not assertions): desktop and phone, light and one dark.
const CAPTURES = '/private/tmp/PicksUi';
test('per-boss picks captures', async ({ page }) => {
  test.skip(!process.env.KANADE_PICKS_CAPTURES, 'set KANADE_PICKS_CAPTURES=1 to write the review captures');
  test.setTimeout(240_000);
  mkdirSync(CAPTURES, { recursive: true });
  const shot = async (name: string) => {
    await settle(page);
    await page.screenshot({ path: `${CAPTURES}/${name}.png`, animations: 'disabled' });
  };
  await setEstimates('assumed', 'MikaBish');
  await lookUp(page, 'n-carling');
  for (const [look, theme] of [
    ['blossom', 'light'],
    ['blossom', 'dark'],
  ] as const) {
    await useLook(page, look, theme);
    const t = theme === 'dark' ? '-dark' : '';
    await page.setViewportSize({ width: 1280, height: 800 });
    let sheet = await sheetOf(page, 'n-carling', true);
    await shot(`admin-grid-2boss${t}`);
    if (theme === 'dark') continue;
    await row(sheet, 'Asahi').getByRole('button', { name: /^Asahi's character for HMaleficStar/ }).click();
    await shot('admin-grid-picker-open');
    await page.keyboard.press('Escape');
    await sheet.locator('.runsheet__aside .estimate-panel').scrollIntoViewIfNeeded();
    await shot('admin-grid-estimate-panel');
    await page.setViewportSize({ width: 390, height: 844 });
    sheet = await sheetOf(page, 'n-carling', true);
    await shot('phone-grid-total');
    await row(sheet, 'Asahi').scrollIntoViewIfNeeded();
    await shot('phone-grid-cards');
    // States: no estimate yet and older than (this week's run).
    await page.setViewportSize({ width: 1280, height: 800 });
    await sheetOf(page);
    await shot('admin-grid-no-estimate');
    await ageEstimate({ run: 'r-carling' }, 8);
    await sheetOf(page);
    await shot('admin-grid-older-than');
    // Fixed editor per boss.
    await page.request.put(`${ADMIN}/api/admin/fixed/f-carling/characters/${MIKA}/bosses/MaleficStar`, { headers: await csrf(page.request), data: { character_id: 'ch-s2', version: 0 } });
    await page.goto(`${ADMIN}/fixed?sw=off`);
    await page.getByRole('button', { name: `Edit ${FIXED}` }).click();
    const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
    await editor.getByRole('tab', { name: /^Characters/ }).click();
    await editor.locator('.bosspick').first().scrollIntoViewIfNeeded();
    await shot('fixed-per-boss');
    await editor.getByRole('button', { name: /^Mika's character for HMaleficStar/ }).click();
    await shot('fixed-per-boss-picker');
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`${ADMIN}/fixed?sw=off`);
    await page.getByRole('button', { name: `Edit ${FIXED}` }).click();
    const phoneEditor = page.getByRole('dialog', { name: FIXED });
    await phoneEditor.getByRole('tab', { name: /^Characters/ }).click();
    await phoneEditor.locator('.bosspick').first().scrollIntoViewIfNeeded();
    await shot('phone-fixed-per-boss');
  }
  // Four bosses: next week's run from the timing with two more bosses.
  await useLook(page, 'blossom', 'light');
  await page.setViewportSize({ width: 1280, height: 800 });
  const rows = (await (await page.request.get(`${ADMIN}/api/admin/fixed`)).json()) as { id: string; weekday: number; time: string; bosses: { token: string }[]; participants: { id: string }[]; channel_id: string; note: string | null }[];
  const carling = rows.find((r) => r.id === 'f-carling')!;
  const { version } = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number };
  const patched = await page.request.patch(`${ADMIN}/api/admin/fixed/f-carling`, {
    headers: await csrf(page.request),
    data: { weekday: carling.weekday, time: carling.time, bosses: 'HCarling HMaleficStar HLimbo NBaldrix', participants: carling.participants.map((p) => p.id), channel_id: carling.channel_id, note: carling.note, version },
  });
  if (patched.ok()) {
    await page.goto(`${ADMIN}/?week=next&sw=off`);
    await page.locator('.board').evaluate((board) => (board.scrollLeft = board.scrollWidth));
    await page.locator('[data-run="n-carling"] .plan-card__open').click();
    await page.locator('.week-pane').getByRole('button', { name: 'Open in a larger view' }).click();
    await expect(page.locator('.bossgrid--wide')).toBeVisible();
    await shot('admin-grid-4boss');
    await page.locator('.bossgrid__scroll--wide').evaluate((el) => (el.scrollLeft = el.scrollWidth));
    await shot('admin-grid-4boss-scrolled');
  } else console.log(`4-boss timing edit refused: ${await patched.text()}`);
  // Portal: Party total and per-boss lines (Asahi, next week).
  await useLook(page, 'marigold', 'light');
  await signInPublic(page);
  await page.goto(`${PUBLIC}/?week=next&run=n-carling&sw=off`);
  await page.getByRole('region', { name: 'Party · characters' }).scrollIntoViewIfNeeded();
  await shot('portal-party-total');
  await page.locator('.party-char--me').scrollIntoViewIfNeeded();
  await shot('portal-per-boss-lines');
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${PUBLIC}/?week=next&run=n-carling&sw=off`);
  await page.locator('.party-total').scrollIntoViewIfNeeded();
  await shot('portal-phone-party-total');
});
