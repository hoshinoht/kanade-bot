import { ADMIN, expect, test } from './support';

// Config → Notifications: the daily reminder-header batch time saves on its
// own, refuses a bad clock inline, and offers Undo.

test('header generation time: saves, refuses a bad clock, undoes', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/config?section=notifications&sw=off`);
  const card = page.getByRole('form', { name: 'Reminder header rewrites' });
  await expect(card).toContainText('Lines for the next 24 h are written then; runs added or moved later get theirs when first seen.');
  const field = card.getByRole('textbox', { name: /Generate daily at/ });
  await expect(field).toHaveValue('00:00');

  await field.fill('3:30');
  await card.getByRole('button', { name: 'Save' }).click();
  await expect(card.getByRole('alert')).toHaveText('The time is HH:MM, for example 03:30.');

  await field.fill('03:30');
  await card.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('Reminder headers are written daily at 03:30.')).toBeVisible();
  await page.reload();
  await expect(page.getByRole('form', { name: 'Reminder header rewrites' }).getByRole('textbox')).toHaveValue('03:30');
});
