import { mkdirSync } from 'node:fs';
import type { Page } from '@playwright/test';
import { ADMIN, choose, csrf, expect, setModules, test, useLook } from './support';

// The character roster (module `roster`; boards in docs/research/2026-10-11-roster-mockups):
// the Members sheet's Characters tab over the e2e server's seed (Mika: MikaNL main,
// MikaBish, Mikadere, MikaHero retired; Hotaru: Hotaru7; Ren (2): none).

/** Mika's snowflake (the seed's member ids are Discord ids). */
const MIKA = '100000000000001003';

const toast = (page: Page, text: string | RegExp) => page.locator('.toast:not(.is-leaving)').filter({ hasText: text });

// The roster module alone: with module `character_data` off the Add dialog keeps the manual
// Class and Level (MapleSEA adds with NEXON connected are `add-character.spec.ts`).
test.beforeEach(async () => {
  await setModules({ character_data: false });
});

async function characters(page: Page, who: RegExp = /^Mika/) {
  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: who }).first().click();
  const pane = page.getByRole('complementary', { name: 'Member details' });
  await pane.getByRole('tab', { name: /^Characters/ }).click();
  return pane;
}

async function more(page: Page, ign: string, item: string) {
  await page.getByRole('button', { name: `More actions for ${ign}` }).click();
  await page.getByRole('menu', { name: ign }).getByRole('menuitem', { name: item }).click();
}

test('roster: the tab lists the main first, the retired apart, and keeps the profile', async ({ page }) => {
  const pane = await characters(page);
  await expect(pane.getByRole('tab', { name: /^Characters/ })).toHaveAttribute('aria-selected', 'true');
  await expect(pane.getByRole('tab', { name: /^Characters/ })).toContainText('3');
  const active = pane.getByRole('list', { name: "Mika's characters" }).getByRole('listitem');
  await expect(active).toHaveCount(3);
  await expect(active.first()).toContainText('MikaNL');
  await expect(active.first()).toContainText('main');
  await expect(pane.getByRole('list', { name: "Mika's retired characters" })).toContainText('MikaHero');
  await expect(pane.getByText('Characters are never sent to the chatbot.')).toBeVisible();
  // The main's Retire and Delete wait, explained.
  await pane.getByRole('button', { name: 'More actions for MikaNL' }).press('Enter');
  const menu = pane.getByRole('menu', { name: 'MikaNL' });
  await expect(menu.getByRole('menuitem', { name: 'Retire' })).toBeFocused();
  await expect(menu.getByRole('menuitem', { name: 'Retire' })).toHaveAttribute('aria-disabled', 'true');
  await expect(menu).toContainText('Make another character main first.');
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();
  await expect(pane.getByRole('button', { name: 'More actions for MikaNL' })).toBeFocused();
  await expect(pane, 'Escape in the menu keeps the pane').toBeVisible();
  await pane.getByRole('tab', { name: 'Profile' }).click();
  await expect(pane.getByText('Chat aliases')).toBeVisible();
});

test('roster: add, edit, make main with undo, retire with undo', async ({ page }) => {
  const pane = await characters(page);
  await pane.getByRole('button', { name: 'Add character' }).click();
  const add = page.getByRole('dialog', { name: 'Add character' });
  await add.getByRole('textbox', { name: 'IGN' }).fill('MikaKain');
  await choose(add.getByRole('combobox', { name: 'Class' }), { label: 'Kain' });
  await add.getByRole('textbox', { name: 'Level' }).fill('250');
  await add.getByRole('button', { name: 'Add character' }).click();
  await expect(add).toBeHidden();
  await expect(toast(page, 'Added MikaKain.')).toBeVisible();
  await expect(pane.getByRole('list', { name: "Mika's characters" })).toContainText('MikaKain');

  await pane.getByRole('button', { name: 'Edit MikaKain' }).click();
  const edit = page.getByRole('dialog', { name: 'Edit MikaKain' });
  await edit.getByRole('textbox', { name: 'Level' }).fill('251');
  await edit.getByRole('button', { name: 'Save' }).click();
  await expect(edit).toBeHidden();
  await expect(pane.getByRole('listitem').filter({ hasText: 'MikaKain' })).toContainText('251');

  await more(page, 'MikaBish', 'Make main');
  await expect(toast(page, "MikaBish is Mika's main now.")).toBeVisible();
  await expect(pane.getByRole('list', { name: "Mika's characters" }).getByRole('listitem').first()).toContainText('MikaBish');
  await toast(page, "MikaBish is Mika's main now.").getByRole('button', { name: 'Undo' }).click();
  await expect(toast(page, "MikaNL is Mika's main again.")).toBeVisible();
  await expect(pane.getByRole('list', { name: "Mika's characters" }).getByRole('listitem').first()).toContainText('MikaNL');

  await more(page, 'Mikadere', 'Retire');
  await expect(toast(page, 'Retired Mikadere.')).toBeVisible();
  await expect(pane.getByRole('list', { name: "Mika's retired characters" })).toContainText('Mikadere');
  await toast(page, 'Retired Mikadere.').getByRole('button', { name: 'Undo' }).click();
  await expect(pane.getByRole('list', { name: "Mika's characters" })).toContainText('Mikadere');
});

