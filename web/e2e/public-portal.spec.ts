import AxeBuilder from '@axe-core/playwright';
import type { Locator, Page } from '@playwright/test';
import { HEADING, PUBLIC, expect, setPortal, settle, signInPublic, test } from './support';

// The member portal's first step (docs/notes/member-auth-contract.md §6,
// decisions D4-A/D5-A) against the mock's public session routes. Every test
// also ends with zero CSP / Trusted Types reports (the auto `csp` fixture).

test.describe.configure({ mode: 'parallel' });

const COOKIE = 'kanade_pub';
/** Everything the portal may read in this step: no schedule, no art. */
const ALLOWED: Record<string, true> = {
  '/api/public/status': true,
  '/api/public/session': true,
  '/api/public/session/avatar': true,
  '/api/public/sessions': true,
  '/api/identity': true,
};

/** The API and art paths this page requested, in order. */
function requests(page: Page): string[] {
  const seen: string[] = [];
  page.on('request', (request) => {
    const { pathname } = new URL(request.url());
    if (pathname.startsWith('/api/') || pathname.startsWith('/art/')) seen.push(`${request.method()} ${pathname}`);
  });
  return seen;
}

async function sessionCookie(page: Page) {
  return (await page.context().cookies(PUBLIC)).find((c) => c.name === COOKIE);
}

async function serious(page: Page, label: string) {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

/** Tab from the top of the page until `target` has focus (a keyboard user's path). */
async function tabTo(page: Page, target: Locator, limit = 25) {
  await page.locator('body').focus();
  for (let i = 0; i < limit; i++) {
    await page.keyboard.press('Tab');
    if (await target.evaluate((el) => el === document.activeElement)) return;
  }
  throw new Error(`Tab never reached ${target}`);
}

async function openAccount(page: Page) {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.getByText('This device')).toBeVisible();
}

const devices = (page: Page) => page.getByRole('list', { name: 'Signed-in devices' }).getByRole('listitem');

/** The masthead's account button (the shared AccountMenu chip). */
const accountButton = (page: Page) => page.getByRole('banner').getByRole('button', { name: /^Account: / });

