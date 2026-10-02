import AxeBuilder from '@axe-core/playwright';
import type { ConfigView } from '@kanade/api-types';
import { ADMIN, expect, test } from './support';

test('models config warns when raw member data leaves the homelab', async ({ page }) => {
  await page.route('**/api/admin/config', async (route) => {
    if (route.request().method() !== 'GET') {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    const view = (await response.json()) as ConfigView;
    view.models.roles.rewrite.alias = 'kanata/unlisted-synthetic';
    await route.fulfill({ response, json: view });
  });

  await page.goto(`${ADMIN}/config?section=models&sw=off`);
  const panel = page.getByRole('tabpanel', { name: 'Models' });
  await expect(panel.getByRole('tab', { name: 'Roles', selected: true })).toBeVisible();

  const extraction = panel.getByRole('group', { name: 'Extraction' });
  await expect(extraction.getByRole('list', { name: 'What kanata/extract can do' }).getByText('homelab')).toBeVisible();
  await extraction.getByRole('combobox', { name: 'Model' }).selectOption('kanata/legacy');
  await expect(extraction.locator('.settings__warn')).toContainText(/publishes no trust zone/);
  await expect(extraction.locator('.settings__warn')).toContainText(/raw member names, IDs, messages, and URLs leave the homelab/i);

  const chat = panel.getByRole('group', { name: 'Chat' });
  await chat.getByRole('combobox', { name: 'Model' }).selectOption('kanata/chat-cloud');
  const externalWarning = chat.locator('.settings__warn');
  await expect(externalWarning).toContainText(/go to an external provider/);
  await expect(externalWarning).toContainText(/raw member names, IDs, messages, and URLs leave the homelab/i);

  const rewrite = panel.getByRole('group', { name: 'Rewrite' });
  const unlistedWarning = rewrite.locator('.settings__warn');
  await expect(unlistedWarning).toContainText('kanata/unlisted-synthetic');
  await expect(unlistedWarning).toContainText(/not in Kanata's list, so its availability is unknown and it is treated as external/);
  await expect(unlistedWarning).toContainText(/raw member names, IDs,\s*messages, and URLs are sent outside the homelab/i);
  await expect(unlistedWarning).toContainText(/requests may fail if the alias is unavailable/i);
  await expect(panel).not.toContainText(/pseudonym|mask(?:ed|ing)?|provider testing|ALLOW_EXTERNAL_UNMASKED|PSEUDONYMIZE/i);

  const modelSelect = chat.getByRole('combobox', { name: 'Model' });
  await modelSelect.focus();
  await expect(modelSelect).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(chat.getByRole('combobox', { name: 'Reasoning' })).toBeFocused();

  await page.waitForTimeout(350);
  const axe = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice'])
    .analyze();
  const serious = axe.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical' || v.id === 'target-size');
  expect(serious.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);

  await page.getByRole('tab', { name: 'Set in the environment' }).click();
  const env = page.getByRole('tabpanel', { name: 'Set in the environment' });
  await expect(env.getByText('KANADE_TIMEZONE')).toBeVisible();
  await expect(env).not.toContainText(/KANADE_PSEUDONYMIZE|KANADE_ALLOW_EXTERNAL_UNMASKED|provider testing|pseudonymisation/i);
});