test('roster: the server’s refusals land on their fields and keep the input', async ({ page }) => {
  const pane = await characters(page);
  await pane.getByRole('button', { name: 'Edit MikaBish' }).click();
  const edit = page.getByRole('dialog', { name: 'Edit MikaBish' });
  const ign = edit.getByRole('textbox', { name: 'IGN' });
  await ign.fill('hotaru7');
  await edit.getByRole('button', { name: 'Save' }).click();
  await expect(edit.getByText('“hotaru7” is already on the roster in MapleSEA.')).toBeVisible();
  await expect(ign).toHaveAttribute('aria-invalid', 'true');
  await expect(ign).toBeFocused();
  await edit.getByRole('textbox', { name: 'Level' }).fill('312');
  await edit.getByRole('button', { name: 'Save' }).click();
  await expect(edit.getByRole('alert')).toContainText('Fix 2 fields to save:');
  await expect(edit.getByText('Level is 1 to 300.')).toBeVisible();
  await expect(ign).toHaveValue('hotaru7');

  // Someone saves it meanwhile: 409 stale, the edits stay and the next Save wins.
  await ign.fill('MikaBishop');
  await edit.getByRole('textbox', { name: 'Level' }).fill('282');
  const list = await page.request.get(`${ADMIN}/api/admin/members/${MIKA}/characters`);
  const bish = (await list.json()).characters.find((c: { ign: string }) => c.ign === 'MikaBish');
  expect((await page.request.patch(`${ADMIN}/api/admin/characters/${bish.id}`, { data: { version: bish.version, level: 283 }, headers: await csrf(page.request) })).ok()).toBe(true);
  await edit.getByRole('button', { name: 'Save' }).click();
  await expect(edit.getByRole('alert')).toContainText('changed while you were editing it');
  await expect(ign).toHaveValue('MikaBishop');
  await edit.getByRole('button', { name: 'Save' }).click();
  await expect(edit).toBeHidden();
  await expect(pane.getByRole('list', { name: "Mika's characters" })).toContainText('MikaBishop');
});

test('roster: delete is confirmed from Cancel, offers Retire instead, and refuses the main (409)', async ({ page }) => {
  const pane = await characters(page);
  await more(page, 'Mikadere', 'Delete…');
  let confirm = page.getByRole('alertdialog', { name: 'Delete Mikadere?' });
  await expect(confirm.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await expect(confirm).toContainText('History keeps an audit row');
  await confirm.getByRole('button', { name: 'Retire instead' }).click();
  await expect(confirm).toBeHidden();
  await expect(pane.getByRole('list', { name: "Mika's retired characters" })).toContainText('Mikadere');

  await more(page, 'MikaHero', 'Delete…');
  confirm = page.getByRole('alertdialog', { name: 'Delete MikaHero?' });
  await confirm.getByRole('button', { name: 'Delete' }).click();
  await expect(toast(page, 'Deleted MikaHero. History keeps the audit row.')).toBeVisible();
  await expect(pane.getByText('MikaHero')).toHaveCount(0);

  // MikaBish becomes the main elsewhere while its confirmation is open: the server refuses.
  await more(page, 'MikaBish', 'Delete…');
  confirm = page.getByRole('alertdialog', { name: 'Delete MikaBish?' });
  const list = await page.request.get(`${ADMIN}/api/admin/members/${MIKA}/characters`);
  const bish = (await list.json()).characters.find((c: { ign: string }) => c.ign === 'MikaBish');
  await page.request.patch(`${ADMIN}/api/admin/characters/${bish.id}`, { data: { version: bish.version, role: 'main' }, headers: await csrf(page.request) });
  await confirm.getByRole('button', { name: 'Delete' }).click();
  await expect(confirm.getByRole('alert')).toContainText("MikaBish is Mika's main now");
  await confirm.getByRole('button', { name: 'Cancel' }).click();
  await expect(pane.getByRole('button', { name: 'More actions for MikaBish' })).toBeFocused();
});

test('roster: a member with no characters gets the empty state, and the first one is the main', async ({ page }) => {
  const pane = await characters(page, /^Ren.*ren2/);
  await expect(pane.getByRole('heading', { name: 'No characters yet' })).toBeVisible();
  await pane.getByRole('button', { name: 'Add character' }).click();
  const add = page.getByRole('dialog', { name: 'Add character' });
  await expect(add.getByRole('button', { name: 'Alt' })).toBeDisabled();
  await add.getByRole('textbox', { name: 'IGN' }).fill('RenTwo');
  await choose(add.getByRole('combobox', { name: 'Class' }), { label: 'Zero' });
  await add.getByRole('button', { name: 'Add character' }).click();
  await expect(pane.getByRole('listitem').filter({ hasText: 'RenTwo' })).toContainText('main');
});

test('roster: a failed read shows Try again and the Profile tab still works', async ({ page }) => {
  await page.route(`**/api/admin/members/${MIKA}/characters`, (route) => route.fulfill({ status: 503, json: { error: 'unavailable', message: 'The server did not answer.' } }));
  const pane = await characters(page);
  await expect(pane.getByRole('heading', { name: 'Couldn’t load characters' })).toBeVisible();
  await expect(pane.getByRole('button', { name: 'Try again' })).toBeVisible();
});

test('roster: module off, no tab and no character requests', async ({ page }) => {
  await setModules({ roster: false });
  const asked: string[] = [];
  // API requests only: the shared `characters` module may load as a script chunk of that name.
  page.on('request', (r) => /\/api\/admin\/.*characters/.test(r.url()) && asked.push(r.url()));
  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: /^Mika\b/ }).click();
  const pane = page.getByRole('complementary', { name: 'Member details' });
  await expect(pane.getByText('Chat aliases')).toBeVisible();
  await expect(pane.getByRole('tablist')).toHaveCount(0);
  await expect(page.getByRole('tab', { name: /^Characters/ })).toHaveCount(0);
  expect(asked).toEqual([]);
});