test('account menu: Account and Appearance move focus there; Sign out signs this device out', async ({ page }) => {
  await openAccount(page);
  const chip = accountButton(page);
  await chip.focus();
  await page.keyboard.press('Enter');
  const menu = page.getByRole('menu', { name: 'Account' });
  await expect(menu.getByRole('menuitem')).toHaveText(['Account', 'Appearance', 'Sign out']);
  await expect(menu.getByRole('menuitem', { name: 'Account' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(menu).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Appearance' })).toBeFocused();
  // Escape closes and gives focus back to the button.
  await chip.click();
  await page.keyboard.press('Escape');
  await expect(chip).toBeFocused();
  await chip.click();
  await menu.getByRole('menuitem', { name: 'Account' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Account' })).toBeFocused();
  await chip.click();
  await menu.getByRole('menuitem', { name: 'Sign out' }).click();
  await expect(page.locator('.gate .flash')).toHaveText("You're signed out on this device.");
  expect(await sessionCookie(page)).toBeUndefined();
});

test('signed out: Sign in with the privacy notice, and no schedule request', async ({ page }) => {
  const seen = requests(page);
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.public);
  const key = page.getByRole('link', { name: 'Sign in with Discord' });
  await expect(key).toHaveAttribute('href', '/api/public/auth/discord/start?next=%2F%3Fsw%3Doff');
  await expect(page.getByText(/We ask Discord only who you are \(the identify scope\)/)).toBeVisible();
  // The source wraps the sentence, so its text keeps a line break: match any whitespace.
  await expect(page.getByText(/We keep your Discord id, display name and avatar, not your email, and never post\s+as you/)).toBeVisible();
  await page.waitForLoadState('networkidle');
  expect(seen).toEqual(expect.arrayContaining(['GET /api/public/status', 'GET /api/public/session']));
  expect(seen.filter((r) => !ALLOWED[r.split(' ')[1]!])).toEqual([]);
  expect(seen.indexOf('GET /api/public/status')).toBeLessThan(seen.indexOf('GET /api/public/session'));
  expect(await sessionCookie(page)).toBeUndefined();
  // Signed out, the masthead names nobody: the bot's identity and this device's time zone only.
  await expect(accountButton(page)).toHaveCount(0);
  await expect(page.locator('.masthead__zone')).toContainText(await page.evaluate(() => Intl.DateTimeFormat().resolvedOptions().timeZone));
});

test('signing in through the Discord stand-in lands on Account with the avatar and name', async ({ page }) => {
  const seen = requests(page);
  await page.goto(`${PUBLIC}/?sw=off`);
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();
  // start → (no Discord in the mock) callback → landing → back to `next`.
  await expect(page).toHaveURL(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1, name: 'Account' })).toBeVisible();
  const chip = accountButton(page);
  await expect(chip).toHaveAccessibleName('Account: Asahi');
  await expect(chip).toContainText('Asahi');
  await expect(chip.locator('img')).toHaveAttribute('src', '/api/public/session/avatar?v=1');
  await expect.poll(() => chip.locator('img').evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
  const cookie = await sessionCookie(page);
  expect(cookie).toMatchObject({ httpOnly: true, sameSite: 'Strict' });
  // Three devices: this one first and marked, then the others; no IP or location anywhere.
  await expect(devices(page)).toHaveCount(3);
  await expect(devices(page).first()).toContainText('This device');
  await expect(devices(page).first()).toContainText('Chrome');
  await expect(page.locator('main')).not.toContainText(/\b\d{1,3}(\.\d{1,3}){3}\b|IP address|location/i);
  await page.waitForLoadState('networkidle');
  // Signed in, this step still reads no schedule (the week comes with member reads).
  expect(seen.filter((r) => r.startsWith('GET /api/') && !ALLOWED[r.split(' ')[1]!] && !r.includes('/auth/discord/'))).toEqual([]);
  expect(seen.some((r) => r.startsWith('GET /art/'))).toBe(false);
});

test('closed: the closed notice, sign-in hidden, and no session or schedule request', async ({ page }) => {
  await setPortal(page.request, false);
  const seen = requests(page);
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1, name: "The schedule isn't open right now" })).toBeVisible();
  await expect(page.getByRole('link', { name: /Sign in/ })).toHaveCount(0);
  await page.waitForLoadState('networkidle');
  expect(seen.filter((r) => r !== 'GET /api/public/status' && r !== 'GET /api/identity')).toEqual([]);
  // Closed means shell, status, identity and sign-out only.
  expect(await (await page.request.get(`${PUBLIC}/api/public/status`)).json()).toEqual({ portal: 'closed' });
  expect((await page.request.get(`${PUBLIC}/api/identity`)).status()).toBe(200);
  for (const path of ['/api/public/session', '/api/public/sessions', '/api/public/week', '/art/entry/Carling']) {
    const closed = await page.request.get(`${PUBLIC}${path}`);
    expect(closed.status(), path).toBe(503);
    expect(await closed.json(), path).toMatchObject({ error: 'closed' });
  }
  // Reopened: "Check again" moves on to Sign in.
  await setPortal(page.request, true);
  await page.getByRole('button', { name: 'Check again' }).click();
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
});

test('closed while signed in: the account gives way to the closed notice', async ({ page }) => {
  await openAccount(page);
  await setPortal(page.request, false);
  await devices(page).nth(1).getByRole('button', { name: /^Sign out/ }).click();
  await expect(page.getByRole('heading', { name: "The schedule isn't open right now" })).toBeVisible();
  await expect(accountButton(page)).toHaveCount(0);
});

test('denied: a neutral page and no session cookie; another account or try again', async ({ page }) => {
  await page.request.post(`${PUBLIC}/__mock/public/discord`, { data: { error: 'not_eligible' } });
  await page.goto(`${PUBLIC}/?sw=off`);
  const seen = requests(page);
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();
  await expect(page.getByRole('heading', { name: "This account can't see the schedule" })).toBeVisible();
  // The outcome is shown once: the address drops it, so a reload starts afresh.
  await expect(page).toHaveURL(`${PUBLIC}/`);
  expect(await page.context().cookies(PUBLIC)).toEqual([]);
  // Neutral: nothing about the guild or which role is missing.
  await expect(page.locator('main')).not.toContainText(/not in the guild|missing|bossing role is missing|not a member/i);
  // Denied reads no session (nobody is signed in) and no schedule.
  await page.waitForLoadState('networkidle');
  expect(seen.filter((r) => r === 'GET /api/public/session' || r === 'GET /api/public/sessions')).toEqual([]);
  expect(seen.filter((r) => !ALLOWED[r.split(' ')[1]!] && !r.includes('/auth/discord/'))).toEqual([]);
  await expect(page.getByRole('link', { name: 'Try again' })).toHaveAttribute('href', /^\/api\/public\/auth\/discord\/start\?next=/);
  await page.getByRole('button', { name: 'Use another account' }).click();
  await expect(page.locator('.gate .flash')).toContainText('switch to it in Discord first');
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
});

