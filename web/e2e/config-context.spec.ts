import AxeBuilder from '@axe-core/playwright';
import type { ConfigView } from '@kanade/api-types';
import type { Page } from '@playwright/test';
import { ADMIN, expect, settle, test } from './support';

const WARNING = 'Context past 16k may result in degraded performance on local models.';
const toast = (page: Page, text: string | RegExp) => page.getByRole('group', { name: 'Notification' }).filter({ hasText: text });

async function open(page: Page) {
  await page.goto(`${ADMIN}/config?section=models&sw=off`);
  const panel = page.getByRole('tabpanel', { name: 'Models' });
  await panel.getByRole('tab', { name: 'Context windows' }).click();
  await expect(panel.getByRole('table', { name: 'In effect now' })).toBeVisible();
  return panel;
}

async function axe(page: Page) {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const serious = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical' || v.id === 'target-size');
  expect(serious.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

test('context windows: slider and exact field, keyboard, local warning, save and reload', async ({ page }) => {
  const panel = await open(page);
  const effective = panel.getByRole('table', { name: 'In effect now' });
  // kanata/chat publishes no window and stays in the homelab: the local default applies.
  await expect(effective.getByRole('row', { name: /^Chat/ })).toContainText('Local default');
  await expect(effective.getByRole('row', { name: /^Chat/ })).toContainText('8,192');
  await expect(effective.getByRole('row', { name: /^Extraction/ })).toContainText('Published by Kanata');

  const defaults = panel.getByRole('group', { name: 'Defaults' });
  const localSlider = defaults.getByRole('slider', { name: 'Local default' });
  const localField = defaults.getByRole('spinbutton', { name: 'Local default' });
  await expect(localField).toHaveValue('8192');
  await expect(defaults.getByText(WARNING)).toHaveCount(0);

  // Keyboard: an arrow moves one stop, Home/End jump to the ends.
  await localSlider.focus();
  await page.keyboard.press('ArrowRight');
  await expect(localField).toHaveValue('10240');
  await expect(localSlider).toHaveAttribute('aria-valuetext', '10,240 tokens');
  await page.keyboard.press('End');
  await expect(localField).toHaveValue('131072');
  await page.keyboard.press('Home');
  await expect(localField).toHaveValue('64');
  // The 16k marker sits on the local track only.
  await expect(defaults.locator('.token__mark')).toHaveCount(1);

  // The exact field drives the slider; past 16k the local slider warns in words.
  await localField.fill('24576');
  await expect(localSlider).toHaveAttribute('aria-valuetext', '24,576 tokens');
  await expect(defaults.getByText(WARNING)).toHaveCount(1);
  await expect(localField).toHaveAttribute('aria-describedby', /warn/);

  // Cloud never warns, however large.
  const cloudField = defaults.getByRole('spinbutton', { name: 'Cloud default' });
  await cloudField.fill('100000');
  await expect(defaults.getByText(WARNING)).toHaveCount(1);
  await expect(defaults.getByRole('slider', { name: 'Cloud default' })).toHaveAttribute('aria-valuetext', '100,000 tokens');

  // A role cap can be set and cleared.
  const chat = panel.getByRole('group', { name: 'Chat limits' });
  await chat.getByRole('checkbox', { name: 'Cap the chat window' }).check();
  await expect(chat.getByRole('spinbutton', { name: 'Chat window cap' })).toHaveValue('8192');
  await chat.getByRole('checkbox', { name: 'Cap the chat window' }).uncheck();
  await expect(chat.getByRole('spinbutton', { name: 'Chat window cap' })).toHaveCount(0);

  // Non-blocking: it saves, and the server's notice joins the toast.
  await panel.getByRole('button', { name: 'Save context windows' }).click();
  await expect(toast(page, 'Context windows saved; the next call uses them.')).toBeVisible();
  await expect(toast(page, WARNING)).toBeVisible();
  await expect(effective.getByRole('row', { name: /^Chat/ })).toContainText('24,576');
  await expect(effective.getByRole('row', { name: /^Chat/ })).toContainText('past 16k on a local model');

  await page.reload();
  await page.getByRole('tab', { name: 'Context windows' }).click();
  const again = page.getByRole('tabpanel', { name: 'Models' }).getByRole('group', { name: 'Defaults' });
  await expect(again.getByRole('spinbutton', { name: 'Local default' })).toHaveValue('24576');
  await expect(again.getByRole('spinbutton', { name: 'Cloud default' })).toHaveValue('100000');
  await expect(again.getByRole('slider', { name: 'Local default' })).toHaveAttribute('aria-valuetext', '24,576 tokens');
  await axe(page);
});

test('context windows: overrides held to the published max, local vs cloud, refusals shown', async ({ page }) => {
  const panel = await open(page);
  const overrides = panel.getByRole('group', { name: 'Per-model overrides' });
  const pick = overrides.getByRole('combobox', { name: 'Override model' });
  const add = overrides.getByRole('button', { name: 'Add override' });

  // A local model: published 65,536; the new row's field takes focus.
  await pick.selectOption('kanata/think');
  await add.click();
  const think = overrides.getByRole('listitem').filter({ has: page.getByText('kanata/think', { exact: true }) });
  const thinkField = think.getByRole('spinbutton', { name: 'kanata/think window' });
  await expect(thinkField).toBeFocused();
  await expect(think.getByRole('list', { name: 'What Kanata publishes for kanata/think' })).toContainText('published max 65,536');
  await expect(think.getByRole('list', { name: 'What Kanata publishes for kanata/think' })).toContainText('max output 8,192');
  await expect(thinkField).toHaveAttribute('max', '65536');
  await thinkField.fill('12288');
  await expect(think.getByText(WARNING)).toHaveCount(0);
  await thinkField.fill('20480');
  await expect(think.getByText(WARNING)).toBeVisible();
  // The slider tops out at the published window.
  await think.getByRole('slider', { name: 'kanata/think window' }).press('End');
  await expect(thinkField).toHaveValue('65536');

  // A cloud model: published past the hard limit, so held to 131,072; no warning.
  await pick.selectOption('kanata/chat-cloud');
  await add.click();
  const cloud = overrides.getByRole('listitem').filter({ has: page.getByText('kanata/chat-cloud', { exact: true }) });
  const cloudField = cloud.getByRole('spinbutton', { name: 'kanata/chat-cloud window' });
  await expect(cloudField).toHaveAttribute('max', '131072');
  await cloudField.fill('120000');
  await expect(cloud.locator('.token__mark')).toHaveCount(0);
  await expect(cloud.getByText(WARNING)).toHaveCount(0);

  // Removing a row returns focus to the model picker.
  await cloud.getByRole('button', { name: 'Remove the kanata/chat-cloud override' }).click();
  await expect(pick).toBeFocused();

  // A reserve that fills the chat window (8,192) is refused inline.
  const chatReserve = panel.getByRole('group', { name: 'Chat limits' }).getByRole('spinbutton', { name: 'Chat reply reserve' });
  await chatReserve.fill('8192');
  await panel.getByRole('button', { name: 'Save context windows' }).click();
  await expect(panel.getByRole('alert').filter({ hasText: 'models.context.chat.reserve must be smaller than its effective window (8192).' })).toBeVisible();
  // The edits stay to fix; a smaller reserve saves.
  await chatReserve.fill('1024');
  await panel.getByRole('button', { name: 'Save context windows' }).click();
  await expect(toast(page, 'Context windows saved')).toBeVisible();
  await axe(page);
});

test('context windows: an override above the published window is refused by the server', async ({ page }) => {
  // A stale catalog read: the app believes kanata/extract publishes 131,072,
  // the server knows 16,384 and refuses.
  await page.route('**/api/admin/config', async (route) => {
    if (route.request().method() !== 'GET') {
      await route.continue();
      return;
    }
    const response = await route.fetch();
    const view = (await response.json()) as ConfigView;
    const extract = view.models.catalog.find((m) => m.id === 'kanata/extract')!;
    extract.context_tokens = 131_072;
    await route.fulfill({ response, json: view });
  });
  const panel = await open(page);
  const overrides = panel.getByRole('group', { name: 'Per-model overrides' });
  await overrides.getByRole('combobox', { name: 'Override model' }).selectOption('kanata/extract');
  await overrides.getByRole('button', { name: 'Add override' }).click();
  await overrides.getByRole('spinbutton', { name: 'kanata/extract window' }).fill('20000');
  await panel.getByRole('button', { name: 'Save context windows' }).click();
  await expect(panel.getByRole('alert').filter({ hasText: "models.context.overrides.kanata/extract exceeds Kanata's published context window (16384)." })).toBeVisible();
});
