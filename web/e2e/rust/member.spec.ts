import type { BrowserContext, Page } from '@playwright/test';
import { MEMBER, expect, test } from './support';

// The fixture's fake Discord approves at once; this cookie picks who signs in.
async function asDiscordUser(context: BrowserContext, who: 'eligible' | 'ineligible' | 'bot' | 'deny'): Promise<void> {
  await context.addCookies([{ name: 'kanade_fake_discord', value: who, domain: '127.0.0.1', path: '/' }]);
}

// Unfiltered: `cookies(url)` hides Secure cookies from an http:// URL, and the fixture serves http.
async function sessionCookie(context: BrowserContext): Promise<string | undefined> {
  return (await context.cookies()).find((cookie) => cookie.name === '__Host-kanade_pub')?.value;
}

async function session(page: Page): Promise<number> {
  return page.evaluate(async () => (await fetch('/api/public/session', { credentials: 'same-origin' })).status);
}

test.describe.configure({ mode: 'serial' });

test('member: an eligible member signs in through Discord, reads the week, sees this device, and signs out everywhere', async ({ page, context }) => {
  await asDiscordUser(context, 'eligible');
  await page.goto(`${MEMBER}/`);
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();

  // Signed in, the portal lands on the member Week, read from the real server.
  await expect(page.getByRole('heading', { level: 1, name: 'Week' })).toBeVisible();
  const week = await page.evaluate(async () => {
    const reply = await fetch('/api/public/week', { credentials: 'same-origin' });
    return { status: reply.status, cache: reply.headers.get('cache-control'), body: await reply.json() };
  });
  expect(week.status).toBe(200);
  expect(week.cache).toBe('no-store');
  expect(Object.keys(week.body).sort()).toEqual(['days', 'generated_at', 'reset', 'runs', 'starts', 'timezone', 'version']);

  await page.goto(`${MEMBER}/account?tab=devices`);
  await expect(page.getByText('Mikan').first()).toBeVisible();
  await expect(page.getByText('This device')).toBeVisible();
  const cookie = (await context.cookies()).find((entry) => entry.name === '__Host-kanade_pub');
  expect(cookie).toMatchObject({ httpOnly: true, secure: true, sameSite: 'Strict', path: '/' });
  expect(await session(page)).toBe(200);
  // The admin realm is not on this listener.
  expect(await page.evaluate(async () => (await fetch('/api/admin/session')).status)).toBe(404);

  await page.getByRole('button', { name: 'Sign out everywhere' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Sign in' })).toBeVisible();
  expect(await sessionCookie(context)).toBeUndefined();
  expect(await session(page)).toBe(401);
});

test('member: a member without the bossing role is denied and gets no session', async ({ page, context }) => {
  await asDiscordUser(context, 'ineligible');
  await page.goto(`${MEMBER}/`);
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();

  await expect(page.getByRole('heading', { name: "This account can't see the schedule" })).toBeVisible();
  expect(await sessionCookie(context)).toBeUndefined();
  expect(await session(page)).toBe(401);
});

test('member: cancelling at Discord returns to sign-in without a session', async ({ page, context }) => {
  await asDiscordUser(context, 'deny');
  await page.goto(`${MEMBER}/`);
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();

  await expect(page.getByRole('heading', { level: 1, name: 'Sign in' })).toBeVisible();
  expect(await sessionCookie(context)).toBeUndefined();
});
