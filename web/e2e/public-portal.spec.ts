import AxeBuilder from '@axe-core/playwright';
import type { Locator, Page } from '@playwright/test';
import { HEADING, PUBLIC, expect, setPortal, settle, signInPublic, test } from './support';

// The member portal's first step (docs/notes/member-auth-contract.md §6,
// decisions D4-A/D5-A) against the mock's public session routes. Every test
// also ends with zero CSP / Trusted Types reports (the auto `csp` fixture).

test.describe.configure({ mode: 'parallel' });

const COOKIE = 'kanade_pub';
/** Everything the portal may read before sign-in or on Account alone: no schedule, no art. */
const ALLOWED: Record<string, true> = {
  '/api/public/status': true,
  '/api/public/session': true,
  '/api/public/session/avatar': true,
  '/api/public/sessions': true,
  '/api/identity': true,
};
/** Signed in, the member's own reads join them: the week, the allowance and boss art. */
const memberRead = (request: string) => {
  const path = request.split(' ')[1]!;
  return ALLOWED[path] || path === '/api/public/week' || path === '/api/public/me/allowance' || path.startsWith('/art/');
};
/** `name` as a whole word ("Ren", not "Render"). */
const word = (name: string) => new RegExp(`\\b${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\b`);

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

/** Signed in, on Account › Devices (the device list). */
async function openAccount(page: Page) {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/account?tab=devices&sw=off`);
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
  await expect(page.locator('.masthead__zone')).toContainText('Times in GMT');
  await expect(page.locator('.masthead__zone')).toHaveAttribute('title', `Times are in ${await page.evaluate(() => Intl.DateTimeFormat().resolvedOptions().timeZone)}`);
});

test('signing in through the Discord stand-in lands on the Week, with the avatar and name on the account chip', async ({ page }) => {
  const seen = requests(page);
  await page.goto(`${PUBLIC}/?sw=off`);
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();
  // start → (no Discord) callback → landing → back to `next`.
  await expect(page).toHaveURL(`${PUBLIC}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1, name: 'Week' })).toBeVisible();
  const chip = accountButton(page);
  await expect(chip).toHaveAccessibleName('Account: Asahi');
  await expect(chip).toContainText('Asahi');
  await expect(chip.locator('img')).toHaveAttribute('src', '/api/public/session/avatar?v=1');
  await expect.poll(() => chip.locator('img').evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
  const cookie = await sessionCookie(page);
  expect(cookie).toMatchObject({ httpOnly: true, sameSite: 'Strict' });
  // Account › Devices: this one first and marked, then the others; no IP or location anywhere.
  await chip.click();
  await page.getByRole('menu', { name: 'Account' }).getByRole('menuitem', { name: 'Account' }).click();
  await expect(page).toHaveURL(`${PUBLIC}/account`);
  await page.getByRole('tab', { name: /^Devices/ }).click();
  await expect(devices(page)).toHaveCount(3);
  await expect(devices(page).first()).toContainText('This device');
  await expect(devices(page).first()).toContainText('Chrome');
  await expect(page.locator('main')).not.toContainText(/\b\d{1,3}(\.\d{1,3}){3}\b|IP address|location/i);
  await page.waitForLoadState('networkidle');
  // Signed in, the portal reads the member's own views and nothing else.
  expect(seen).toEqual(expect.arrayContaining(['GET /api/public/week', 'GET /api/public/me/allowance']));
  expect(seen.filter((r) => r.startsWith('GET ') && !memberRead(r) && !r.includes('/auth/discord/'))).toEqual([]);
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

test('other sign-in outcomes: each code lands on its screen and notice kind, with the next step and no extra reads', async ({ page }) => {
  const seen = requests(page);
  const main = page.getByRole('main');
  const start = /^\/api\/public\/auth\/discord\/start\?next=/;
  /** Lands on `code`; then what it read was status, identity and (Sign in only) the session, nothing else. */
  async function land(code: string, readsSession: boolean) {
    seen.length = 0;
    await page.goto(`${PUBLIC}/?login_error=${code}&sw=off`);
    await expect(main.getByRole('heading', { level: 1 })).toHaveCount(1);
    await page.waitForLoadState('networkidle');
    expect(seen.filter((r) => !ALLOWED[r.split(' ')[1]!]), code).toEqual([]);
    expect(seen.includes('GET /api/public/session'), code).toBe(readsSession);
    expect(seen.includes('GET /api/public/sessions'), code).toBe(false);
    expect(await sessionCookie(page), code).toBeUndefined();
  }

  // Sign in with a notice: a failure is an alert, a wait or a fresh start a status; the key starts Discord again.
  for (const [code, kind] of [
    ['state', 'status'],
    ['rate_limited', 'status'],
    ['denied', 'alert'],
    ['discord', 'alert'],
    ['something_new', 'alert'],
  ] as const) {
    await land(code, true);
    await expect(main.getByRole(kind), code).toHaveCount(1);
    await expect(main.getByRole(kind === 'alert' ? 'status' : 'alert'), code).toHaveCount(0);
    const key = main.getByRole('link', { name: /with Discord$/ });
    await expect(key, code).toHaveAttribute('href', start);
    // Too many tries: the key says how long to wait, as its description.
    if (code === 'rate_limited') await expect(key).toHaveAccessibleDescription(/\S/);
    else await expect(key, code).not.toHaveAttribute('aria-describedby');
  }

  // Member data unavailable: an alert and Try again, no Discord key (nobody was refused).
  await land('unavailable', true);
  await expect(main.getByRole('alert')).toHaveCount(1);
  await expect(main.getByRole('link', { name: 'Try again', exact: true })).toHaveAttribute('href', start);
  await expect(main.getByRole('link', { name: /with Discord$/ })).toHaveCount(0);

  // Not eligible: neutral (no alert or status notice), another account or Try again; nobody is signed in.
  await land('not_eligible', false);
  await expect(main.getByRole('alert')).toHaveCount(0);
  await expect(main.getByRole('status')).toHaveCount(0);
  await expect(main.getByRole('button', { name: 'Use another account' })).toBeVisible();
  await expect(main.getByRole('link', { name: 'Try again', exact: true })).toHaveAttribute('href', start);

  // Closed: sign-in hidden; Check again re-reads the status.
  await land('closed', false);
  await expect(main.getByRole('link')).toHaveCount(0);
  await expect(main.getByRole('button', { name: 'Check again' })).toBeVisible();
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
  await page.getByRole('tab', { name: 'This browser' }).click();
  const ways = page.getByRole('group', { name: 'Colourway' });
  const terminal = ways.getByRole('button', { name: 'Terminal', exact: true });
  await terminal.click();
  await ways.getByRole('radio', { name: 'Tokyo Night' }).check();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'tokyonight');
  await page.getByRole('group', { name: 'Mode' }).getByRole('radio', { name: 'Dark' }).check();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'tokyonight');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

test('the service worker stores no member data: no /api/, no /art/, no names, in any cache or storage', async ({ page }) => {
  await signInPublic(page);
  // Every name and run the member could see, from the week answers themselves.
  const names = new Set<string>();
  const runIds = new Set<string>();
  const passed: { path: string; fromSW: boolean }[] = [];
  page.on('response', async (response) => {
    const { pathname } = new URL(response.url());
    if (!pathname.startsWith('/api/') && !pathname.startsWith('/art/')) return;
    passed.push({ path: pathname, fromSW: response.fromServiceWorker() });
    if (pathname === '/api/public/week' && response.ok()) {
      const week = (await response.json()) as { runs: { id: string; participants: { name: string }[] }[] };
      for (const run of week.runs) {
        runIds.add(run.id);
        for (const p of run.participants) names.add(p.name);
      }
    }
  });
  // Controlled: the first load installs the worker, the reload is controlled.
  await page.goto(`${PUBLIC}/`);
  await page.evaluate(() => navigator.serviceWorker.ready);
  await page.reload();
  await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);

  // Browse: the Week, an own run and another's, My runs, Account.
  const cards = page.locator('main [data-run]');
  await expect(cards.first()).toBeVisible();
  await page.getByRole('button', { name: /, you are in$/ }).first().click();
  await expect(page.getByRole('complementary', { name: /^Your run · / })).toBeVisible();
  await page.getByRole('button', { name: /, not in this run, view only$/ }).first().click();
  await expect(page.getByRole('complementary', { name: /^View only · / })).toBeVisible();
  await page.getByRole('tab', { name: /^List/ }).click();
  await page.getByRole('banner').getByRole('link', { name: /^My runs/ }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'My runs' })).toBeVisible();
  await accountButton(page).click();
  await page.getByRole('menu', { name: 'Account' }).getByRole('menuitem', { name: 'Account' }).click();
  await page.getByRole('tab', { name: /^Devices/ }).click();
  await expect(page.getByText('This device')).toBeVisible();
  await page.waitForLoadState('networkidle');

  expect(names.size).toBeGreaterThan(3);
  expect(passed.map((r) => r.path)).toEqual(expect.arrayContaining(['/api/public/week', '/api/public/me/allowance', '/api/public/sessions']));
  expect(passed.some((r) => r.path.startsWith('/art/'))).toBe(true);
  // The worker answered none of them: each went to the network.
  expect(passed.filter((r) => r.fromSW)).toEqual([]);

  const stored = await page.evaluate(async () => {
    const cached: { url: string; type: string; body: string }[] = [];
    for (const name of await caches.keys()) {
      const cache = await caches.open(name);
      for (const request of await cache.keys()) {
        const response = await cache.match(request);
        const type = response?.headers.get('content-type') ?? '';
        cached.push({ url: request.url, type, body: /html|json|text\/plain/.test(type) ? ((await response?.text()) ?? '') : '' });
      }
    }
    const local = Object.entries({ ...localStorage });
    const session = Object.entries({ ...sessionStorage });
    const databases: { name: string; rows: string[] }[] = [];
    for (const info of await indexedDB.databases()) {
      const db = await new Promise<IDBDatabase>((resolve, reject) => {
        const open = indexedDB.open(info.name!);
        open.onsuccess = () => resolve(open.result);
        open.onerror = () => reject(open.error);
      });
      const rows: string[] = [];
      for (const store of Array.from(db.objectStoreNames)) {
        const all = await new Promise<unknown[]>((resolve, reject) => {
          const read = db.transaction(store).objectStore(store).getAll();
          read.onsuccess = () => resolve(read.result);
          read.onerror = () => reject(read.error);
        });
        rows.push(...all.map((row) => JSON.stringify(row)));
      }
      db.close();
      databases.push({ name: info.name!, rows });
    }
    return { cached, local, session, databases };
  });
  // Caches hold the precached static build only.
  expect(stored.cached.length).toBeGreaterThan(5);
  for (const { url } of stored.cached) {
    const { pathname } = new URL(url);
    expect(pathname, url).not.toMatch(/^\/(api|art)\//);
    expect(pathname, url).toMatch(/\.(js|css|html|woff2|png|webmanifest)$/);
  }
  // Nothing stored anywhere names a member or a run, or holds a member read.
  const pages = stored.cached.map((c) => `${c.url} ${c.body}`).join('\n');
  const kept = [...stored.local.flat(), ...stored.session.flat(), ...stored.databases.flatMap((d) => [d.name, ...d.rows])].join('\n');
  for (const name of names) expect(`${pages}\n${kept}`, name).not.toMatch(word(name));
  for (const id of runIds) expect(kept, id).not.toContain(id);
  for (const path of ['/api/public/', '/art/']) expect(`${pages}\n${kept}`, path).not.toContain(path);

  // Sign-out drops the last-seen week from memory: nothing of it is left on screen, nor after Back.
  await accountButton(page).click();
  await page.getByRole('menu', { name: 'Account' }).getByRole('menuitem', { name: 'Sign out' }).click();
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  await page.goBack();
  await page.goBack();
  await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
  const text = await page.locator('body').innerText();
  for (const name of names) expect(text, name).not.toMatch(word(name));
  await expect(page.locator('[data-run], [data-row]')).toHaveCount(0);
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
