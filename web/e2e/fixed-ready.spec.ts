import { mkdirSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, csrf, expect, settle, test } from './support';

// The Fixed editor's Characters tab (modules `roster` and `character_data`; boards
// docs/research/2026-10-13-fixed-readiness): who each member brings to a weekly timing and
// their readiness, over the mock's seed. Tuesday 22:00 HCarling + HStar (f-carling): Mika
// picked MikaBish for the timing (main MikaNL), Asahi brings his main AsahiDK (of two),
// Ren (1), Yuzu, Tsubame and Hotaru their mains. Tuesday 23:30 XBM (f-bm): Minato has no
// characters, Kaito one alt (only character), Sora two and no main.

const CARLING = 'Tuesday 22:00 — HCarling + HStar';
const BM = 'Tuesday 23:30 — XBM';

/** The editor (pane, or the phone's Modal) of `title`, on its Characters tab. */
async function characters(page: Page, title = CARLING): Promise<Locator> {
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: `Edit ${title}` }).click();
  const editor = page.viewportSize()!.width < 600 ? page.getByRole('dialog', { name: title }) : page.getByRole('complementary', { name: 'Weekly timing details' });
  await editor.getByRole('tab', { name: /^Characters/ }).click();
  await expect(editor.getByRole('list', { name: 'Party characters' })).toBeVisible();
  return editor;
}

const row = (editor: Locator, name: string) =>
  editor.locator('.fixedready__row').filter({ has: editor.page().locator('.fixedready__name', { hasText: new RegExp(`^${name.replace(/[()]/g, '\\$&')}$`) }) });
const status = (editor: Locator) => editor.locator('.fixedready__status');

async function timingSlot(page: Page, fixed: string, member: string): Promise<{ picked: string | null; brings: string | null; basis: string | null; version: number }> {
  const response = await page.request.get(`${ADMIN}/api/admin/fixed/${fixed}/characters`);
  const slots = (await response.json()).slots as { member_id: string; picked: string | null; brings: string | null; basis: string | null; version: number }[];
  return slots.find((s) => s.member_id === member)!;
}

test('fixed ready: the main is the default; a pick overrides it for the timing and Undo restores it', async ({ page }) => {
  const editor = await characters(page);
  await expect(editor.getByRole('tab', { name: /^Timing/ })).toHaveAttribute('aria-selected', 'false');
  await expect(editor.getByRole('tab', { name: /^Characters/ })).toHaveAttribute('aria-selected', 'true');
  await expect(editor.locator('.ready__note')).toContainText('Readiness · never blocks a pick or a save');
  await expect(editor.locator('.ready__note')).toContainText('Data based on NEXON Open API');

  const asahi = row(editor, 'Asahi');
  await expect(asahi.locator('.fixedready__basis')).toHaveText('main');
  await expect(asahi).toContainText('2 characters');
  await expect(asahi.getByRole('group', { name: 'Readiness of AsahiDK' })).toContainText(/HCarling\s*Lv ok/);
  const mika = row(editor, 'Mika');
  await expect(mika.locator('.fixedready__basis')).toHaveText('picked for this timing');
  await expect(mika).toContainText('main is MikaNL');
  // Per boss of the timing, against its entry level.
  await expect(editor.locator('.fixedready__hint').filter({ hasText: 'Per boss of the timing' })).toContainText('HCarling Lv. 275, HStar Lv. 280');

  const chip = asahi.getByRole('button', { name: "Asahi's character for this timing: AsahiDK, change" });
  await chip.click();
  const picker = editor.getByRole('dialog', { name: `Asahi's character for ${CARLING}` });
  const options = picker.getByRole('option');
  await expect(options).toHaveText([/AsahiDK\s*main\s*Dark Knight · the default/, /AsahiMech\s*Mechanic · under HStar’s Lv\. 280/]);
  // Under an entry level: tinted, still offered (readiness never blocks a pick).
  await expect(options.nth(1)).not.toHaveAttribute('aria-disabled', 'true');
  await expect(picker).toContainText('Saves at once. Runs made from this timing use it unless the run has its own pick. Choosing AsahiDK goes back to the main.');
  await expect(options.first()).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(picker).toBeHidden();

  await expect(status(editor)).toHaveText(/Asahi brings AsahiMech to this timing\.\s*Undo/);
  await expect(asahi.locator('.fixedready__basis')).toHaveText('picked for this timing');
  await expect(asahi).toContainText('main is AsahiDK');
  await expect(asahi.getByRole('group', { name: 'Readiness of AsahiMech' })).toContainText('under Lv. 280');
  expect(await timingSlot(page, 'f-carling', '1001')).toMatchObject({ picked: 'ch-s6', basis: 'timing_pick', version: 1 });
  // Saved at once: the form has nothing to save, and Cancel leaves the pick.
  await expect(asahi.getByRole('button', { name: /AsahiMech, change$/ })).toBeFocused();

  await status(editor).getByRole('button', { name: 'Undo' }).click();
  await expect(asahi.locator('.fixedready__basis')).toHaveText('main');
  await expect(status(editor)).toContainText('Asahi is back on AsahiDK (main).');
  expect(await timingSlot(page, 'f-carling', '1001')).toMatchObject({ picked: null, brings: 'ch-s5', basis: 'main' });

  // Choosing the main again clears a pick: Mika goes back to MikaNL.
  await mika.getByRole('button', { name: /MikaBish, change$/ }).click();
  const mikaPicker = editor.getByRole('dialog', { name: `Mika's character for ${CARLING}` });
  await expect(mikaPicker.getByRole('option', { name: /MikaBish/ })).toHaveAttribute('aria-selected', 'true');
  await expect(mikaPicker.getByRole('option', { name: /MikaBish/ })).toContainText(/picked 28 Sep by Asahi/);
  await expect(mikaPicker).toContainText('1 retired character not offered.');
  await mikaPicker.getByRole('option', { name: /MikaNL/ }).click();
  await expect(status(editor)).toContainText('Mika is back on MikaNL (main).');
  expect(await timingSlot(page, 'f-carling', '1003')).toMatchObject({ picked: null, basis: 'main' });
});

