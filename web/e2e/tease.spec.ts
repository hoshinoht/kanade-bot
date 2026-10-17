import type { Page } from '@playwright/test';
import { ADMIN, ASAHI_SNOWFLAKE, PUBLIC, csrf, expect, signInPublic, test, toast } from './support';

// Reminder teasing (boards in docs/research/2026-10-17-header-tease): the
// member's own "OK to be teased" on the portal's Account › Profile, the
// admin's switch and running-joke note on the Members sheet, and Config ›
// Notifications' two header switches. The seed has teasing and estimate
// headers off, and nobody opted in. Every test ends with zero CSP/TT reports.

/** Mika's snowflake (the seed's member ids are Discord ids). */
const MIKA = '100000000000001003';

async function headers(page: Page, notifications: { header_estimates?: boolean; header_teasing?: boolean }) {
  const reply = await page.request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(page.request), data: { notifications } });
  expect(reply.status()).toBe(200);
}

async function adminPatch(page: Page, member: string, data: { tease_ok?: boolean; running_joke?: string }) {
  const reply = await page.request.patch(`${ADMIN}/api/admin/members/${member}`, { headers: await csrf(page.request), data });
  expect(reply.status()).toBe(200);
}

async function profile(page: Page) {
  await page.goto(`${PUBLIC}/account?sw=off`);
  const row = page.locator('li.account-row', { hasText: 'OK to be teased' });
  await expect(row.getByRole('switch', { name: 'OK to be teased' })).toBeVisible();
  return row;
}

async function sheet(page: Page) {
  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: /^Mika/ }).first().click();
  const pane = page.getByRole('complementary', { name: 'Member details' });
  await expect(pane.getByText('Reminder teasing')).toBeVisible();
  return pane;
}

test('member: the switch works while teasing is off for everyone, with Undo, and the note shows once it is on', async ({ page }) => {
  await signInPublic(page);
  const row = await profile(page);
  const flag = row.getByRole('switch', { name: 'OK to be teased' });
  // Off for everyone (the seed): the chip and why, and the choice still applies.
  await expect(row.getByText('off for everyone', { exact: true })).toBeVisible();
  await expect(flag).toHaveAccessibleDescription('Teasing is off for everyone right now. Your choice is kept for when an admin turns it on.');
  await expect(flag).toHaveAttribute('aria-checked', 'false');
  await flag.click();
  await expect(flag).toHaveAttribute('aria-checked', 'true');
  await toast(page, 'You’re OK to be teased.').getByRole('button', { name: 'Undo' }).click();
  await expect(toast(page, 'You’re no longer OK to be teased.')).toBeVisible();
  await expect(flag).toHaveAttribute('aria-checked', 'false');
  await flag.click();
  await expect(flag).toHaveAttribute('aria-checked', 'true');
  const saved = (await (await page.request.get(`${PUBLIC}/api/public/me/preferences`)).json()) as { tease_ok: boolean };
  expect(saved.tease_ok).toBe(true);

  // On for everyone, with a note from the admins: no chip, the note says it is sent.
  await headers(page, { header_estimates: true, header_teasing: true });
  await adminPatch(page, ASAHI_SNOWFLAKE, { running_joke: 'still owes the party a Pitched box from August' });
  await profile(page);
  await expect(row.getByText('off for everyone', { exact: true })).toHaveCount(0);
  await expect(flag).toHaveAccessibleDescription('Reminder headers may joke about your share of the party’s estimate. Your name is never sent to the chat model.');
  const note = page.locator('li.tease-joke');
  await expect(note).toContainText('Running joke · from the admins');
  await expect(note).toContainText('“still owes the party a Pitched box from August”');
  await expect(note).toContainText('Sent with your share');
  // Opted out, the note is kept and not sent.
  await flag.click();
  await expect(note).toContainText('Kept; not sent while you’re off');
});

test('member: a refused save keeps the switch as it was and says why', async ({ page }) => {
  await signInPublic(page);
  const row = await profile(page);
  await page.route('**/api/public/me/preferences', (route) =>
    route.request().method() === 'PATCH'
      ? route.fulfill({ status: 503, contentType: 'application/json', body: '{"error":"unavailable","message":"Kanade is busy; try again."}' })
      : route.continue(),
  );
  const flag = row.getByRole('switch', { name: 'OK to be teased' });
  await flag.click();
  await expect(toast(page, 'Couldn\'t save your choice: Kanade is busy; try again.')).toBeVisible();
  await expect(flag).toHaveAttribute('aria-checked', 'false');
});

