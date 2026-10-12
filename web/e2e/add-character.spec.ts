import { mkdirSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, choose, expect, settle, test } from './support';

// Add character with NEXON connected (module `character_data`, the mock's `ok` health): IGN,
// region and role only; Look up asks the mock, which answers by IGN prefix (`Nope…` not
// found, `Busy…` held, `Offline…` not connected, anything else found); nothing is stored and
// saving makes the member's pending link. Other regions keep the manual Class and Level.

const ATTRIBUTION = 'Data based on NEXON Open API';
const SHOWN_ONLY = 'Shown only; saved after the member confirms in Discord.';

async function openAdd(page: Page): Promise<Locator> {
  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: /^Mika/ }).first().click();
  const phone = page.viewportSize()!.width < 600;
  const pane = phone ? page.getByRole('dialog', { name: /^Mika/ }).first() : page.getByRole('complementary', { name: 'Member details' });
  await pane.getByRole('tab', { name: /^Characters/ }).click();
  await pane.getByRole('button', { name: 'Add character' }).click();
  const add = page.getByRole('dialog', { name: 'Add character' });
  await expect(add.getByRole('button', { name: 'Look up' })).toBeVisible();
  return add;
}

const card = (add: Locator) => add.getByRole('region', { name: 'NEXON lookup' });

test('add character: MapleSEA asks no class or level; Look up shows NEXON, shown only', async ({ page }) => {
  const asked: string[] = [];
  page.on('request', (r) => r.url().includes('/api/admin/characters/lookup') && r.method() === 'POST' && asked.push(r.postData() ?? ''));
  const add = await openAdd(page);
  await expect(add.getByRole('combobox', { name: 'Class' })).toHaveCount(0);
  await expect(add.getByRole('textbox', { name: 'Level' })).toHaveCount(0);
  await expect(add.getByText('Kanade doesn’t message Mika; tell them.')).toBeVisible();
  const ign = add.getByRole('textbox', { name: 'IGN' });
  await ign.fill('MikaKain');
  // Enter looks up (it does not save).
  await ign.press('Enter');
  const found = card(add);
  await expect(found.locator('.status-chip')).toHaveText('found');
  await expect(found).toContainText(/Lv\. \d+ · \w/);
  await expect(found).toContainText('MikaKain · as of');
  await expect(found.getByText(ATTRIBUTION, { exact: true })).toBeVisible();
  await expect(found.getByText(SHOWN_ONLY)).toBeVisible();
  await expect(add).toBeVisible();
  expect(asked).toHaveLength(1);
  // Typing on forgets the answer.
  await ign.fill('MikaKai');
  await expect(card(add)).toHaveCount(0);
  await ign.fill('MikaKain');

  // Saving sends no class or level and asks Mika to link it.
  const posted = page.waitForRequest((r) => r.url().endsWith('/api/admin/members/1003/characters') && r.method() === 'POST');
  await add.getByRole('button', { name: 'Add character' }).click();
  const body = (await posted).postDataJSON();
  expect(body).toEqual({ ign: 'MikaKain', world: 'msea', role: 'alt' });
  await expect(add).toBeHidden();
  await expect(page.locator('.toast:not(.is-leaving)').filter({ hasText: 'Added MikaKain. Asked Mika to link it' })).toBeVisible();
  const list = await (await page.request.get(`${ADMIN}/api/admin/members/1003/characters`)).json();
  const row = list.characters.find((c: { ign: string }) => c.ign === 'MikaKain');
  expect([row.class, row.level]).toEqual([null, null]);
  const link = await (await page.request.get(`${ADMIN}/api/admin/characters/${row.id}/link`)).json();
  expect(link.state).toBe('pending');
});

