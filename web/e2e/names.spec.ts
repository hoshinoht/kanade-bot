import { ADMIN, expect, test } from './support';

// User decision 2026-09-26: Discord members, channels and roles show by name,
// never by raw id; activating a name copies the id; mention tokens in message
// text read as @name / #channel / @role.

test.beforeEach(async ({ context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
});

test('chat log: names, never ids, and a click copies the id', async ({ page }) => {
  await page.goto(`${ADMIN}/chat?sw=off`);
  const table = page.getByRole('table', { name: /Chatbot interactions/ });
  // Someone the lists do not know yet: neutral words, not "user 11494860" or the id.
  // Role mentions read from GET /api/admin/roles.
  const stranger = table.getByRole('row').filter({ has: page.getByRole('link', { name: 'is @staff around tonight?' }) });
  await expect(stranger).toContainText('Unknown member');
  await expect(stranger).toContainText('#unknown-channel');
  await expect(table).not.toContainText('user 1149');
  await expect(table).not.toContainText('114948601234567890');
  await expect(table).not.toContainText('999000111222333444');
  // Every question is asked of Kanade: the opening bot mention is left out of the preview.
  await expect(table.getByRole('link', { name: 'when is carling this week', exact: true })).toBeVisible();
  await expect(table).not.toContainText('@YuukiSakuna');
  await expect(table).not.toContainText('<@');
  // Other mentions still read as names.
  await expect(table.getByRole('link', { name: 'who is in #fa-night tonight, is @Yuzu in?' })).toBeVisible();

  // Guild-local friendly time, the full stamp in its tooltip; compact durations.
  const when = table.getByRole('row', { name: /when is carling this week/ });
  const time = when.locator('time');
  await expect(time).toHaveText('Mon 28 Sep · 00:00');
  await expect(time).toHaveAttribute('title', 'Mon 28 Sep 2026, 00:00:00 (Asia/Kuala_Lumpur)');
  await expect(time).toHaveAttribute('datetime', '2026-09-27T16:00:00Z');
  await expect(when).toContainText('3.4 s');
  await expect(table).not.toContainText('2026-09-');

  const ren = table.getByRole('row', { name: /when is carling this week/ }).getByRole('button', { name: 'Ren', exact: true });
  // A clipped cell's tooltip leads with the whole name.
  await expect(ren).toHaveAttribute('title', 'Ren · Member ID 1002 — click to copy');
  await ren.click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('1002');
  await expect(page.getByRole('status').filter({ hasText: 'Copied the member ID of Ren.' })).toBeAttached();

  // An unknown one still copies its id.
  await stranger.getByRole('button', { name: 'Unknown member' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('114948601234567890');
  await stranger.getByRole('button', { name: '#unknown-channel' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('999000111222333444');
});

test('chat turn: mention tokens render as copyable names', async ({ page }) => {
  await page.goto(`${ADMIN}/chat/c-error?sw=off`);
  const asked = page.locator('.pane__section', { hasText: 'What they asked' }).locator('xpath=following-sibling::p[1]');
  await expect(asked).toHaveText('@YuukiSakuna who is in #fa-night tonight, is @Yuzu in?');
  await asked.getByRole('button', { name: '@Yuzu' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('1004');
  await asked.getByRole('button', { name: '#fa-night' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('fa-night');
  await expect(page.locator('.page-head').getByRole('button', { name: 'Yuzu' })).toBeVisible();
});

test('run sheet, history and member sheet name people without their ids', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-kalos"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'XKalos' });
  await sheet.getByText('Answers — set who’s in or out').click();
  const tsubame = sheet.locator('.answers__who').getByRole('button', { name: 'Tsubame' });
  await tsubame.click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('1005');
  await page.keyboard.press('Escape');

  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: /^Tsubame/ }).first().click();
  const member = page.getByRole('dialog', { name: 'Tsubame' });
  await expect(member).not.toContainText('1005');
  await member.getByRole('button', { name: 'Tsubame' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('1005');
});

test('roles, the bot and message authors resolve from the server\'s ids', async ({ page }) => {
  // The bot by Identity.bot_user_id, a role by /roles (with its colour as a swatch).
  await page.goto(`${ADMIN}/chat/c-stranger?sw=off`);
  const asked = page.locator('.pane__section', { hasText: 'What they asked' }).locator('xpath=following-sibling::p[1]');
  await expect(asked).toHaveText('@YuukiSakuna is @staff around tonight?');
  const staff = asked.getByRole('button', { name: '@staff' });
  await expect(staff).toHaveAttribute('title', 'Role ID 300001 — click to copy');
  await expect(asked.locator('.name__swatch')).toHaveCount(1);
  await staff.click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('300001');
  await asked.getByRole('button', { name: '@YuukiSakuna' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('1543532497948909578');

  // Evidence authors are names that copy their ids.
  await page.goto(`${ADMIN}/inbox?sw=off`);
  const evidence = page.locator('.inbox__detail').getByLabel('Evidence');
  await evidence.getByRole('button', { name: 'Minato' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('1012');
});

