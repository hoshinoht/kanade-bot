import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, PUBLIC, expect, test } from './support';

async function serious(page: Page, label: string) {
  // Let entry animations finish; axe reads mid-fade opacity as low contrast.
  await page.waitForTimeout(350);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} (${v.impact}) ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
  return result.violations.length;
}

// All five colourways in both faces.
const LOOKS = (['marigold', 'blossom', 'periwinkle', 'coral', 'twilight'] as const).flatMap((c) =>
  (['light', 'dark'] as const).map((t) => [c, t] as const),
);

for (const [colorway, theme] of LOOKS) {
  test(`axe: public and admin views, ${colorway} ${theme}`, async ({ page }) => {
    // Forty-odd scans per face: well past the default 30 s on a busy machine.
    test.setTimeout(120_000);
    await page.addInitScript(([c, t]) => {
      localStorage.setItem('colorway', c!);
      localStorage.setItem('theme', t!);
    }, [colorway, theme]);

    await page.goto(`${PUBLIC}/?sw=off`);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 runs');
    await serious(page, 'public week');
    await page.getByRole('tab', { name: /List/ }).click();
    await serious(page, 'public list');

    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    await serious(page, 'admin planner');
    await page.locator('[data-run="r-carling"] .plan-card__open').click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await serious(page, 'admin modal');
    await page.keyboard.press('Escape');
    await page.keyboard.press('ControlOrMeta+k');
    await expect(page.getByRole('dialog', { name: 'Command palette' })).toBeVisible();
    await serious(page, 'admin palette');
    await page.keyboard.press('Escape');
    await page.getByRole('tab', { name: 'Answers' }).click();
    await expect(page.locator('.chart__canvas canvas')).toBeVisible();
    await serious(page, 'admin answers');
    await page.getByRole('link', { name: 'Config' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'Config' })).toBeVisible();
    await serious(page, 'admin config');
    await page.getByRole('tab', { name: 'Models' }).click();
    await expect(page.getByText(/Capacity groups/)).toBeVisible();
    await serious(page, 'admin config models');
    await page.getByRole('tab', { name: 'Channel access' }).click();
    await expect(page.getByRole('table', { name: "The bot's permissions in each channel" })).toBeVisible();
    await serious(page, 'admin config access');
    await page.getByRole('tab', { name: 'Self-service' }).click();
    await expect(page.getByText(/pre-filled link to the public portal/)).toBeVisible();
    await serious(page, 'admin config self-service');
    await page.getByRole('tab', { name: 'Persona' }).click();
    await expect(page.getByText(/Reload profiles/)).toBeVisible();
    await serious(page, 'admin config persona');
    await page.getByRole('tab', { name: 'Pings' }).click();
    await expect(page.getByRole('textbox', { name: 'Morning ping' })).toBeVisible();
    await serious(page, 'admin config pings');
    await page.getByRole('tab', { name: 'Set in the environment' }).click();
    await expect(page.getByRole('row', { name: /Watched categories/ })).toBeVisible();
    await serious(page, 'admin config env');
    await page.getByRole('tab', { name: 'Models' }).click();
    await page.getByRole('combobox', { name: /^Model/ }).first().selectOption('kanata/chat-cloud');
    await expect(page.getByText(/go to an external provider/)).toBeVisible();
    await serious(page, 'admin config cloud warning');
    await page.getByRole('link', { name: /^Inbox/ }).click();
    await expect(page.getByRole('listbox', { name: 'Extractor items' })).toBeVisible();
    await serious(page, 'admin inbox extractor');
    await page.getByRole('tab', { name: /Self-service/ }).click();
    await page.getByRole('option', { name: /HFA/ }).click();
    await expect(page.getByText('Changed since the member asked')).toBeVisible();
    await serious(page, 'admin inbox self-service');
    await page.getByRole('link', { name: 'Extractions' }).click();
    await page.getByText('Re-read the party channels').click();
    await serious(page, 'admin extractions');
    await page.goto(`${ADMIN}/extractions/x-kalos?sw=off`);
    await expect(page.getByRole('tab', { name: /Changes/ })).toBeVisible();
    await serious(page, 'admin extraction');
    await page.goto(`${ADMIN}/chat/c-move?sw=off`);
    await page.getByRole('tab', { name: /Tool trace/ }).click();
    await serious(page, 'admin chat turn');
    await page.goto(`${ADMIN}/limits?sw=off`);
    await expect(page.getByRole('heading', { name: /^extract · / })).toBeVisible();
    await serious(page, 'admin limits');
    await page.getByRole('tab', { name: /Admission/ }).click();
    await serious(page, 'admin limits admission');
    await page.goto(`${ADMIN}/history?sw=off`);
    await expect(page.getByText('reverts #8')).toBeVisible();
    await page.getByText('Rows (1)').first().click();
    await serious(page, 'admin history');
    await page.getByRole('button', { name: 'Revert #8' }).click();
    await expect(page.getByRole('dialog', { name: 'Revert #8?' })).toBeVisible();
    await serious(page, 'admin revert dialog');
    await page.keyboard.press('Escape');
    await page.goto(`${ADMIN}/bosses/Kai/knowledge?sw=off`);
    await expect(page.getByText('Event boss.')).toBeVisible();
    await serious(page, 'admin event knowledge');
    await page.getByRole('link', { name: 'Fixed' }).click();
    await expect(page.getByRole('row').nth(1)).toBeVisible();
    await serious(page, 'admin fixed');
    await page.getByRole('button', { name: /^Edit Tuesday 22:00/ }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await serious(page, 'admin fixed editor');
    await page.keyboard.press('Escape');
    await page.getByRole('link', { name: 'Bosses' }).click();
    await expect(page.locator('.bossrow').first()).toBeVisible();
    await serious(page, 'admin bosses');
    await page.getByRole('link', { name: 'Carling' }).click();
    await expect(page.getByRole('heading', { name: 'Sources' })).toBeVisible();
    await serious(page, 'admin knowledge');
    await page.getByRole('link', { name: 'Members' }).click();
    await page.getByRole('button', { name: /^Asahi/ }).click();
    await expect(page.getByRole('dialog', { name: 'Asahi' })).toBeVisible();
    await serious(page, 'admin member sheet');
    await page.keyboard.press('Escape');
    await page.getByRole('link', { name: 'Reminders' }).click();
    await expect(page.getByRole('heading', { name: /^Queued/ })).toBeVisible();
    await serious(page, 'admin reminders');
    await page.goto(`${ADMIN}/?week=next&sw=off`);
    await expect(page.locator('[data-run="n-carling"]')).toBeVisible();
    await serious(page, 'admin next week');
    await page.goto(`${ADMIN}/login?sw=off`);
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    await serious(page, 'admin login');
  });
}

test('axe: offline windows', async ({ page }) => {
  await page.goto(`${PUBLIC}/offline.html`);
  await expect(page.getByRole('heading', { name: "You're offline" })).toBeVisible();
  await serious(page, 'public offline page');
});

test('axe: public closed window', async ({ page, request }) => {
  await request.patch(`${ADMIN}/api/admin/config`, { data: { self_service: { public_portal: false } } });
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { name: "The schedule isn't public right now" })).toBeVisible();
  await serious(page, 'public closed');
  await request.patch(`${ADMIN}/api/admin/config`, { data: { self_service: { public_portal: true } } });
});
