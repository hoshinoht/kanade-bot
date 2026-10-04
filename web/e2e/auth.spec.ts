import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Sign-in against the mock's copy of the server's auth routes: methods,
// break-glass token, the Discord browser flow and its login_error codes,
// 401 → sign in → back, method-based proposal controls, sign-out.

/** The mock's break-glass token (tools/pwa-mock `auth::MOCK_TOKEN`). */
const TOKEN = 'kanade-mock-token';

async function signOutBehind(page: Page) {
  expect((await page.request.post(`${ADMIN}/__mock/session`, { data: { method: 'none' } })).status()).toBe(204);
}

test('signed out, a page sends you to sign in and back; a wrong token is refused inline', async ({ page }) => {
  await signOutBehind(page);
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await expect(page).toHaveURL(`${ADMIN}/login?next=%2Ffixed%3Fsw%3Doff`);
  // Only the methods this server offers: Discord and the token, no tailnet.
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  await expect(page.getByText(/On the tailnet/)).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Sign in with Tailscale' })).toHaveCount(0);
  await expect(page.getByText('Sign-in is not connected')).toHaveCount(0);

  await page.getByText('Break-glass: admin token').click();
  const field = page.getByLabel('Admin token');
  await field.fill('not-the-token');
  await page.getByRole('button', { name: 'Sign in with the token' }).click();
  await expect(page.getByRole('alert').filter({ hasText: "That token isn't right." })).toBeVisible();
  await expect(field).toBeFocused();
  await expect(field).toHaveValue('');
  await expect(page).toHaveURL(/\/login\?next=/);

  await field.fill(TOKEN);
  await page.getByRole('button', { name: 'Sign in with the token' }).click();
  await expect(page).toHaveURL(`${ADMIN}/fixed?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('8 weekly timings');
  await expect(page.locator('.account__name')).toHaveText('Break-glass token');
  await expect(page.getByText("Can't reach Kanade")).toHaveCount(0);
});

test('an unsafe next never leaves the portal', async ({ page }) => {
  await signOutBehind(page);
  await page.goto(`${ADMIN}/login?next=${encodeURIComponent('//evil.example/')}&sw=off`);
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toHaveAttribute('href', '/api/admin/auth/discord/start?next=%2F');
  await page.getByText('Break-glass: admin token').click();
  await page.getByLabel('Admin token').fill(TOKEN);
  await page.getByRole('button', { name: 'Sign in with the token' }).click();
  await expect(page).toHaveURL(`${ADMIN}/`);
});

test('Discord: the start link carries next, and the flow comes back signed in', async ({ page }) => {
  await signOutBehind(page);
  await page.goto(`${ADMIN}/inbox?tab=self_service&sw=off`);
  await expect(page).toHaveURL(/\/login\?next=/);
  const discord = page.getByRole('link', { name: 'Sign in with Discord' });
  await expect(discord).toHaveAttribute('href', `/api/admin/auth/discord/start?next=${encodeURIComponent('/inbox?tab=self_service&sw=off')}`);
  await discord.click();
  await expect(page).toHaveURL(`${ADMIN}/inbox?tab=self_service&sw=off`);
  await expect(page.getByRole('listbox', { name: 'Self-service items' })).toBeVisible();
  await expect(page.locator('.account__name')).toHaveText('Asahi');
});

const LOGIN_ERRORS: [string, RegExp][] = [
  ['state', /expired or was already used/],
  ['denied', /cancelled/],
  ['forbidden', /isn't recognised as a Kanade admin/],
  ['discord', /Discord didn't complete the sign-in/],
  ['unavailable', /Discord sign-in is unavailable right now/],
  ['rate_limited', /Too many sign-in attempts/],
];

test('Discord: each login_error comes back as a clear message on the sign-in page', async ({ page }) => {
  await signOutBehind(page);
  for (const [code, words] of LOGIN_ERRORS) {
    await page.goto(`${ADMIN}/?login_error=${code}&sw=off`);
    await expect(page).toHaveURL(new RegExp(`/login\\?login_error=${code}`));
    await expect(page.getByRole('alert').filter({ hasText: words })).toBeVisible();
  }
  // Through the flow itself: the server refuses the account as not staff.
  await page.request.post(`${ADMIN}/__mock/discord`, { data: { error: 'forbidden' } });
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();
  await expect(page).toHaveURL(/\/login\?login_error=forbidden/);
  await expect(page.getByRole('alert').filter({ hasText: "Your Discord account isn't recognised as a Kanade admin." })).toBeVisible();
});

test('the session method decides the proposal controls up front', async ({ page }) => {
  await signOutBehind(page);
  await page.goto(`${ADMIN}/login?next=%2Finbox&sw=off`);
  await page.getByText('Break-glass: admin token').click();
  await page.getByLabel('Admin token').fill(TOKEN);
  await page.getByRole('button', { name: 'Sign in with the token' }).click();
  const detail = page.locator('.inbox__detail');
  await expect(detail.getByRole('heading', { level: 2 })).toContainText('Black Mage');
  await expect(detail.getByRole('button', { name: 'Approve', exact: true })).toBeDisabled();
  await expect(detail).toContainText("Sign in with Discord to approve or reject Kanade's proposals.");

  // A Discord session decides them.
  await page.request.post(`${ADMIN}/__mock/session`, { data: { method: 'discord' } });
  await page.goto(`${ADMIN}/inbox?sw=off`);
  await expect(detail.getByRole('button', { name: 'Approve', exact: true })).toBeEnabled();
});

test('sign out ends the session and shows sign-in', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('.account__name')).toHaveText('Asahi');
  await page.getByRole('button', { name: /Asahi/ }).click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await expect(page).toHaveURL(`${ADMIN}/login`);
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  expect((await page.request.get(`${ADMIN}/api/admin/session`)).status()).toBe(401);
  // Back into the app while signed out lands on sign-in again, returning to the page.
  await page.goto(`${ADMIN}/history?sw=off`);
  await expect(page).toHaveURL(`${ADMIN}/login?next=%2Fhistory%3Fsw%3Doff`);
});

test("signed out, the sign-in page shows tonight's run without anyone's name", async ({ page }) => {
  const week = await (await page.request.get(`${ADMIN}/api/admin/week`)).json();
  const carling = week.runs.find((run: { id: string }) => run.id === 'r-carling');
  const names: string[] = carling.participants.map((p: { name: string }) => p.name);
  expect(names.length).toBeGreaterThan(0);
  await signOutBehind(page);
  await page.goto(`${ADMIN}/login?sw=off`);
  const strip = page.locator('.gate__tonight');
  await expect(strip).toHaveText(/^\s*Tonight\s*22:00\s*Carling \+ Radiant Malefic Star\s*answered yes: 4\/7\s*$/);
  const body = (await page.locator('.gate__body').innerText()).toLowerCase();
  for (const name of names) expect(body).not.toContain(name.toLowerCase());
  await expect(page.locator('.gate__body')).not.toContainText('#');
});

test('the tonight strip renders nothing when the read fails or no run is today, and says Today for a daytime run', async ({ page }) => {
  await signOutBehind(page);
  await page.route('**/api/admin/auth/tonight', (route) => route.fulfill({ status: 503, json: { error: 'unavailable', message: 'down' } }));
  await page.goto(`${ADMIN}/login?sw=off`);
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  await expect(page.locator('.gate__tonight')).toHaveCount(0);
  await expect(page.getByRole('alert')).toHaveCount(0);

  await page.unroute('**/api/admin/auth/tonight');
  await page.route('**/api/admin/auth/tonight', (route) => route.fulfill({ json: { run: null } }));
  await page.reload();
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  await expect(page.locator('.gate__tonight')).toHaveCount(0);

  await page.unroute('**/api/admin/auth/tonight');
  await page.route('**/api/admin/auth/tonight', (route) =>
    route.fulfill({ json: { run: { time: '09:30', bosses: ['Lucid'], tally: { on: 1, total: 6 } } } }),
  );
  await page.reload();
  await expect(page.locator('.gate__tonight')).toHaveText(/^\s*Today\s*09:30\s*Lucid\s*answered yes: 1\/6\s*$/);
});
