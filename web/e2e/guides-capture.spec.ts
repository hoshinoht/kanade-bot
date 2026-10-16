import { mkdirSync } from 'node:fs';
import type { Page } from '@playwright/test';
import { ADMIN, choose, expect, setModules, settle, test, useLook } from './support';

// Review captures of the Guides screens (boards docs/research/2026-10-16-guide-mockups)
// for the user's approval, not assertions: KANADE_GUIDES_CAPTURES=1 (and
// KANADE_REAL_ART=1 for the real art) writes them to /private/tmp/GuidesUi.
// Each state is its own test (the fixture resets the server first).
test.describe.configure({ mode: 'parallel' });

const OUT = '/private/tmp/GuidesUi';
const LIMBO = '3d0c5b8a-7e21-4f3a-b6d9-2a4c8e1f5a10';
const DESK = { width: 1280, height: 800 };
const PHONE = { width: 390, height: 844 };

async function script(name: string) {
  const set = await fetch(`${ADMIN}/__e2e/discord/guide`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ script: name }) });
  expect(set.status, await set.text()).toBe(204);
}

async function shot(page: Page, name: string) {
  await settle(page);
  mkdirSync(OUT, { recursive: true });
  await page.screenshot({ path: `${OUT}/${name}.png`, animations: 'disabled' });
}

async function open(page: Page, item = LIMBO) {
  await page.goto(`${ADMIN}/guides?${item ? `item=${item}&` : ''}sw=off`);
  await expect(page.getByRole('listbox', { name: 'Guides' }).or(page.locator('.guide-phone')).first()).toBeVisible();
  if (item) await expect(page.locator('.gslot').first().or(page.locator('.guide-phonetabs')).first()).toBeVisible();
}

const block = (page: Page, n: number) => page.getByRole('button', { name: new RegExp(`^Edit block ${n}:`) });

async function editText(page: Page, n: number, text: string) {
  await block(page, n).click();
  await page.getByLabel(`Block ${n} text, Discord Markdown`).fill(text);
}

async function save(page: Page) {
  await page.getByRole('button', { name: 'Save draft' }).last().click();
  await expect(page.getByText(/Draft saved · version \d+|Version \d+ saved/).first()).toBeVisible();
  // The save's toast sits over the phone's bottom bar: let it go before the next press.
  await page.locator('.toast').getByRole('button', { name: 'Dismiss' }).click();
  await expect(page.locator('.toast:not(.is-leaving)')).toHaveCount(0);
}

async function plan(page: Page) {
  await page.getByRole('button', { name: /^(Review and publish…|Publish…)$/ }).last().click();
  const dialog = page.getByRole('dialog', { name: /^(Publish|Move) / });
  await expect(dialog.locator('.gplan__counts, .flash--ok').first()).toBeVisible();
  return dialog;
}

/** Limbo party roles with block 1 changed and saved (an edit to publish). */
async function edited(page: Page) {
  await open(page);
  await editText(page, 1, '# Limbo party roles\nWho stands where, who calls the swaps, and who brings the feast.');
  await save(page);
}

const LOOKS = [
  ['light', DESK, 'desk'],
  ['light', PHONE, 'phone'],
] as const;