test('add character: not found, held and not connected show inline', async ({ page }) => {
  const add = await openAdd(page);
  const ign = add.getByRole('textbox', { name: 'IGN' });
  await ign.fill('NopeMika');
  await add.getByRole('button', { name: 'Look up' }).click();
  await expect(card(add).locator('.status-chip')).toHaveText('not found');
  await expect(card(add)).toContainText('NEXON has no MapleSEA character named “NopeMika”');
  await expect(ign).toHaveAttribute('aria-describedby', /lookup/);

  await ign.fill('BusyMika');
  await add.getByRole('button', { name: 'Look up' }).click();
  await expect(card(add).locator('.status-chip')).toHaveText('held');
  await expect(card(add)).toContainText('You can still add BusyMika');

  // No key after all: the manual fields come back with the reason.
  await ign.fill('OfflineMika');
  await add.getByRole('button', { name: 'Look up' }).click();
  await expect(card(add).locator('.status-chip')).toHaveText('not connected');
  await expect(add.getByRole('combobox', { name: 'Class' })).toBeVisible();
  await expect(add.getByRole('button', { name: 'Look up' })).toHaveCount(0);
});

test('add character: another region keeps the manual class and level and asks no link', async ({ page }) => {
  const add = await openAdd(page);
  await add.getByRole('textbox', { name: 'IGN' }).fill('MikaGms');
  await choose(add.getByRole('combobox', { name: 'Region' }), { label: 'GMS' });
  await expect(add.getByRole('button', { name: 'Look up' })).toHaveCount(0);
  await choose(add.getByRole('combobox', { name: 'Class' }), { label: 'Kain' });
  await add.getByRole('textbox', { name: 'Level' }).fill('250');
  await add.getByRole('button', { name: 'Add character' }).click();
  await expect(add).toBeHidden();
  const toast = page.locator('.toast:not(.is-leaving)').filter({ hasText: 'Added MikaGms.' });
  await expect(toast).toBeVisible();
  await expect(toast).not.toContainText('link');
});

test('add character: without a key the manual fields stay', async ({ page }) => {
  await page.request.post(`${ADMIN}/__mock/character-data`, { data: { health: 'setup' } });
  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: /^Mika/ }).first().click();
  const pane = page.getByRole('complementary', { name: 'Member details' });
  await pane.getByRole('tab', { name: /^Characters/ }).click();
  await pane.getByRole('button', { name: 'Add character' }).click();
  const add = page.getByRole('dialog', { name: 'Add character' });
  await expect(add.getByRole('combobox', { name: 'Class' })).toBeVisible();
  await expect(add.getByRole('button', { name: 'Look up' })).toHaveCount(0);
});

// Review captures (desktop 1280x800 and phone 390x844): empty, found, not found, another region.
const CAPTURES = process.env.KANADE_ADD_CHARACTER_CAPTURES;
for (const [frame, size] of [
  ['desktop', { width: 1280, height: 800 }],
  ['phone', { width: 390, height: 844 }],
] as const) {
  test(`add character capture: ${frame}`, async ({ page }) => {
    test.skip(!CAPTURES, 'set KANADE_ADD_CHARACTER_CAPTURES=<dir> to write the review captures');
    mkdirSync(CAPTURES!, { recursive: true });
    await page.setViewportSize(size);
    const add = await openAdd(page);
    const shot = async (name: string) => {
      await settle(page);
      await page.screenshot({ path: `${CAPTURES}/${frame}-${name}.png` });
    };
    await shot('1-empty');
    const ign = add.getByRole('textbox', { name: 'IGN' });
    await ign.fill('MikaKain');
    await add.getByRole('button', { name: 'Look up' }).click();
    await expect(card(add).locator('.status-chip')).toHaveText('found');
    await shot('2-found');
    await ign.fill('NopeMika');
    await add.getByRole('button', { name: 'Look up' }).click();
    await expect(card(add).locator('.status-chip')).toHaveText('not found');
    await shot('3-not-found');
    await ign.fill('MikaGms');
    await choose(add.getByRole('combobox', { name: 'Region' }), { label: 'GMS' });
    await expect(add.getByRole('combobox', { name: 'Class' })).toBeVisible();
    await shot('4-other-region');
  });
}
