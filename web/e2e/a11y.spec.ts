import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { COLORWAYS } from '../packages/tokens/src/colorways';
import { ADMIN, PUBLIC, csrf, expect, settle, test, choose } from './support';

// Every test is independent (the fixture resets the mock), so the looks spread across workers.
test.describe.configure({ mode: 'parallel' });

async function serious(page: Page, label: string, rules?: string[]) {
  // Let entry animations finish; axe reads mid-fade opacity as low contrast.
  await settle(page);
  const axe = new AxeBuilder({ page });
  if (rules) axe.withRules(rules);
  else axe.withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']);
  const result = await axe.analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} (${v.impact}) ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
  return result.violations.length;
}

async function look(page: Page, colorway: string, theme: string) {
  await page.addInitScript(([c, t]) => {
    localStorage.setItem('colorway', c!);
    localStorage.setItem('theme', t!);
  }, [colorway, theme]);
}

/** The default colourway gets the full walk (all rules, every screen) in both faces. */
const FULL = 'marigold';
const THEMES = ['light', 'dark'] as const;
// Every other colourway only changes colours: colour contrast on representative screens.
const CONTRAST_LOOKS = COLORWAYS.map((way) => way.key)
  .filter((c) => c !== FULL)
  .flatMap((c) => THEMES.map((t) => [c, t] as const));

