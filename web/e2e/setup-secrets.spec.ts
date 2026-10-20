import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Admin Setup's secrets (`/api/admin/setup/secrets…`) on the e2e server: every
// seeded secret is a host file except the model gateway key, which starts
// unset; checks are fake (`good-…` accepted, anything else refused) and
// "Restart now" answers without stopping the server (examples/e2e_server).

const secret = (page: Page, name: string) => page.locator(`.setup-row[data-secret="${name}"]`);

async function open(page: Page) {
  await page.goto(`${ADMIN}/setup?sw=off`);
  await expect(secret(page, 'model_api_key')).toBeVisible();
}

async function moveClock(page: Page, seconds: number) {
  const moved = await page.request.post(`${ADMIN}/__e2e/clock`, { data: { advance_s: seconds } });
  expect(moved.status()).toBe(200);
}

test('setup secrets: host files are shown and locked; the unset key can be entered', async ({ page }) => {
  await open(page);
  await expect(page.getByRole('heading', { level: 3, name: /^Secrets/ })).toBeVisible();
  await expect(secret(page, 'model_api_key').locator('.status-chip')).toHaveText('Not set');
  await expect(secret(page, 'model_api_key').getByRole('button', { name: 'Enter… Model gateway key' })).toBeVisible();
  for (const name of ['discord_token', 'discord_client_secret', 'public_discord_client_secret', 'nexon_api_key']) {
    await expect(secret(page, name).locator('.status-chip')).toHaveText('On the host');
    await expect(secret(page, name).getByRole('button')).toHaveCount(0);
  }
  await expect(page.getByText('Restart now')).toHaveCount(0);
});

test('setup secrets: check, save and restart a model gateway key', async ({ page }) => {
  await open(page);
  await secret(page, 'model_api_key').getByRole('button', { name: /^Enter/ }).click();
  const dialog = page.getByRole('dialog', { name: 'Model gateway key' });
  const value = dialog.getByLabel('Value');
  await expect(value).toHaveAttribute('type', 'password');

  await value.fill('stolen-key');
  await dialog.getByRole('button', { name: 'Check' }).click();
  await expect(dialog.getByRole('status')).toHaveText('The model gateway refused this key.');
  await value.fill('good-key');
  await expect(dialog.getByRole('status')).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Check' }).click();
  await expect(dialog.getByRole('status')).toHaveText('The model gateway accepts this key and lists 3 models.');

  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(secret(page, 'model_api_key').locator('.status-chip')).toHaveText('Saved here');
  await expect(secret(page, 'model_api_key').getByRole('button', { name: /^Replace/ })).toBeVisible();
  // The value never comes back: not on the page, not in the list read.
  await expect(page.locator('body')).not.toContainText('good-key');
  const listed = await page.request.get(`${ADMIN}/api/admin/setup/secrets`);
  expect(await listed.text()).not.toContain('good-key');

  const note = page.getByRole('status').filter({ hasText: 'Saved secrets are read when Kanade starts' });
  await expect(note).toBeVisible();
  await note.getByRole('button', { name: 'Restart now' }).click();
  await expect(page.getByText('Kanade is restarting; refresh this page in a few seconds.')).toBeVisible();
});

test('setup secrets: a sign-in older than 15 minutes asks to sign in again before saving', async ({ page }) => {
  await open(page);
  await moveClock(page, 16 * 60);
  await secret(page, 'model_api_key').getByRole('button', { name: /^Enter/ }).click();
  const dialog = page.getByRole('dialog', { name: 'Model gateway key' });
  await dialog.getByLabel('Value').fill('good-key');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Saving secrets needs a sign-in from the last 15 minutes.');
  await expect(dialog.getByRole('link', { name: 'Sign in again' })).toHaveAttribute('href', '/login');
  // Still signed in: the dialog stays, nothing was saved.
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: /close/i }).click();
  await expect(secret(page, 'model_api_key').locator('.status-chip')).toHaveText('Not set');
});
