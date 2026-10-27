import type { Page } from '@playwright/test';
import { ADMIN, choose, csrf, expect, go, test, toast } from './support';

// A Config section save joins the History timeline as a view-only row with
// its before/after. Clock pinned.

async function saveQuietMode(page: Page) {
  await go(page, '/config?section=notifications');
  await page.getByRole('switch', { name: /^Quiet mode/ }).click();
  await expect(toast(page, 'Quiet mode is on.')).toBeVisible();
}

test('history: a saved Config section is a view-only row with its before and after', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await saveQuietMode(page);
  await go(page, '/history');
  // The seed's character saves are Config rows too (Asahi's, through the real character writes).
  const row = page.locator('.history-row--config').filter({ hasText: 'Config · Notifications' });
  await expect(row).toHaveCount(1);
  await expect(row).toContainText('Config · Notifications — quiet_mode');
  await expect(row.locator('.chip', { hasText: 'Config' })).toBeVisible();
  await expect(row).toContainText('1 setting');

  await row.click();
  await expect(row).toHaveAttribute('aria-current', 'true');
  const pane = page.getByRole('complementary', { name: 'Change details' });
  await expect(pane.getByRole('heading', { name: 'Notifications settings saved' })).toBeVisible();
  const field = pane.locator('.history-diff__field');
  await expect(field).toHaveCount(1);
  await expect(field.locator('dt')).toHaveText('quiet_mode');
  await expect(field.locator('.history-diff__was')).toHaveText(/^was\s*0$/);
  await expect(field.locator('.history-diff__now')).toHaveText(/^now\s*1$/);
  // View-only: no rollback tools for a Config save.
  await expect(pane.getByRole('button', { name: 'Revert…' })).toHaveCount(0);
  await expect(pane.getByRole('button', { name: 'Restore week to here…' })).toHaveCount(0);
  await expect(pane.locator('.history-member')).toHaveCount(0);

  const trigger = pane.getByRole('button', { name: 'Show raw JSON' });
  await trigger.click();
  const viewer = page.getByRole('dialog', { name: 'Notifications settings raw JSON' });
  await expect(viewer).toContainText('"quiet_mode"');
  await page.keyboard.press('Escape');
  await expect(viewer).toBeHidden();
  await expect(trigger).toBeFocused();

  // Who applies to Config saves too. The seed's character saves are Asahi's as well, so only her Notifications save is counted.
  await choose(page.getByRole('combobox', { name: 'Who' }), 'member:100000000000001005');
  await expect(page.locator('.history-row--config')).toHaveCount(0);
  await choose(page.getByRole('combobox', { name: 'Who' }), 'admin:discord:100000000000001001');
  await expect(row).toHaveCount(1);

  // The link opens the section it saved.
  await row.click();
  await pane.getByRole('link', { name: 'Open Notifications in Config' }).click();
  await expect(page).toHaveURL(/\/config\?section=notifications/);
  await expect(page.getByRole('switch', { name: /^Quiet mode/ })).toHaveAttribute('aria-checked', 'true');
});

test('history: on a phone a Config save opens as a sheet without Revert', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await saveQuietMode(page);
  await go(page, '/history');
  const row = page.locator('.history-row--config').filter({ hasText: 'Config · Notifications' });
  await row.click();
  const detail = page.getByRole('dialog', { name: 'Notifications settings' });
  await expect(detail).toBeVisible();
  await expect(detail.locator('.history-diff__field dt')).toHaveText('quiet_mode');
  await expect(detail.getByRole('button', { name: 'Revert…' })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(detail).toBeHidden();
  await expect(row).toBeFocused();
});

test('history: an answer reads "→ on for <run>" in the Timeline and the change pane', async ({ page }) => {
  type WeekRead = { version: number; runs: { id: string; participants: { id: string; name: string; answer: string }[] }[] };
  const answer = async (pick: number) => {
    const week = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as WeekRead;
    const who = week.runs.find((r) => r.id === 'r-carling')!.participants.filter((p) => p.answer !== 'yes')[pick]!;
    const answered = await page.request.post(`${ADMIN}/api/admin/runs/r-carling/rsvp`, { headers: await csrf(page.request), data: { member_id: who.id, answer: 'yes', version: week.version } });
    expect(answered.ok()).toBe(true);
    return who;
  };
  // The first answer also moves the run's status (the record's headline); the second is an answer alone.
  await answer(0);
  const who = await answer(0);
  const line = new RegExp(`${who.name}(?: \\(\\d\\))? → on for HCarling \\+ HMaleficStar`);
  await go(page, '/history');
  const summary = page.locator('[data-history] .history-row__summary').filter({ hasText: line }).first();
  await expect(summary).toHaveText(line);
  await expect(summary).not.toContainText('on on');
  await expect(summary).not.toContainText('r-carling');
  await summary.click();
  await expect(page.getByRole('complementary', { name: 'Change details' }).getByRole('heading', { level: 2 })).toHaveText(line);
});