test('roster: phone sheet, 44 px targets and Add as a bottom sheet over it', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: /^Mika\b/ }).click();
  const sheet = page.getByRole('dialog', { name: 'Mika', exact: true });
  await sheet.getByRole('tab', { name: /^Characters/ }).click();
  const edit = sheet.getByRole('button', { name: 'Edit MikaNL' });
  await expect(edit).toBeVisible();
  // Measured at rest, not mid-enter.
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  expect((await edit.boundingBox())!.height).toBeGreaterThanOrEqual(44);
  await sheet.getByRole('button', { name: 'Add character' }).click();
  const add = page.getByRole('dialog', { name: 'Add character' });
  await add.getByRole('textbox', { name: 'IGN' }).fill('MikaKain');
  await add.getByRole('textbox', { name: 'Level' }).fill('0');
  await add.getByRole('button', { name: 'Add character' }).click();
  await expect(add.getByRole('alert')).toContainText('Fix 2 fields to save:');
  await add.getByRole('button', { name: 'Cancel' }).click();
  await expect(sheet, 'closing the sheet over it keeps the member sheet').toBeVisible();
});

// For user approval (not assertions): desktop and phone, blossom and marigold.
const CAPTURES = '/private/tmp/roster-ui/captures';
for (const colorway of ['blossom', 'marigold']) {
  test(`roster capture: ${colorway}`, async ({ page }) => {
    test.skip(!process.env.KANADE_ROSTER_CAPTURES, 'set KANADE_ROSTER_CAPTURES=1 to write the review captures');
    mkdirSync(CAPTURES, { recursive: true });
    await useLook(page, colorway);
    const shot = (name: string) => page.screenshot({ path: `${CAPTURES}/${colorway}-${name}.png`, animations: 'disabled' });
    const pane = await characters(page);
    await pane.getByRole('button', { name: 'More actions for MikaBish' }).click();
    await shot('desktop-1-characters');
    await page.keyboard.press('Escape');
    await pane.getByRole('button', { name: 'Edit MikaBish' }).click();
    const edit = page.getByRole('dialog', { name: 'Edit MikaBish' });
    await edit.getByRole('textbox', { name: 'IGN' }).fill('hotaru7');
    await edit.getByRole('button', { name: 'Save' }).click();
    await edit.getByRole('textbox', { name: 'Level' }).fill('312');
    await edit.getByRole('button', { name: 'Save' }).click();
    await expect(edit.getByRole('alert')).toBeVisible();
    await shot('desktop-2-edit');
    await edit.getByRole('button', { name: 'Cancel' }).click();
    await more(page, 'MikaBish', 'Delete…');
    await expect(page.getByRole('alertdialog')).toBeVisible();
    await shot('desktop-3-delete');
    await page.getByRole('alertdialog').getByRole('button', { name: 'Cancel' }).click();
    await more(page, 'Mikadere', 'Retire');
    await expect(toast(page, 'Retired Mikadere.')).toBeVisible();
    await shot('desktop-4-retired-toast');
    await characters(page, /^Ren.*ren2/);
    await shot('desktop-5-empty');

    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`${ADMIN}/members?sw=off`);
    await page.getByRole('button', { name: /^Mika\b/ }).click();
    const sheet = page.getByRole('dialog', { name: 'Mika', exact: true });
    await sheet.getByRole('tab', { name: /^Characters/ }).click();
    await shot('phone-1-characters');
    await sheet.getByRole('button', { name: 'Add character' }).click();
    const add = page.getByRole('dialog', { name: 'Add character' });
    await add.getByRole('textbox', { name: 'IGN' }).fill('MikaKain');
    await choose(add.getByRole('combobox', { name: 'Class' }), { label: 'Kain' });
    await add.getByRole('textbox', { name: 'Level' }).fill('0');
    await add.getByRole('button', { name: 'Add character' }).click();
    await expect(add.getByText('Level is 1 to 300.')).toBeVisible();
    await shot('phone-2-add');
    await add.getByRole('button', { name: 'Cancel' }).click();
    await more(page, 'MikaBish', 'Delete…');
    await expect(page.getByRole('alertdialog')).toBeVisible();
    await shot('phone-3-delete');
  });
}
