import { ADMIN, expect, test } from './support';

test('admin run sheet: status, answers, roster and preview ping', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-limbo"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'HLimbo' });
  // Results show in the sheet's own status line: the modal makes the page's toasts inert.
  const notice = sheet.locator('.sheet__notice');

  const status = sheet.getByRole('group', { name: 'Status' });
  await expect(status.getByRole('button', { name: 'Planned' })).toHaveAttribute('aria-pressed', 'true');
  await status.getByRole('button', { name: 'Confirmed' }).click();
  await expect(status.getByRole('button', { name: 'Confirmed' })).toHaveAttribute('aria-pressed', 'true');
  await expect(notice).toContainText('HLimbo is now confirmed.');
  await notice.getByRole('button', { name: 'Undo' }).click();
  await expect(status.getByRole('button', { name: 'Planned' })).toHaveAttribute('aria-pressed', 'true');

  await sheet.getByText('Answers — set who’s in or out').click();
  const sora = sheet.getByRole('group', { name: 'Answer for Sora on HLimbo' });
  await sora.getByRole('button', { name: 'Out' }).click();
  await expect(sora.getByRole('button', { name: 'Out' })).toHaveAttribute('aria-pressed', 'true');
  await expect(sheet.locator('.chip--no', { hasText: 'Sora' })).toBeVisible();
  await expect(sheet.getByText('someone said no')).toBeVisible();

  await sheet.getByRole('combobox', { name: 'Add someone to HLimbo for this week' }).selectOption({ label: 'Hotaru' });
  await expect(sheet.locator('.run__people .chip', { hasText: 'Hotaru' })).toBeVisible();
  await sheet.getByRole('button', { name: 'Take Hotaru off this run for this week only' }).click();
  await expect(sheet.locator('.run__people .chip', { hasText: 'Hotaru' })).toHaveCount(0);

  await sheet.getByRole('button', { name: 'Preview ping' }).click();
  await expect(notice).toContainText('Posted the morning card for HLimbo in #limbo-trio as a TEST message.');
});