test('fixed ready: a stale pick (409) shows whose pick is saved now', async ({ page }) => {
  const editor = await characters(page);
  const asahi = row(editor, 'Asahi');
  await asahi.getByRole('button', { name: /AsahiDK, change$/ }).click();
  const picker = editor.getByRole('dialog', { name: `Asahi's character for ${CARLING}` });
  await expect(picker.getByRole('option').first()).toBeFocused();
  // Another admin picks AsahiMech meanwhile.
  const other = await page.request.put(`${ADMIN}/api/admin/fixed/f-carling/characters/1001`, {
    headers: await csrf(page.request),
    data: { character_id: 'ch-s6', version: 0 },
  });
  expect(other.status()).toBe(200);
  await picker.getByRole('option', { name: /AsahiMech/ }).click();
  const line = status(editor);
  await expect(line).toHaveText(/changed Asahi's pick a moment ago, so yours wasn't saved\. Showing theirs: AsahiMech \(picked for this timing\)\. Pick again to change it\./);
  await expect(line.getByRole('button', { name: 'Undo' })).toHaveCount(0);
  await expect(asahi.getByRole('button', { name: /AsahiMech, change$/ })).toBeVisible();
  expect((await timingSlot(page, 'f-carling', '1001')).version).toBe(1);
});

test('fixed ready: several characters and no main bring nothing until picked; no characters links to Members', async ({ page }) => {
  const editor = await characters(page, BM);
  const sora = row(editor, 'Sora');
  await expect(sora.locator('.fixedready__basis')).toHaveText('no main');
  await expect(sora).toContainText('2 characters');
  await expect(sora).toContainText('Readiness shows once a character is picked. Set a main on Members to make one the default everywhere.');
  await expect(sora.getByRole('group')).toHaveCount(0);

  const minato = row(editor, 'Minato');
  await expect(minato.locator('.fixedready__basis')).toHaveText('no characters');
  await expect(minato.getByRole('button')).toHaveCount(0);
  await expect(minato.getByRole('link', { name: 'Minato has no characters yet: add them on Members' })).toHaveAttribute('href', '/members?open=1012');

  const kaito = row(editor, 'Kaito');
  await expect(kaito.locator('.fixedready__basis')).toHaveText('only character');
  await expect(kaito.getByRole('group', { name: 'Readiness of KaitoLyn' })).toContainText('Lv unknown');

  await sora.getByRole('button', { name: "Pick Sora's character for this timing: 2 to choose from" }).click();
  const picker = editor.getByRole('dialog', { name: `Sora's character for ${BM}` });
  await expect(picker).toContainText('Sora has no main, so nothing is brought until you pick. This pick is for this timing only.');
  await expect(picker.getByRole('option', { selected: true })).toHaveCount(0);
  await picker.getByRole('option', { name: /SoraLumi/ }).click();
  await expect(status(editor)).toContainText('Sora brings SoraLumi to this timing.');
  await expect(sora.locator('.fixedready__basis')).toHaveText('picked for this timing');
  await expect(sora.getByRole('group', { name: 'Readiness of SoraLumi' })).toBeVisible();
  expect(await timingSlot(page, 'f-bm', '1008')).toMatchObject({ picked: 'ch-s12', basis: 'timing_pick' });
});

