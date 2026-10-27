import { ADMIN, expect, test, choose } from './support';

// Both tests answer on Tuesday's Black Mage (r-bm, 23:30 at the pinned noon): runs that have ended
// refuse writes on the real server (409 run_ended).
test('admin run pane: status, answers, roster and preview ping', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-bm"] .plan-card__open').click();
  // Gate G4: on a wide screen the run opens in the Week window's side pane.
  const sheet = page.getByRole('complementary', { name: 'XBM' });
  const notice = sheet.locator('.sheet__notice');

  const status = sheet.getByRole('group', { name: 'Status' });
  await expect(status.getByRole('button', { name: 'Planned' })).toHaveAttribute('aria-pressed', 'true');
  await status.getByRole('button', { name: 'Confirmed' }).click();
  await expect(status.getByRole('button', { name: 'Confirmed' })).toHaveAttribute('aria-pressed', 'true');
  await expect(notice).toContainText('XBM is now confirmed.');
  await notice.getByRole('button', { name: 'Undo' }).click();
  await expect(status.getByRole('button', { name: 'Planned' })).toHaveAttribute('aria-pressed', 'true');

  await sheet.getByRole('tab', { name: /^Answers/ }).click();
  const sora = sheet.getByRole('group', { name: 'Answer for Sora on XBM' });
  await sora.getByRole('button', { name: 'Out' }).click();
  await expect(sora.getByRole('button', { name: 'Out' })).toHaveAttribute('aria-pressed', 'true');
  await sheet.getByRole('tab', { name: 'Run' }).click();
  await expect(sheet.locator('.chip--no', { hasText: 'Sora' })).toBeVisible();
  // Undo set Planned by hand, which holds it; an admin's answer keeps a held status.
  await expect(status.getByRole('button', { name: 'Planned' })).toHaveAttribute('aria-pressed', 'true');
  await expect(sheet.getByText('someone said no')).toHaveCount(0);

  // A party change before the start ends the hold and re-derives: Sora's no puts XBM at risk.
  await choose(sheet.getByRole('combobox', { name: 'Add someone to XBM for this week' }), { label: 'Hotaru' });
  await expect(sheet.locator('.run__people .chip', { hasText: 'Hotaru' })).toBeVisible();
  await expect(sheet.getByText('someone said no')).toBeVisible();
  await sheet.getByRole('button', { name: 'Take Hotaru off this run for this week only' }).click();
  await expect(sheet.locator('.run__people .chip', { hasText: 'Hotaru' })).toHaveCount(0);

  await sheet.getByRole('button', { name: 'Preview ping' }).click();
  await expect(notice).toContainText(/Preview \(not posted\): the morning card for XBM at \d\d:\d\d in #bm-trio\./);
  await expect(sheet.getByRole('button', { name: 'Preview ping' })).toHaveAttribute('title', /nothing is posted/);
});

test('admin run sheet below 900 px: status, answers and roster in one modal', async ({ page }) => {
  await page.setViewportSize({ width: 800, height: 800 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-bm"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'XBM' });
  // Results show in the sheet's own status line: the modal makes the page's toasts inert.
  const notice = sheet.locator('.sheet__notice');

  // An answer first: Sora's no puts XBM at risk (a hand-set status would hold against it).
  // HeroPhone: answers are the window's Answers tab; Party shows each member as a slot.
  await sheet.getByRole('tab', { name: /^Answers/ }).click();
  const sora = sheet.getByRole('group', { name: 'Answer for Sora on XBM' });
  await sora.getByRole('button', { name: 'Out' }).click();
  await expect(sora.getByRole('button', { name: 'Out' })).toHaveAttribute('aria-pressed', 'true');
  await sheet.getByRole('tab', { name: /^Party/ }).click();
  await expect(sheet.locator('.slot--no', { hasText: 'Sora' })).toBeVisible();
  await expect(sheet.getByText('someone said no')).toBeVisible();

  // At risk is not a status to go back to, so only a change from a set status offers Undo.
  const status = sheet.getByRole('group', { name: 'Status' });
  await status.getByRole('button', { name: 'Confirmed' }).click();
  await expect(notice).toContainText('XBM is now confirmed.');
  await expect(notice.getByRole('button', { name: 'Undo' })).toHaveCount(0);
  await status.getByRole('button', { name: 'Planned' }).click();
  await expect(notice).toContainText('XBM is now planned.');
  await notice.getByRole('button', { name: 'Undo' }).click();
  await expect(status.getByRole('button', { name: 'Confirmed' })).toHaveAttribute('aria-pressed', 'true');
});