test('other sign-in outcomes: a generic notice on Sign in; closed lands on Closed', async ({ page }) => {
  for (const code of ['state', 'denied', 'discord', 'unavailable', 'rate_limited']) {
    await page.goto(`${PUBLIC}/?login_error=${code}&sw=off`);
    await expect(page.getByRole('alert')).toHaveText("Sign-in didn't finish. Try again in a moment.");
    await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  }
  await page.goto(`${PUBLIC}/?login_error=closed&sw=off`);
  await expect(page.getByRole('heading', { name: "The schedule isn't open right now" })).toBeVisible();
  await expect(page.getByRole('link', { name: /Sign in/ })).toHaveCount(0);
});

test('devices: sign out one, then sign out everywhere ends this one too', async ({ page }) => {
  await openAccount(page);
  const other = devices(page).filter({ hasText: 'Safari · iPhone' });
  await other.getByRole('button', { name: /^Sign out Safari · iPhone/ }).click();
  await expect(page.getByRole('group', { name: 'Notification' }).filter({ hasText: 'Signed out Safari · iPhone.' })).toBeVisible();
  await expect(devices(page)).toHaveCount(2);
  // Its button is gone: focus is on the list's heading, not lost to the page.
  await expect(page.getByRole('heading', { name: 'Signed-in devices' })).toBeFocused();
  // This device has no "Sign out" of its own in the list (the profile's Sign out ends it).
  await expect(devices(page).first().getByRole('button')).toHaveCount(0);

  await page.getByRole('button', { name: 'Sign out everywhere' }).click();
  await expect(page.locator('.gate .flash')).toHaveText("You're signed out everywhere: this device and 1 other.");
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  await expect(accountButton(page)).toHaveCount(0);
  await expect(page.locator('main')).toBeFocused();
  expect(await sessionCookie(page)).toBeUndefined();
  expect((await page.request.get(`${PUBLIC}/api/public/session`)).status()).toBe(401);
});

test('sign out: this device only, back to Sign in', async ({ page }) => {
  await openAccount(page);
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await expect(page.locator('.gate .flash')).toHaveText("You're signed out on this device.");
  expect(await sessionCookie(page)).toBeUndefined();
  await page.reload();
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
});

test('session ended mid-use: the Session ended screen, nothing of the account left', async ({ page }) => {
  await openAccount(page);
  // Expired, signed out elsewhere or no longer eligible: the server ended it.
  await page.request.post(`${PUBLIC}/__mock/public/end`);
  await devices(page).nth(1).getByRole('button', { name: /^Sign out/ }).click();
  await expect(page.getByRole('heading', { level: 1, name: "You've been signed out" })).toBeVisible();
  await expect(accountButton(page)).toHaveCount(0);
  await expect(page.getByText('Signed-in devices')).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Sign in again' })).toHaveAttribute('href', /^\/api\/public\/auth\/discord\/start\?next=/);
  await expect(page.locator('main')).toBeFocused();
});

test('a rotated session: writes carry the newest token any answer sent', async ({ page }) => {
  await openAccount(page);
  // As after a client IP change: the next request rotates the id and token.
  await page.request.post(`${PUBLIC}/__mock/public/rotate`);
  const sent: string[] = [];
  page.on('request', (request) => {
    if (request.method() === 'DELETE') sent.push(request.headers()['x-kanade-csrf'] ?? '');
  });
  const answered: string[] = [];
  page.on('response', (response) => {
    if (response.request().method() === 'DELETE') answered.push(response.headers()['x-kanade-csrf'] ?? '');
  });
  // The first sign-out rotates on the server and answers the new token…
  await devices(page).filter({ hasText: 'Firefox' }).getByRole('button').click();
  await expect(devices(page)).toHaveCount(2);
  // …which the second one carries (the old token would be refused with 403).
  await devices(page).filter({ hasText: 'Safari' }).getByRole('button').click();
  await expect(devices(page)).toHaveCount(1);
  expect(sent).toHaveLength(2);
  expect(answered[0]).not.toBe('');
  expect(sent[1]).toBe(answered[0]);
  expect(sent[0]).not.toBe(sent[1]);
});