test('fixed ready: a scouter reading replaces CP, once labelled', async ({ page }) => {
  const headers = await csrf(page.request);
  const { job } = await (await page.request.post(`${ADMIN}/api/admin/estimates`, { headers, data: { run_id: 'r-carling' } })).json();
  await expect.poll(async () => (await (await page.request.get(`${ADMIN}/api/admin/estimates/${job.id}`)).json()).state).toBe('done');
  const editor = await characters(page);
  await expect(editor.locator('.ready__estimate')).toHaveText('Clear-time estimate · For planning only, not eligibility.');
  const asahi = row(editor, 'Asahi').getByRole('group', { name: 'Readiness of AsahiDK' });
  await expect(asahi.locator('.ready__values')).toHaveText('Lv. 284');
  await expect(asahi.locator('.ready__scouter-at')).toContainText(/^Estimate · \d{1,2} \w{3} \d{2}:\d{2}$/);
  await expect(asahi.locator('.ready__scouter')).toHaveCount(2);
  await expect(asahi.locator('.ready__scouter').first()).toHaveText(/^Estimate \d+%( · ~\d+ min)? · .+$/);
});

test('fixed ready: either module off draws today’s editor and asks nothing', async ({ page }) => {
  for (const off of [{ roster: false }, { character_data: false }]) {
    await page.request.post(`${ADMIN}/api/admin/reset`);
    await page.request.post(`${ADMIN}/__mock/modules`, { data: off });
    const asked: string[] = [];
    page.on('request', (r) => /\/api\/admin\/fixed\/.*(characters|readiness)/.test(r.url()) && asked.push(r.url()));
    await page.goto(`${ADMIN}/fixed?sw=off`);
    await page.getByRole('button', { name: `Edit ${CARLING}` }).click();
    const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
    await expect(editor.getByRole('checkbox', { name: 'Hard Carling' })).toBeChecked();
    await expect(editor.getByRole('tablist')).toHaveCount(0);
    await expect(editor.locator('.fixedready')).toHaveCount(0);
    expect((await editor.boundingBox())!.width).toBeLessThanOrEqual(421);
    await page.waitForTimeout(300);
    expect(asked, JSON.stringify(off)).toEqual([]);
    page.removeAllListeners('request');
  }
});

test('fixed ready: a new timing previews readiness from the form, picks closed until it is added', async ({ page }) => {
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: 'Add a weekly timing' }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  // The difficulty pill is the label of a visually hidden checkbox.
  await editor.getByRole('group', { name: 'Carling difficulties' }).getByText('HARD').click();
  await expect(editor.getByRole('checkbox', { name: 'Hard Carling' })).toBeChecked();
  await editor.getByRole('checkbox', { name: 'Mika' }).check();
  await editor.getByRole('button', { name: /^Show \d+ more members?$/ }).click();
  await editor.getByRole('checkbox', { name: 'Ren (2)' }).check();
  const previews: unknown[] = [];
  page.on('request', (r) => r.url().endsWith('/api/admin/fixed/readiness') && previews.push(r.postDataJSON()));
  await editor.getByRole('tab', { name: /^Characters/ }).click();
  await expect(editor.locator('.fixedready__box').first()).toContainText('Picks open once the timing is added');
  const mika = row(editor, 'Mika');
  await expect(mika.locator('.fixedready__basis')).toHaveText('main');
  const chip = mika.getByRole('button', { name: "Mika's character: MikaNL. Picks open once the timing is added." });
  await expect(chip).toHaveAttribute('aria-disabled', 'true');
  // aria-disabled (still focusable, so its reason is read): a press opens nothing.
  await chip.dispatchEvent('click');
  await expect(editor.getByRole('dialog')).toHaveCount(0);
  // One boss: its verdict is not named.
  await expect(mika.getByRole('group', { name: 'Readiness of MikaNL' }).getByRole('listitem')).toHaveText([/^\s*Lv ok/]);
  await expect(row(editor, 'Ren (2)').locator('.fixedready__basis')).toHaveText('no characters');
  expect(previews.at(-1)).toEqual({ fixed_run_id: null, bosses: ['HCarling'], participants: ['1003', '1013'] });
  // Nothing new to save: the timing's own Add stays the one write.
  await expect(editor.getByRole('button', { name: 'Add timing' })).toBeEnabled();
});