for (const [theme, viewport, frame] of [...LOOKS, ['dark', DESK, 'desk'] as const]) {
  const t = `${frame}-${theme}`;
  test.describe(t, () => {
    test.skip(!process.env.KANADE_GUIDES_CAPTURES, 'set KANADE_GUIDES_CAPTURES=1 to write the review captures');
    test.use({ viewport });
    test.beforeEach(async ({ page }) => useLook(page, 'blossom', theme));

    test(`list ${t}`, async ({ page }) => {
      await open(page, frame === 'phone' ? '' : LIMBO);
      if (frame === 'desk' && theme === 'light') await page.getByRole('button', { name: /^More/ }).click();
      await shot(page, `01-list-${t}`);
    });

    test(`editor bosses ${t}`, async ({ page }) => {
      await open(page);
      await block(page, 2).click();
      await page.locator('.gentry').first().scrollIntoViewIfNeeded();
      await shot(page, `03-editor-bosses-${t}`);
    });

    if (theme === 'dark') return;

    test(`editor text ${t}`, async ({ page }) => {
      await open(page);
      await editText(page, 3, `## Light side\n- Two bishops and the caller.\n- ${'Hold the line at the left pillar until the swap call. '.repeat(20)}`);
      await page.locator('.locked, .gblock--open').first().scrollIntoViewIfNeeded();
      await shot(page, `02-editor-text-${t}`);
    });

    test(`preview ${t}`, async ({ page }) => {
      await open(page);
      await editText(page, 1, '# Limbo party roles\nWho stands where, and who calls the **swaps**.');
      if (frame === 'phone') await page.getByRole('tab', { name: /^Preview/ }).click();
      await shot(page, `04-preview-${t}`);
    });

    test(`plan ${t}`, async ({ page }) => {
      await edited(page);
      await plan(page);
      await shot(page, `05-plan-${t}`);
    });

    test(`plan repost ${t}`, async ({ page }) => {
      await open(page);
      await block(page, 2).click();
      await choose(page.getByRole('combobox', { name: 'Add boss' }), { label: 'Gatekeeper Kalos' });
      await save(page);
      const dialog = await plan(page);
      await dialog.getByRole('checkbox', { name: /Delete and repost/ }).check();
      await shot(page, `06-plan-repost-${t}`);
    });

    if (frame === 'phone') return;

    test(`plan move ${t}`, async ({ page }) => {
      await open(page);
      await choose(page.locator('.guide-channel').getByRole('combobox', { name: 'Channel' }), { label: /#guide-raids/ });
      await save(page);
      await plan(page);
      await shot(page, `07-plan-move-${t}`);
    });

    test(`progress and done ${t}`, async ({ page }) => {
      // Move the guide (5 posts, then 5 deletes); posts wait on `hold` so the run is seen running.
      await open(page);
      await choose(page.locator('.guide-channel').getByRole('combobox', { name: 'Channel' }), { label: /#guide-raids/ });
      await save(page);
      for (let i = 0; i < 3; i++) await script('hold');
      const dialog = await plan(page);
      await dialog.getByRole('button', { name: /^Move and publish/ }).click();
      await expect(page.locator('.guide-pane .drafts__panehead .cap')).toHaveText(/^Publishing/);
      await script('release');
      await script('hold');
      await expect(page.locator('.guide-pane .drafts__panetitle')).toHaveText(/^[1-9] of /, { timeout: 10_000 });
      await shot(page, `08-progress-${t}`);
      await script('release');
      await expect(page.locator('.guide-pane .drafts__panehead .cap')).toHaveText(/^Published/, { timeout: 15_000 });
      await shot(page, `09-done-${t}`);
    });

    test(`unconfirmed ${t}`, async ({ page }) => {
      await open(page);
      await page.locator('.gblocks__add').getByRole('button', { name: 'Text' }).click();
      await page.getByLabel('Block 6 text, Discord Markdown').fill('## Swaps\n- The caller counts down from three.');
      await save(page);
      await script('ambiguous_create');
      const dialog = await plan(page);
      await dialog.getByRole('button', { name: /^Publish/ }).click();
      await expect(page.locator('.guide-unknown')).toBeVisible({ timeout: 15_000 });
      await page.getByRole('button', { name: 'Back to the preview' }).click({ timeout: 2_000 }).catch(() => undefined);
      await shot(page, `10-unconfirmed-${t}`);
      await page.getByRole('button', { name: 'Mark as not posted…' }).click();
      await shot(page, `10b-unconfirmed-confirm-${t}`);
    });

    test(`delete ${t}`, async ({ page }) => {
      await open(page);
      await page.getByRole('button', { name: 'Delete guide…' }).click();
      await expect(page.getByRole('alertdialog')).toBeVisible();
      await shot(page, `11-delete-unpublish-${t}`);
      await page.getByRole('button', { name: /^Unpublish/ }).click();
      await expect(page.locator('.guide-pane .drafts__panehead .cap')).toHaveText(/^Unpublished/, { timeout: 15_000 });
      await page.locator('.guide-pane').getByRole('button', { name: 'Delete guide…' }).click();
      await expect(page.getByRole('alertdialog')).toContainText('Nothing is posted any more');
      await shot(page, `11b-delete-guide-${t}`);
    });

    test(`history ${t}`, async ({ page }) => {
      await page.goto(`${ADMIN}/history?sw=off`);
      await page.locator('.history-row').filter({ hasText: 'publish requested' }).click();
      await expect(page.locator('.side-pane--history')).toContainText('Limbo party roles: publish requested');
      await shot(page, `12-history-${t}`);
    });

    test(`module off ${t}`, async ({ page }) => {
      await setModules({ guides: false });
      await page.goto(`${ADMIN}/guides?sw=off`);
      await expect(page.getByRole('heading', { level: 1 })).toHaveText("That doesn't exist");
      await page.getByRole('button', { name: 'More' }).click();
      await shot(page, `13-module-off-${t}`);
    });
  });
}
