import { mkdirSync } from 'node:fs';
import type { Page } from '@playwright/test';
import { ADMIN, expect, settle, test } from './support';

// Approval captures for the dropdown (P_Select boards), in the four faces:
// `KANADE_REAL_ART=1 KANADE_SELECT_CAPTURE=<dir> bunx playwright test select-capture`.
// Skipped unless KANADE_SELECT_CAPTURE names an output directory.
const OUT = process.env.KANADE_SELECT_CAPTURE ?? '';
const FACES = [
  { name: 'blossom-light', colorway: 'blossom', theme: 'light' },
  { name: 'blossom-dark', colorway: 'blossom', theme: 'dark' },
  { name: 'marigold-light', colorway: 'marigold', theme: 'light' },
  { name: 'marigold-dark', colorway: 'marigold', theme: 'dark' },
];

test.describe.configure({ mode: 'parallel' });
test.skip(!OUT, 'KANADE_SELECT_CAPTURE is not set');

async function shot(page: Page, name: string) {
  await settle(page);
  await page.screenshot({ path: `${OUT}/${name}.png`, animations: 'disabled' });
}

const SCENES: { name: string; width: number; height: number; run: (page: Page) => Promise<void> }[] = [
  {
    name: 'reminders-filters-open',
    width: 1280,
    height: 800,
    run: async (page) => {
      await page.goto(`${ADMIN}/reminders?sw=off`);
      await page.getByRole('button', { name: /^Filters/ }).click();
      const run = page.getByRole('combobox', { name: 'Run' });
      await run.click();
      await page.getByRole('listbox', { name: 'Run' }).getByRole('option').nth(3).click();
      await run.click();
      await page.keyboard.press('ArrowDown');
      await expect(page.getByRole('listbox', { name: 'Run' })).toBeVisible();
    },
  },
  {
    name: 'history-who-search',
    width: 1280,
    height: 800,
    run: async (page) => {
      await page.goto(`${ADMIN}/history?sw=off`);
      await page.getByRole('combobox', { name: 'Who' }).click();
      await page.getByRole('combobox', { name: 'Filter people' }).fill('r');
      await expect(page.getByRole('listbox', { name: 'Who' })).toBeVisible();
    },
  },
  {
    name: 'reread-multi',
    width: 1280,
    height: 800,
    run: async (page) => {
      await page.goto(`${ADMIN}/config?section=rescan&sw=off`);
      const channels = page.getByRole('combobox', { name: 'Channels to re-read' });
      await channels.click();
      const list = page.getByRole('listbox', { name: 'Channels to re-read' });
      await list.getByRole('option').nth(0).click();
      await list.getByRole('option').nth(2).click();
      await page.keyboard.press('ArrowDown');
    },
  },
  {
    name: 'run-lengths-error',
    width: 1280,
    height: 800,
    run: async (page) => {
      await page.goto(`${ADMIN}/config?section=run-lengths&sw=off`);
      const panel = page.getByRole('tabpanel', { name: 'Run lengths' });
      await panel.getByRole('button', { name: 'Add an override' }).click();
      await panel.getByRole('button', { name: 'Save run lengths' }).click();
      await expect(panel.getByText('Pick a boss before saving this length.')).toBeVisible();
    },
  },
  {
    name: 'history-week-titlebar',
    width: 1280,
    height: 800,
    run: async (page) => {
      await page.goto(`${ADMIN}/history?sw=off`);
      await page.getByRole('combobox', { name: 'Week' }).click();
      await expect(page.getByRole('listbox', { name: 'Week' })).toBeVisible();
    },
  },
  {
    name: 'phone-triggers',
    width: 390,
    height: 844,
    run: async (page) => {
      await page.goto(`${ADMIN}/reminders?sw=off`);
      await page.getByRole('button', { name: /^Filters/ }).click();
      await page.getByRole('combobox', { name: 'Run' }).selectOption({ index: 2 });
    },
  },
];

for (const scene of SCENES) {
  for (const face of FACES) {
    test(`${scene.name} ${face.name}`, async ({ page }) => {
      mkdirSync(OUT, { recursive: true });
      await page.setViewportSize({ width: scene.width, height: scene.height });
      await page.addInitScript(
        ([c, t]) => {
          localStorage.setItem('colorway', c!);
          localStorage.setItem('theme', t!);
        },
        [face.colorway, face.theme],
      );
      await scene.run(page);
      await shot(page, `${scene.name}-${face.name}`);
    });
  }
}
