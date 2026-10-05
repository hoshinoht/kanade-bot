import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// One shared single-pane switch at 900 px (user decision 2026-10-05): with an
// item open, every list-detail screen shows one pane at a time at 880 px
// (between the old 840 px switch and 900) and at 899 px, and the list beside
// the detail from 900 px. All are rail frames (not the phone frame).

const NARROW = { width: 880, height: 800 };
const EDGE = { width: 899, height: 800 };
const WIDE = { width: 900, height: 800 };

interface Screen {
  name: string;
  open: (page: Page) => Promise<void>;
  /** Asserts one pane (`false`) or two (`true`). */
  panes: (page: Page, two: boolean) => Promise<void>;
}

const shown = async (page: Page, list: ReturnType<Page['locator']>, detail: ReturnType<Page['locator']>, two: boolean) => {
  await expect(detail).toBeVisible();
  if (two) await expect(list).toBeVisible();
  else await expect(list).toBeHidden();
};

// Side-pane screens: wide, a non-modal pane beside the list; narrow, a modal sheet instead.
const sidePane = async (page: Page, two: boolean, pane: string, dialog: string | RegExp) => {
  if (two) {
    await expect(page.getByRole('complementary', { name: pane })).toBeVisible();
    await expect(page.getByRole('dialog', { name: dialog })).toHaveCount(0);
  } else {
    await expect(page.getByRole('dialog', { name: dialog })).toBeVisible();
    await expect(page.getByRole('complementary', { name: pane })).toHaveCount(0);
  }
};

const SCREENS: Screen[] = [
  {
    name: 'inbox',
    open: (page) => page.goto(`${ADMIN}/inbox?tab=self_service&item=p-fa-request&sw=off`).then(() => undefined),
    panes: (page, two) => shown(page, page.locator('.inbox__list'), page.getByText('Changed since the member asked'), two),
  },
  {
    name: 'chat',
    open: (page) => page.goto(`${ADMIN}/chat/c-move?sw=off`).then(() => undefined),
    panes: (page, two) => shown(page, page.getByRole('listbox', { name: /Chatbot interactions/ }), page.getByRole('button', { name: 'Copy transcript' }), two),
  },
  {
    name: 'extractions',
    open: (page) => page.goto(`${ADMIN}/extractions?call=x-kalos&sw=off`).then(() => undefined),
    panes: (page, two) => shown(page, page.getByRole('listbox', { name: /Extraction calls/ }), page.getByText('kalos 10pm instead?'), two),
  },
  {
    name: 'bosses',
    open: (page) => page.goto(`${ADMIN}/bosses/MaleficStar/knowledge?sw=off`).then(() => undefined),
    panes: (page, two) =>
      shown(page, page.getByRole('navigation', { name: 'Boss catalog' }), page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' }), two),
  },
  {
    name: 'config',
    open: (page) => page.goto(`${ADMIN}/config?sw=off`).then(() => undefined),
    // Narrow, the section list folds into a sideways strip over the one section.
    panes: (page, two) =>
      expect(page.getByRole('tablist', { name: 'Settings sections' })).toHaveAttribute('aria-orientation', two ? 'vertical' : 'horizontal'),
  },
  {
    name: 'members',
    open: async (page) => {
      await page.goto(`${ADMIN}/members?sw=off`);
      await page.getByRole('button', { name: /^Asahi/ }).click();
    },
    panes: (page, two) => sidePane(page, two, 'Member details', 'Asahi'),
  },
  {
    name: 'fixed',
    open: async (page) => {
      await page.goto(`${ADMIN}/fixed?sw=off`);
      await page.getByRole('button', { name: 'Edit Friday 21:30 — XKalos' }).click();
    },
    panes: (page, two) => sidePane(page, two, 'Weekly timing details', /.+/),
  },
  {
    name: 'history',
    open: async (page) => {
      await page.goto(`${ADMIN}/history?sw=off`);
      await page.locator('[data-history="2"]').click();
    },
    panes: (page, two) => sidePane(page, two, 'Change details', 'Change #2'),
  },
  {
    name: 'week run pane',
    open: async (page) => {
      await page.goto(`${ADMIN}/?sw=off`);
      await page.locator('[data-run="r-carling"] .plan-card__open').click();
    },
    panes: (page, two) => sidePane(page, two, 'HCarling + HStar', 'HCarling + HStar'),
  },
];

for (const screen of SCREENS) {
  test(`single-pane switch at 900 px: ${screen.name}`, async ({ page }) => {
    for (const size of [NARROW, EDGE]) {
      await page.setViewportSize(size);
      await page.keyboard.press('Escape');
      await screen.open(page);
      await screen.panes(page, false);
      // The document never scrolls in either layout.
      expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight + 1)).toBe(true);
    }

    await page.setViewportSize(WIDE);
    await page.keyboard.press('Escape');
    await screen.open(page);
    await screen.panes(page, true);
    expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight + 1)).toBe(true);
  });
}

test('bosses in the rail frame below 900 px: focus follows the pick into the detail and back to its link', async ({ page }) => {
  await page.setViewportSize(NARROW);
  await page.goto(`${ADMIN}/bosses?sw=off`);
  const catalog = page.getByRole('navigation', { name: 'Boss catalog' });
  const link = catalog.getByRole('link', { name: 'Radiant Malefic Star' });
  const detail = page.locator('.knowledge-detail');
  await expect(link).toBeVisible();

  // A pick hides the catalog and moves focus into the detail.
  await link.focus();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(`${ADMIN}/bosses/MaleficStar/knowledge`);
  await expect(page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' })).toBeVisible();
  await expect(catalog).toBeHidden();
  await expect(detail).toBeFocused();

  // Back pops the pick's entry: the catalog returns with focus on the opened link.
  await page.getByRole('button', { name: 'Back to the catalog' }).click();
  await expect(page).toHaveURL(`${ADMIN}/bosses?sw=off`);
  await expect(catalog).toBeVisible();
  await expect(link).toBeFocused();

  // The browser's Back does the same, and never reopens the boss after a return.
  await link.click();
  await expect(detail).toBeFocused();
  await page.goBack();
  await expect(page).toHaveURL(`${ADMIN}/bosses?sw=off`);
  await expect(link).toBeFocused();
  await page.goBack();
  await expect(page).not.toHaveURL(/\/bosses\/MaleficStar/);

  // A deep link has no entry to pop: Back replaces it with the catalog.
  await page.goto(`${ADMIN}/bosses/MaleficStar/knowledge?sw=off`);
  await page.getByRole('button', { name: 'Back to the catalog' }).click();
  await expect(page).toHaveURL(`${ADMIN}/bosses`);
  await expect(catalog).toBeVisible();
  await expect(link).toBeFocused();
});
