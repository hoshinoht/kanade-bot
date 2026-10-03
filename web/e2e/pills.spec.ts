import { ADMIN, expect, test } from './support';

// One set of semantic pill profiles (success / warning / danger / info /
// neutral) for every state pill; the word always rides along.

test('pill profiles: chat, extractions, inbox, planner sheet and config share one colour story', async ({ page }) => {
  await page.goto(`${ADMIN}/chat?sw=off`);
  const chat = page.getByRole('table', { name: /Chatbot interactions/ });
  await expect(chat.locator('.tone--success', { hasText: 'answered' }).first()).toBeVisible();
  await expect(chat.locator('.tone--danger', { hasText: /timed out|timeout/i }).first()).toBeVisible();
  await expect(chat.locator('.status')).toHaveCount(0);

  await page.goto(`${ADMIN}/extractions?sw=off`);
  await expect(page.locator('table .tone--danger', { hasText: 'failed' }).first()).toBeVisible();

  await page.goto(`${ADMIN}/inbox?tab=self_service&sw=off`);
  const list = page.getByRole('listbox', { name: 'Self-service items' });
  await list.locator('[data-item="p-fa-request"]').click();
  await expect(list.locator('.tone--danger', { hasText: 'conflict' })).toBeVisible();
  await list.locator('[data-item="p-kalos-expired"]').click();
  await expect(list.locator('.tone--neutral', { hasText: 'expired' })).toBeVisible();

  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-kalos"] .plan-card__open').click();
  await expect(page.getByRole('complementary', { name: 'XKalos' }).locator('.tone--danger', { hasText: 'At risk' }).first()).toBeVisible();
  await page.keyboard.press('Escape');

  await page.goto(`${ADMIN}/config?section=models&sw=off`);
  await page.getByRole('tab', { name: 'Capacity' }).click();
  await expect(page.getByRole('table', { name: /Every model shares one group/ }).locator('.models__check .tone--success')).toHaveCount(1);
});