test('appearance: colourways apply at once and survive a reload', async ({ page }) => {
  await openAccount(page);
  const ways = page.getByRole('group', { name: 'Colourway' });
  const terminal = ways.getByRole('button', { name: 'Terminal', exact: true });
  await terminal.click();
  await ways.getByText('Tokyo Night', { exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'tokyonight');
  await page.getByText('Dark', { exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'tokyonight');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

test('the service worker never caches /api/ and never answers it', async ({ page }) => {
  await signInPublic(page);
  // Controlled: the first load installs the worker, the reload is controlled.
  await page.goto(`${PUBLIC}/`);
  await page.evaluate(() => navigator.serviceWorker.ready);
  const api: { path: string; fromSW: boolean }[] = [];
  page.on('response', (response) => {
    const { pathname } = new URL(response.url());
    if (pathname.startsWith('/api/')) api.push({ path: pathname, fromSW: response.fromServiceWorker() });
  });
  await page.reload();
  await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);
  await expect(page.getByText('This device')).toBeVisible();
  await page.waitForLoadState('networkidle');
  expect(api.map((r) => r.path)).toEqual(expect.arrayContaining(['/api/public/status', '/api/public/session', '/api/public/sessions']));
  expect(api.filter((r) => r.fromSW)).toEqual([]);
  const cached = await page.evaluate(async () => {
    const urls: string[] = [];
    for (const name of await caches.keys()) for (const request of await (await caches.open(name)).keys()) urls.push(new URL(request.url).pathname);
    return urls;
  });
  expect(cached.length).toBeGreaterThan(5);
  expect(cached.filter((u) => u.startsWith('/api/'))).toEqual([]);
});

// Each screen: axe, a keyboard path to its main action with a visible focus
// ring, and the phone frame (no document scroll, nothing wider than the screen).
const SCREENS: { name: string; open: (page: Page) => Promise<void>; action: (page: Page) => Locator }[] = [
  {
    name: 'Sign in',
    open: async (page) => {
      await page.goto(`${PUBLIC}/?sw=off`);
      await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.public);
    },
    action: (page) => page.getByRole('link', { name: 'Sign in with Discord' }),
  },
  {
    name: 'Closed',
    open: async (page) => {
      await setPortal(page.request, false);
      await page.goto(`${PUBLIC}/?sw=off`);
      await expect(page.getByRole('heading', { name: "The schedule isn't open right now" })).toBeVisible();
    },
    action: (page) => page.getByRole('button', { name: 'Check again' }),
  },
  {
    name: 'Denied',
    open: async (page) => {
      await page.goto(`${PUBLIC}/?login_error=not_eligible&sw=off`);
      await expect(page.getByRole('heading', { name: "This account can't see the schedule" })).toBeVisible();
    },
    action: (page) => page.getByRole('button', { name: 'Use another account' }),
  },
  {
    name: 'Session ended',
    open: async (page) => {
      await openAccount(page);
      await page.request.post(`${PUBLIC}/__mock/public/end`);
      await devices(page).nth(1).getByRole('button').click();
      await expect(page.getByRole('heading', { name: "You've been signed out" })).toBeVisible();
    },
    action: (page) => page.getByRole('link', { name: 'Sign in again' }),
  },
  {
    name: 'Account',
    open: openAccount,
    action: (page) => page.getByRole('button', { name: 'Sign out everywhere' }),
  },
];

for (const screen of SCREENS) {
  for (const size of [
    { width: 1280, height: 800 },
    { width: 390, height: 844 },
  ]) {
    test(`${screen.name} at ${size.width}×${size.height}: axe, keyboard and fit`, async ({ page }) => {
      await page.setViewportSize(size);
      await screen.open(page);
      await serious(page, `${screen.name} ${size.width}`);
      const action = screen.action(page);
      await tabTo(page, action);
      await expect(action).toBeInViewport();
      expect(await action.evaluate((el) => el.matches(':focus-visible'))).toBe(true);
      const box = (await action.boundingBox())!;
      expect(box.height).toBeGreaterThanOrEqual(24);
      const fit = await page.evaluate(() => {
        window.scrollTo(0, 400);
        return { scrolled: document.scrollingElement!.scrollTop, wide: Math.max(0, document.documentElement.scrollWidth - innerWidth) };
      });
      expect(fit).toEqual({ scrolled: 0, wide: 0 });
    });
  }
}
