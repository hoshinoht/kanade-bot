import { mkdirSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import type { ConfigView } from '@kanade/api-types';
import { ADMIN, choose, expect, go, openList, optionIn, setGateway, settle, test, toast } from './support';

// Config › Models and Setup against a model gateway that is not Kanata
// (`/__e2e/gateway`): `plain` lists the same ids with no Kanata metadata,
// `no_listing` answers `/v1/models` with 404. The server declares every
// model's capabilities, so both run on the declarations; the fixture's reset
// brings the Kanata listing back.

async function models(page: Page) {
  await go(page, '/config?section=models');
  const panel = page.getByRole('tabpanel', { name: 'Models' });
  await expect(panel.getByRole('tab', { name: 'Roles', selected: true })).toBeVisible();
  return panel;
}

/**
 * Picks the rewrite model, saves, and reads the saved alias back from the API. The seed's
 * extraction level is Off, which kanata/extract refuses (it requires reasoning), so it moves to Low.
 */
async function saveRewrite(page: Page, panel: Locator, alias: string) {
  await choose(panel.getByRole('group', { name: 'Extraction' }).getByRole('combobox', { name: 'Reasoning' }), 'low');
  await choose(panel.getByRole('group', { name: 'Rewrite' }).getByRole('combobox', { name: 'Model' }), alias);
  await panel.getByRole('button', { name: 'Save models' }).click();
  await expect(toast(page, /Models saved; the next question uses them\./)).toBeVisible();
  const saved = (await (await page.request.get(`${ADMIN}/api/admin/config`)).json()) as ConfigView;
  expect(saved.models.roles.rewrite.alias).toBe(alias);
}

test('plain gateway: Models lists the gateway’s models, saves a role, and claims no Kanata zone', async ({ page }) => {
  await setGateway('plain');
  await page.setViewportSize({ width: 1280, height: 800 });
  const panel = await models(page);

  await expect(panel.getByText('Three roles, each a base model from the model gateway’s live list.')).toBeVisible();
  await expect(panel.getByRole('alert')).toHaveCount(0);
  await expect(panel.getByText(/provides no model list/)).toHaveCount(0);
  // Listed, so never "Declared"; no published zone, so external with the warning, never the homelab.
  const extraction = panel.getByRole('group', { name: 'Extraction' });
  const caps = extraction.getByRole('list', { name: 'What kanata/extract can do' });
  await expect(caps.locator('.capchip').first()).toHaveText('trust zone unknown');
  await expect(caps).not.toContainText('Declared');
  await expect(extraction.locator('.settings__warn')).toContainText('kanata/extract publishes no trust zone, so it is treated as an external route.');
  await expect(panel.locator('.capchip', { hasText: /^\s*(homelab|leaves the homelab)\s*$/ })).toHaveCount(0);
  await expect(panel).not.toContainText('Stays in the homelab');
  // Admission limits are Kanata's alone: none published, so no Kanata sentence on any tab.
  await expect(panel).not.toContainText(/Kanata/);
  await expect(panel.getByText("The model gateway publishes no limit for this key; it is shared with the owner's other clients.")).toHaveCount(1);

  const rewrite = panel.getByRole('group', { name: 'Rewrite' }).getByRole('combobox', { name: 'Model' });
  const list = await openList(rewrite);
  for (const id of ['kanata/chat', 'kanata/extract', 'kanata/think', 'kanata/rewrite-small']) await expect(optionIn(list, id)).toBeVisible();
  await rewrite.press('Escape');
  await saveRewrite(page, panel, 'kanata/think');
  await expect(panel.getByRole('list', { name: 'What kanata/think can do' }).locator('.capchip').first()).toHaveText('trust zone unknown');
});

test('no listing: declared models, the no-list note, a saved role, and Setup ready with its line', async ({ page }) => {
  await setGateway('no_listing');
  await page.setViewportSize({ width: 1280, height: 800 });
  const panel = await models(page);

  await expect(panel.getByText('Three roles, each a base model from the declared models.')).toBeVisible();
  await expect(panel.getByText('The model gateway provides no model list, so roles are picked from the declared models ([models.capabilities] in kanade.toml).')).toBeVisible();
  // Not read-only: the unreachable alert stays away and the pickers are live.
  await expect(panel.getByRole('alert')).toHaveCount(0);
  for (const role of ['Extraction', 'Chat', 'Rewrite']) {
    const group = panel.getByRole('group', { name: role });
    await expect(group.getByRole('combobox', { name: 'Model' })).toBeEnabled();
    const caps = group.getByRole('list', { name: /^What .+ can do$/ });
    await expect(caps.locator('.capchip').first()).toHaveText('Declared');
    await expect(caps.locator('.capchip').nth(1)).toHaveText('trust zone unknown');
    await expect(group.locator('.settings__warn')).toContainText(/treated as an external route/);
  }
  await expect(panel).not.toContainText('Stays in the homelab');

  await saveRewrite(page, panel, 'kanata/think');
  await expect(panel.getByRole('list', { name: 'What kanata/think can do' }).locator('.capchip').first()).toHaveText('Declared');

  await go(page, '/setup');
  const gateway = page.locator('.setup-row[data-item="model_gateway"]');
  await expect(gateway.locator('.status-chip')).toHaveText('Ready');
  await expect(gateway).toContainText('The model gateway answers; it lists no models, so the declared models are used.');
});

// Review captures of Config › Models with no model list: `KANADE_GATEWAY_CAPTURES=<dir>` (PNGs only).
const CAPTURES = process.env.KANADE_GATEWAY_CAPTURES;
for (const [frame, size] of [
  ['desk', { width: 1280, height: 800 }],
  ['phone', { width: 390, height: 844 }],
] as const) {
  test(`no listing capture: ${frame}`, async ({ page }) => {
    test.skip(!CAPTURES, 'set KANADE_GATEWAY_CAPTURES=<dir> to write the review captures');
    mkdirSync(CAPTURES!, { recursive: true });
    await setGateway('no_listing');
    await page.setViewportSize(size);
    const panel = await models(page);
    await expect(panel.getByText(/provides no model list/)).toBeVisible();
    await settle(page);
    await page.screenshot({ path: `${CAPTURES}/config-models-no-listing-${frame}.png`, animations: 'disabled' });
  });
}