test('admin: the switch applies at once, and the note is refused for a roster IGN with the text kept', async ({ page }) => {
  const pane = await sheet(page);
  const flag = pane.getByRole('switch', { name: 'OK to be teased' });
  await expect(flag).toHaveAttribute('aria-checked', 'false');
  await flag.click();
  await expect(flag).toHaveAttribute('aria-checked', 'true');
  await expect(pane.locator('.membersheet__notice')).toHaveText('Mika is OK to be teased.');

  const field = pane.getByRole('textbox', { name: 'Running-joke note' });
  const save = pane.getByRole('button', { name: 'Save', exact: true });
  await expect(save).toBeDisabled();
  // Another member's IGN: the sheet doesn't hold every member's characters, so it says what the rule is.
  await field.fill('RenShade carries again');
  await expect(pane.getByText('22 / 80')).toBeVisible();
  await save.click();
  const error = pane.getByRole('alert');
  await expect(error).toHaveText('Not saved: it names someone on the roster (a name, alias or IGN). A note never names anyone.');
  await expect(field).toHaveAttribute('aria-invalid', 'true');
  await expect(field).toHaveValue('RenShade carries again');
  await expect(save).toBeEnabled();
  await expect(pane.locator('.membersheet__notice')).toHaveText('');
  // Mika's own IGN: named, with the note without it to try.
  await field.fill('MikaBish forgets her familiar every single week');
  await save.click();
  await expect(error).toHaveText('Not saved: “MikaBish” is a roster IGN. A note never names anyone; try “forgets her familiar every single week”.');
  await field.fill('@here look');
  await save.click();
  await expect(error).toHaveText('Not saved: plain text only, with no mentions, @ or # handles, links or formatting.');

  await field.fill('forgets her familiar every single week');
  await save.click();
  await expect(pane.locator('.membersheet__notice')).toHaveText('Mika’s running-joke note saved.');
  await expect(error).toHaveCount(0);
  await expect(field).not.toHaveAttribute('aria-invalid', 'true');
  await expect(save).toBeDisabled();
  const rows = (await (await page.request.get(`${ADMIN}/api/admin/members`)).json()) as { id: string; tease_ok: boolean; running_joke: string | null }[];
  expect(rows.find((m) => m.id === MIKA)).toMatchObject({ tease_ok: true, running_joke: 'forgets her familiar every single week' });
});

test('config: teasing waits for estimates; estimates off turns teasing off, and Undo brings both back', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=notifications&sw=off`);
  const estimates = page.getByRole('switch', { name: /^Estimates in reminder headers/ });
  const teasing = page.getByRole('switch', { name: /^Teasing in reminder headers/ });
  // Estimates off (the seed): teasing can't be turned on, and says why.
  await expect(estimates).toHaveAttribute('aria-checked', 'false');
  await expect(teasing).toHaveAttribute('aria-disabled', 'true');
  await expect(teasing).toHaveAccessibleDescription(/^Turn on Estimates in reminder headers first\./);
  // Playwright waits for an aria-disabled control to enable: press it regardless, as a user could.
  await teasing.click({ force: true });
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(teasing).toHaveAttribute('aria-checked', 'false');
  // The server refuses it too.
  const refused = await page.request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(page.request), data: { notifications: { header_teasing: true } } });
  expect(refused.status()).toBe(422);

  await estimates.click();
  await expect(toast(page, 'Estimates in reminder headers are on.')).toBeVisible();
  await expect(teasing).toHaveAttribute('aria-disabled', 'false');
  await teasing.click();
  const ask = page.getByRole('dialog', { name: 'Turn teasing on?' });
  await expect(ask).toContainText('Members who allowed it may be teased in headers.');
  await ask.getByRole('button', { name: 'Turn teasing on' }).click();
  await expect(teasing).toHaveAttribute('aria-checked', 'true');

  await estimates.click();
  const off = page.getByRole('dialog', { name: 'Turn estimate headers off?' });
  await expect(off).toContainText('Headers go back to today’s wording, including lines already written, and teasing goes off too.');
  await off.getByRole('button', { name: 'Turn estimate headers off' }).click();
  await expect(estimates).toHaveAttribute('aria-checked', 'false');
  await expect(teasing).toHaveAttribute('aria-checked', 'false');
  await expect(teasing).toHaveAttribute('aria-disabled', 'true');
  await toast(page, 'Estimates in reminder headers are off, and teasing with them.').getByRole('button', { name: 'Undo' }).click();
  await expect(toast(page, 'Estimates and teasing in reminder headers are on again.')).toBeVisible();
  await expect(estimates).toHaveAttribute('aria-checked', 'true');
  await expect(teasing).toHaveAttribute('aria-checked', 'true');
});