for (const [colorway, theme] of CONTRAST_LOOKS) {
  test(`axe: colour contrast on representative views, ${colorway} ${theme}`, async ({ page }) => {
    const contrast = (label: string) => serious(page, label, ['color-contrast']);
    await look(page, colorway, theme);
    await page.goto(`${PUBLIC}/?sw=off`);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 runs');
    // Dynamic: wait for the avatar's palette; later pages paint it from the cache.
    if (colorway === 'dynamic') await expect(page.locator('html')).toHaveAttribute('data-dynamic', 'avatar');
    await contrast('public week');

    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    await contrast('admin planner');
    await page.locator('[data-run="r-carling"] .plan-card__open').click();
    await expect(page.getByRole('complementary', { name: 'HCarling + HStar' })).toBeVisible();
    await contrast('admin run pane');
    await page.keyboard.press('Escape');
    await page.getByRole('link', { name: 'Config' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Config' })).toBeVisible();
    await contrast('admin config');
    await page.getByRole('link', { name: /^Inbox/ }).click();
    await expect(page.getByRole('listbox', { name: 'Extractor items' })).toBeVisible();
    await contrast('admin inbox extractor');
    await page.goto(`${ADMIN}/history?sw=off`);
    await expect(page.locator('.history-row--active .history-tag--revert')).toHaveText('reverts #8');
    await page.locator('[data-history="8"]').click();
    await expect(page.getByRole('complementary', { name: 'Change details' })).toBeVisible();
    await contrast('admin history');
    await page.getByRole('link', { name: 'Bosses' }).click();
    await expect(page.locator('.bossrow').first()).toBeVisible();
    await page.getByRole('link', { name: 'Carling' }).click();
    await expect(page.getByRole('tablist', { name: 'Guide sections' })).toBeVisible();
    await contrast('admin knowledge');
    await page.getByRole('link', { name: 'Members' }).click();
    await page.getByRole('button', { name: /^Asahi/ }).click();
    await expect(page.getByRole('complementary', { name: 'Member details' })).toBeVisible();
    await contrast('admin member sheet');
  });
}

// The full walk for the default colourway, in four parts per face so they
// spread across workers (one walk of forty-odd scans outran its budget on CI).
// Each part starts from a fresh page and mock.
const WALK: { name: string; walk: (page: Page) => Promise<void> }[] = [
  {
    name: 'public and admin week',
    walk: async (page) => {
      await page.goto(`${PUBLIC}/?sw=off`);
      await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 runs');
      await serious(page, 'public week');
      await page.getByRole('tab', { name: /List/ }).click();
      await serious(page, 'public list');

      await page.goto(`${ADMIN}/?sw=off`);
      await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
      await serious(page, 'admin planner');
      await page.locator('[data-run="r-carling"] .plan-card__open').click();
      await expect(page.getByRole('complementary', { name: 'HCarling + HStar' })).toBeVisible();
      await serious(page, 'admin run pane');
      await page.keyboard.press('Escape');
      await page.keyboard.press('ControlOrMeta+k');
      await expect(page.getByRole('dialog', { name: 'Command palette' })).toBeVisible();
      await serious(page, 'admin palette');
      await page.keyboard.press('Escape');
      await page.getByRole('tab', { name: /^Answers/ }).click();
      await expect(page.getByRole('heading', { name: 'Still waiting' })).toBeVisible();
      await serious(page, 'admin answers');
      await page.goto(`${ADMIN}/?week=next&sw=off`);
      await expect(page.locator('[data-run="n-carling"]')).toBeVisible();
      await serious(page, 'admin next week');
      await page.goto(`${ADMIN}/login?sw=off`);
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
      await serious(page, 'admin login');
    },
  },
  {
    name: 'admin config',
    walk: async (page) => {
      await page.goto(`${ADMIN}/?sw=off`);
      await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
      await page.getByRole('link', { name: 'Config' }).click();
      await expect(page.getByRole('heading', { level: 1, name: 'Config' })).toBeVisible();
      await serious(page, 'admin config');
      await page.getByRole('tab', { name: 'Models' }).click();
      await page.getByRole('tab', { name: 'Capacity' }).click();
      await expect(page.getByRole('heading', { name: 'Capacity groups' })).toBeVisible();
      await serious(page, 'admin config models');
      await page.getByRole('tab', { name: 'Channel access' }).click();
      await expect(page.getByRole('table', { name: "The bot's permissions in each channel" })).toBeVisible();
      await serious(page, 'admin config access');
      await page.getByRole('tab', { name: 'Self-service' }).click();
      await expect(page.getByRole('switch', { name: /Public portal/ })).toBeVisible();
      await serious(page, 'admin config self-service');
      await page.getByRole('tab', { name: /^Profanity/ }).click();
      await page.getByRole('combobox', { name: 'Find a built-in word to allow again' }).fill('r');
      await expect(page.getByRole('listbox', { name: 'Built-in words' })).toBeVisible();
      await serious(page, 'admin config profanity');
      await page.getByRole('tab', { name: /^Persona/ }).click();
      await expect(page.getByText(/Reload profiles/)).toBeVisible();
      await serious(page, 'admin config persona');
      await page.getByRole('tab', { name: 'Pings' }).click();
      await expect(page.getByRole('textbox', { name: 'Morning ping' })).toBeVisible();
      await serious(page, 'admin config pings');
      await page.getByRole('tab', { name: 'Set in the environment' }).click();
      await expect(page.getByRole('row', { name: /Timezone/ })).toBeVisible();
      await serious(page, 'admin config env');
      await page.getByRole('tab', { name: /^Channels/ }).click();
      await expect(page.getByRole('button', { name: 'Save watched categories' })).toBeVisible();
      await serious(page, 'admin config channels');
      await page.getByRole('tab', { name: 'Models' }).click();
      await page.getByRole('tab', { name: 'Roles' }).click();
      await choose(page.getByRole('combobox', { name: /^Model/ }).first(), 'kanata/chat-cloud');
      await expect(page.getByText(/raw member names, IDs, messages, and URLs leave the homelab/i)).toBeVisible();
      await serious(page, 'admin config cloud warning');
    },
  },
  {
    name: 'admin inbox, logs and limits',
    walk: async (page) => {
      // From Config, as the walk always did (the Week page links Inbox twice).
      await page.goto(`${ADMIN}/config?sw=off`);
      await expect(page.getByRole('heading', { level: 1, name: 'Config' })).toBeVisible();
      await page.getByRole('link', { name: /^Inbox/ }).click();
      await expect(page.getByRole('listbox', { name: 'Extractor items' })).toBeVisible();
      await serious(page, 'admin inbox extractor');
      await page.getByRole('tab', { name: /Self-service/ }).click();
      await page.getByRole('option', { name: /HFA/ }).click();
      await expect(page.getByText('Changed since the member asked')).toBeVisible();
      await serious(page, 'admin inbox self-service');
      await page.getByRole('tab', { name: 'Past' }).click();
      await expect(page.locator('.inbox__detail .past__sentence')).toBeVisible();
      await serious(page, 'admin inbox past');
      await page.getByRole('link', { name: 'Extractions' }).click();
      await page.getByRole('button', { name: 'Re-read channels' }).click();
      await serious(page, 'admin extractions');
      await page.goto(`${ADMIN}/extractions/x-kalos?sw=off`);
      await expect(page.getByRole('tab', { name: /Changes/ })).toBeVisible();
      await serious(page, 'admin extraction');
      await page.getByRole('tab', { name: 'Prompt' }).click();
      await expect(page.getByRole('tabpanel', { name: 'Prompt' })).toBeVisible();
      await serious(page, 'admin extraction prompt');
      await page.goto(`${ADMIN}/chat/c-move?sw=off`);
      await page.getByRole('tab', { name: /Tool trace/ }).click();
      await serious(page, 'admin chat turn');
      await page.goto(`${ADMIN}/chat/c-safe-line?sw=off`);
      await expect(page.getByRole('region', { name: 'Profanity in the reply' })).toBeVisible();
      await serious(page, 'admin chat profanity turn');
      await page.goto(`${ADMIN}/rewrites?attempt=rw-over&sw=off`);
      await expect(page.getByRole('complementary', { name: 'Verdict' })).toContainText('budget_exceeded');
      await page.getByText(/^Reasoning/).click();
      await serious(page, 'admin rewrites');
      await page.goto(`${ADMIN}/limits?sw=off`);
      await expect(page.getByRole('heading', { level: 3, name: 'gateway' })).toBeVisible();
      await serious(page, 'admin limits');
      await page.getByRole('tab', { name: /Admission/ }).click();
      await serious(page, 'admin limits admission');
    },
  },
  {
    name: 'admin history, fixed, bosses, members and reminders',
    walk: async (page) => {
      await page.goto(`${ADMIN}/history?sw=off`);
      await expect(page.locator('.history-row--active .history-tag--revert')).toHaveText('reverts #8');
      await page.locator('[data-history="8"]').click();
      await serious(page, 'admin history');
      await page.getByRole('complementary', { name: 'Change details' }).getByRole('button', { name: 'Revert…' }).click();
      await expect(page.getByRole('dialog', { name: 'Revert #8?' })).toBeVisible();
      await serious(page, 'admin revert dialog');
      await page.keyboard.press('Escape');
      await page.getByRole('tab', { name: 'Checkpoints' }).click();
      await expect(page.getByRole('table', { name: /Backups/ })).toBeVisible();
      await serious(page, 'admin history checkpoints');
      await page.goto(`${ADMIN}/bosses/Kai/knowledge?sw=off`);
      await expect(page.getByText('Event boss.')).toBeVisible();
      await serious(page, 'admin event knowledge');
      await page.getByRole('link', { name: 'Fixed' }).click();
      await expect(page.getByRole('row').nth(1)).toBeVisible();
      await serious(page, 'admin fixed');
      await page.getByRole('button', { name: /^Edit Tuesday 22:00/ }).click();
      await expect(page.getByRole('complementary', { name: 'Weekly timing details' })).toBeVisible();
      await serious(page, 'admin fixed editor');
      await page.keyboard.press('Escape');
      await page.getByRole('link', { name: 'Bosses' }).click();
      await expect(page.locator('.bossrow').first()).toBeVisible();
      await serious(page, 'admin bosses');
      await page.getByRole('link', { name: 'Carling' }).click();
      await expect(page.getByRole('tablist', { name: 'Guide sections' })).toBeVisible();
      await serious(page, 'admin knowledge');
      await page.getByRole('link', { name: 'Members' }).click();
      await page.getByRole('button', { name: /^Asahi/ }).click();
      await expect(page.getByRole('complementary', { name: 'Member details' })).toBeVisible();
      await serious(page, 'admin member sheet');
      await page.keyboard.press('Escape');
      await page.getByRole('link', { name: 'Reminders' }).click();
      await expect(page.getByRole('tab', { name: /^Queued/ })).toBeVisible();
      await serious(page, 'admin reminders');
    },
  },
];

for (const theme of THEMES) {
  for (const part of WALK) {
    test(`axe: ${part.name}, ${FULL} ${theme}`, async ({ page }) => {
      // About ten full scans each: past the default 30 s on a loaded CI runner.
      test.setTimeout(120_000);
      await look(page, FULL, theme);
      await part.walk(page);
    });
  }
}

test('axe: members list-detail and phone sheet', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: /^Asahi/ }).click();
  await expect(page.getByRole('complementary', { name: 'Member details' })).toBeVisible();
  await serious(page, 'admin members side pane');
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole('dialog', { name: 'Asahi' })).toBeVisible();
  await serious(page, 'admin members phone sheet');
});

test('axe: offline windows', async ({ page }) => {
  await page.goto(`${PUBLIC}/offline.html`);
  await expect(page.getByRole('heading', { name: "You're offline" })).toBeVisible();
  await serious(page, 'public offline page');
});

test('axe: public closed window', async ({ page, request }) => {
  await request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(request), data: { self_service: { public_portal: false } } });
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { name: "The schedule isn't public right now" })).toBeVisible();
  await serious(page, 'public closed');
  await request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(request), data: { self_service: { public_portal: true } } });
});