test('fixed ready: a party member added in the form is drawn from it, their pick closed until saved', async ({ page }) => {
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: `Edit ${CARLING}` }).click();
  const editor = page.getByRole('complementary', { name: 'Weekly timing details' });
  await editor.getByRole('button', { name: /^Show \d+ more members?$/ }).click();
  await editor.getByRole('checkbox', { name: 'Kaito' }).check();
  await editor.getByRole('tab', { name: /^Characters/ }).click();
  await expect(editor.getByText("The party changed and isn’t saved: Kaito is drawn from the form. Save to pick a character for Kaito.")).toBeVisible();
  const kaito = row(editor, 'Kaito');
  await expect(kaito.locator('.fixedready__basis--new')).toHaveText('not saved yet');
  await expect(kaito.getByRole('button', { name: "Kaito's character: KaitoLyn. Save the party to pick for Kaito." })).toHaveAttribute('aria-disabled', 'true');
  await expect(kaito.getByRole('group', { name: 'Readiness of KaitoLyn' })).toContainText(/HCarling\s*Lv unknown/);
  // Saved members keep their picks.
  await expect(row(editor, 'Asahi').getByRole('button', { name: /AsahiDK, change$/ })).not.toHaveAttribute('aria-disabled', 'true');
});

test('fixed ready: readiness that fails to load keeps the rows and picks, and Try again reads it', async ({ page }) => {
  let fail = true;
  await page.route('**/api/admin/fixed/*/readiness', (route) =>
    fail ? route.fulfill({ status: 503, contentType: 'application/json', body: '{"error":"unavailable","message":"The server didn’t answer."}' }) : route.continue(),
  );
  const editor = await characters(page);
  const alert = editor.locator('.fixedready').getByRole('alert');
  await expect(alert).toContainText('Readiness couldn’t load');
  await expect(alert).toContainText('Picks and the timing still save.');
  await expect(row(editor, 'Mika').locator('.fixedready__basis')).toHaveText('picked for this timing');
  await expect(row(editor, 'Mika').getByRole('button', { name: /MikaBish, change$/ })).toBeEnabled();
  await expect(editor.getByRole('group', { name: /^Readiness of / })).toHaveCount(0);
  fail = false;
  await alert.getByRole('button', { name: 'Try again' }).click();
  await expect(alert).toBeHidden();
  await expect(row(editor, 'Mika').getByRole('group', { name: 'Readiness of MikaBish' })).toBeVisible();
});

test('fixed ready: runs made from the timing read its pick live, tagged "from timing"', async ({ page }) => {
  const editor = await characters(page);
  await row(editor, 'Mika').getByRole('button', { name: /MikaBish, change$/ }).click();
  await editor.getByRole('dialog', { name: `Mika's character for ${CARLING}` }).getByRole('option', { name: /Mikadere/ }).click();
  await expect(status(editor)).toContainText('Mika brings Mikadere to this timing.');

  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('.board').evaluate((board) => (board.scrollLeft = board.scrollWidth));
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  await page.getByRole('complementary', { name: 'HCarling + HStar' }).getByRole('button', { name: 'Open in a larger view' }).click();
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  const mika = sheet.locator('.slot').filter({ has: page.locator('.slot__name', { hasText: /^Mika$/ }) });
  await expect(mika.getByRole('button', { name: "Mika's character on HCarling + HStar: Mikadere, from timing. Change" })).toBeVisible();
  await expect(mika.locator('.slot__basis')).toHaveText('from timing');
  // Asahi's own run pick still wins over the default.
  const asahi = sheet.locator('.slot').filter({ has: page.locator('.slot__name', { hasText: /^Asahi$/ }) });
  await expect(asahi.getByRole('button', { name: "Asahi's character on HCarling + HStar: AsahiDK. Change" })).toBeVisible();
  await expect(asahi.locator('.slot__basis')).toHaveCount(0);
  const tsubame = sheet.locator('.slot').filter({ has: page.locator('.slot__name', { hasText: /^Tsubame$/ }) });
  await expect(tsubame.locator('.slot__basis')).toHaveText('main');
});

