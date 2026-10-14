import { ADMIN, expect, test } from './support';

// Toasts (user decision 2026-10-05): at most two stacked, newest on top;
// errors stay until dismissed.

test('toasts: two at most, the newest on top', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  await page.goto(`${ADMIN}/config?section=env&sw=off`);
  const panel = page.locator('.settings__panel:not([hidden])');
  const live = page.locator('.toast:not(.is-leaving)');
  for (const key of ['KANADE_BOSS_WEEK_RESET_WEEKDAY', 'KANADE_POST_CHANNEL_ID', 'KANADE_TIMEZONE']) {
    await panel.getByRole('button', { name: `Copy ${key}` }).click();
    await expect(live.first()).toContainText(`Copied ${key}.`);
  }
  await expect(live).toHaveCount(2);
  await expect(live).toHaveText([/Copied KANADE_TIMEZONE\./, /Copied KANADE_POST_CHANNEL_ID\./]);
  // Newest on top on screen as well as in reading order.
  const [top, below] = await Promise.all([live.nth(0).boundingBox(), live.nth(1).boundingBox()]);
  expect(top!.y).toBeLessThan(below!.y);
});

test('toasts: success hides after 6 s; an error stays until dismissed', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  await page.clock.install();
  await page.goto(`${ADMIN}/config?section=env&sw=off`);
  await page.locator('.settings__panel:not([hidden])').getByRole('button', { name: 'Copy KANADE_TIMEZONE' }).click();
  const copied = page.getByRole('group', { name: 'Notification' }).filter({ hasText: 'Copied KANADE_TIMEZONE.' });
  await expect(copied).toBeVisible();
  await page.clock.runFor(5_000);
  await expect(copied).toBeVisible();
  await page.clock.runFor(1_500);
  await expect(copied).toHaveCount(0);

  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: () => Promise.reject(new Error('denied')) }, configurable: true });
  });
  await page.locator('.settings__panel:not([hidden])').getByRole('button', { name: 'Copy KANADE_TIMEZONE' }).click();
  const error = page.getByRole('group', { name: 'Notification' }).filter({ hasText: "Couldn't copy here" });
  await expect(error).toBeVisible();
  await page.clock.runFor(30_000);
  await expect(error).toBeVisible();
  await error.getByRole('button', { name: 'Dismiss' }).click();
  await expect(error).toHaveCount(0);
});