test('fixed ready: the phone picks from a bottom sheet with 44 px controls', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const editor = await characters(page);
  const tab = editor.getByRole('tab', { name: /^Characters/ });
  expect((await tab.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  const chip = row(editor, 'Mika').getByRole('button', { name: /MikaBish, change$/ });
  expect((await chip.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  await chip.click();
  const sheet = page.getByRole('dialog', { name: "Mika's character for this timing" });
  await expect(sheet.getByRole('radio', { name: /MikaBish/ })).toBeChecked();
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  for (const option of await sheet.locator('.charpick__option').all()) expect((await option.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  await expect(sheet).toContainText('MikaNL goes back to the main.');
  await sheet.getByRole('radio', { name: /Mikadere/ }).check();
  await sheet.getByRole('button', { name: 'Use Mikadere' }).click();
  await expect(sheet).toBeHidden();
  await expect(row(editor, 'Mika').getByRole('button', { name: /Mikadere, change$/ })).toBeFocused();
  await expect(status(editor)).toContainText('Mika brings Mikadere to this timing.');
});

// For review (not assertions): each board's screen, named after the board.
const CAPTURES = '/private/tmp/fr-captures';
test('fixed ready captures', async ({ page }) => {
  test.skip(!process.env.KANADE_FIXED_READY_CAPTURES, 'set KANADE_FIXED_READY_CAPTURES=1 to write the review captures');
  test.setTimeout(120_000);
  mkdirSync(CAPTURES, { recursive: true });
  const shot = async (name: string) => {
    await settle(page);
    await page.screenshot({ path: `${CAPTURES}/${name}.png`, animations: 'disabled' });
  };
  const reset = () => page.request.post(`${ADMIN}/api/admin/reset`);
  const lookUp = async () => {
    const { job } = await (await page.request.post(`${ADMIN}/api/admin/estimates`, { headers: await csrf(page.request), data: { run_id: 'r-carling' } })).json();
    await expect.poll(async () => (await (await page.request.get(`${ADMIN}/api/admin/estimates/${job.id}`)).json()).state).toBe('done');
  };
  for (const [prefix, size] of [
    ['B_', { width: 1280, height: 800 }],
    ['B_Phone', { width: 390, height: 844 }],
  ] as const) {
    await page.setViewportSize(size);
    await reset();
    await lookUp();
    let editor = await characters(page);
    await shot(`${prefix}FixedReady`);
    await editor.locator('.fixedsheet__content').evaluate((el) => (el.scrollTop = el.scrollHeight));
    await shot(`${prefix}FixedReadyEnd`);
    await editor.locator('.fixedsheet__content').evaluate((el) => (el.scrollTop = 0));
    await row(editor, 'Mika').getByRole('button', { name: /MikaBish, change$/ }).click();
    await shot(`${prefix}FixedReadyPick`);
    await page.keyboard.press('Escape');

    // States 1: a new timing; an unsaved party change; no main; a pick saved; a stale pick.
    await page.goto(`${ADMIN}/fixed?sw=off`);
    await page.getByRole('button', { name: 'Add a weekly timing' }).click();
    editor = prefix === 'B_' ? page.getByRole('complementary', { name: 'Weekly timing details' }) : page.getByRole('dialog', { name: 'Add a weekly timing' });
    await editor.getByRole('group', { name: 'Carling difficulties' }).getByText('HARD').click();
    await editor.getByRole('checkbox', { name: 'Mika' }).check();
    await editor.getByRole('button', { name: /^Show \d+ more members?$/ }).click();
    await editor.getByRole('checkbox', { name: 'Ren (2)' }).check();
    await editor.getByRole('tab', { name: /^Characters/ }).click();
    await expect(row(editor, 'Mika').getByRole('group')).toBeVisible();
    await shot(`${prefix}FixedReadyStates-new-timing`);
    editor = await characters(page, BM);
    await row(editor, 'Sora').getByRole('button', { name: /^Pick Sora's character/ }).click();
    await shot(`${prefix}FixedReadyStates-no-main`);
    await page.keyboard.press('Escape');
    editor = await characters(page);
    await row(editor, 'Asahi').getByRole('button', { name: /AsahiDK, change$/ }).click();
    if (prefix === 'B_') await editor.getByRole('option', { name: /AsahiMech/ }).click();
    else {
      await page.getByRole('radio', { name: /AsahiMech/ }).check();
      await page.getByRole('button', { name: 'Use AsahiMech' }).click();
    }
    await expect(status(editor)).toContainText('Undo');
    await shot(`${prefix}FixedReadyStates-pick-saved`);
    await row(editor, 'Asahi').getByRole('button', { name: /AsahiMech, change$/ }).click();
    await page.request.put(`${ADMIN}/api/admin/fixed/f-carling/characters/1001`, { headers: await csrf(page.request), data: { character_id: null, version: 1 } });
    if (prefix === 'B_') await editor.getByRole('option', { name: /AsahiDK/ }).click();
    else {
      await page.getByRole('radio', { name: /AsahiDK/ }).check();
      await page.getByRole('button', { name: 'Use AsahiDK' }).click();
    }
    await shot(`${prefix}FixedReadyStates-stale`);
    if (prefix === 'B_') {
      await page.goto(`${ADMIN}/fixed?sw=off`);
      await page.getByRole('button', { name: `Edit ${CARLING}` }).click();
      editor = page.getByRole('complementary', { name: 'Weekly timing details' });
      await editor.getByRole('button', { name: /^Show \d+ more members?$/ }).click();
      await editor.getByRole('checkbox', { name: 'Kaito' }).check();
      await editor.getByRole('tab', { name: /^Characters/ }).click();
      await editor.locator('.fixedsheet__content').evaluate((el) => (el.scrollTop = el.scrollHeight));
      await shot('B_FixedReadyStates-unsaved-party');
    }

    // States 2: couldn't load; loading; the run sheet of a run made from the timing.
    await page.route('**/api/admin/fixed/*/readiness', (route) => route.fulfill({ status: 503, contentType: 'application/json', body: '{"error":"unavailable","message":"The server didn’t answer."}' }));
    await characters(page);
    await shot(`${prefix}FixedReadyStates2-error`);
    await page.unroute('**/api/admin/fixed/*/readiness');
    await page.route('**/api/admin/fixed/*/readiness', () => {});
    await page.goto(`${ADMIN}/fixed?sw=off`);
    await page.getByRole('button', { name: `Edit ${CARLING}` }).click();
    await page.getByRole('tab', { name: /^Characters/ }).click();
    await expect(page.getByText('Reading levels and CP…')).toBeVisible();
    await shot(`${prefix}FixedReadyStates2-loading`);
    await page.unroute('**/api/admin/fixed/*/readiness');
    await reset();
    await page.goto(`${ADMIN}/?sw=off`);
    if (prefix === 'B_') await page.locator('.board').evaluate((board) => (board.scrollLeft = board.scrollWidth));
    await page.locator('[data-run="r-carling"] .plan-card__open').click();
    if (prefix === 'B_') await page.getByRole('complementary', { name: 'HCarling + HStar' }).getByRole('button', { name: 'Open in a larger view' }).click();
    const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
    await expect(sheet.locator('.slot__basis').filter({ hasText: 'from timing' })).toBeVisible();
    await sheet.locator('.slot__basis').filter({ hasText: 'from timing' }).scrollIntoViewIfNeeded();
    await shot(`${prefix}FixedReadyStates2-run-sheet`);

    // Module off: today's editor.
    await reset();
    await page.request.post(`${ADMIN}/__mock/modules`, { data: { character_data: false } });
    await page.goto(`${ADMIN}/fixed?sw=off`);
    await page.getByRole('button', { name: `Edit ${CARLING}` }).click();
    await shot(prefix === 'B_' ? 'B_FixedReadyOff' : 'B_PhoneFixedReadyStates-module-off');
  }
});
