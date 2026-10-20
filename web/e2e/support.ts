import { expect, test as base, type APIRequestContext, type APIResponse, type BrowserContext, type Locator, type Page, type Route } from '@playwright/test';

/**
 * This worker's own e2e server (see the port scheme in playwright.config.ts):
 * admin on base + 2 × parallel index, public one above. Workers set
 * TEST_PARALLEL_INDEX before any test file loads.
 */
function origins(): { admin: string; public: string } {
  const port = Number(process.env.KANADE_E2E_ORIGIN_BASE ?? '4373') + 2 * Number(process.env.TEST_PARALLEL_INDEX ?? '0');
  return { admin: `http://127.0.0.1:${port}`, public: `http://127.0.0.1:${port + 1}` };
}
export const ADMIN = origins().admin;
export const PUBLIC = origins().public;
/** Must match playwright.config.ts; every reset checks the server clock is back at it. */
export const PINNED_NOW = '2026-09-29T04:00:00Z';
export const REAL_ART = process.env.KANADE_REAL_ART === '1';
/** First-load page headings under the pinned clock: admin's week hides its
 * done and cancelled runs (v4); the public portal opens signed out on Sign in. */
export const HEADING = { admin: '7 runs', public: 'Sign in to see the boss week' } as const;
/** The member a public sign-in becomes, and the admin every spec browses as: the seed's Asahi. */
export const ASAHI_SNOWFLAKE = '100000000000001001';
/** The seed's required Setup steps left: the gateway never connects and one channel lacks a permission. */
export const SETUP_LEFT = 2;
/** The rail's More item by name: on one of its pages it names that page, and its badge adds "N waiting" (Setup's steps left). */
export const moreNamed = (current?: string) => new RegExp(`^More${current ? `, current page ${current}` : ''}(, \\d+ waiting)?$`);

/** The harness's fake Discord page (`examples/e2e_server/fakes/discord.rs`). */
const FAKE_DISCORD_PATH = '/__e2e/discord';

/**
 * Who this browser is at Discord, for the fake Discord page: a
 * seeded member's snowflake (Asahi by default), or one of the page's own
 * choices: `eligible` (Mikan: a bosser, not staff), `ineligible` (Plain: in
 * the guild without the bossing role), `deny` (cancelled at Discord). The
 * cookie is scoped to the fake page's path, so neither origin's own cookies
 * (`context.cookies(origin)`) ever list it.
 */
export async function useDiscordAccount(context: BrowserContext, account: string = ASAHI_SNOWFLAKE): Promise<void> {
  await context.addCookies([{ name: 'kanade_fake_discord', value: account, domain: '127.0.0.1', path: FAKE_DISCORD_PATH }]);
}

/**
 * The harness's Discord sign-in for `origin`: its fake Discord page approves
 * the user the `kanade_fake_discord` cookie names (Asahi). `page.request`
 * shares the context's cookie jar; the redirects are followed one hop at a
 * time so each hop sends the cookies the last one set (see `sendSecureCookies`).
 */
async function discordSignIn(page: Page, origin: string, realm: 'admin' | 'public', account = ASAHI_SNOWFLAKE): Promise<void> {
  await useDiscordAccount(page.context(), account);
  let url = `${origin}/api/${realm}/auth/discord/start`;
  for (let hop = 0; hop < 5; hop++) {
    const location = (await page.request.get(url, { maxRedirects: 0 })).headers()['location'];
    if (!location) break;
    url = new URL(location, url).href;
  }
  expect(new URL(url).searchParams.get('login_error'), `the Discord sign-in outcome (${url})`).toBeNull();
  expect((await page.request.get(`${origin}/api/${realm}/session`)).status(), `the ${realm} session after sign-in`).toBe(200);
}

/** Moves the server clock by `seconds` (`/__e2e/clock`). */
async function moveClock(seconds: number): Promise<void> {
  const moved = await fetch(`${ADMIN}/__e2e/clock`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ advance_s: seconds }) });
  expect(moved.status, 'the clock change').toBe(200);
}

/** How far back a stale sign-in happens: past the portal's 15 minute fresh-write window. */
const STALE_S = 35 * 60;

/**
 * Signs this page's browser context in on the public origin as Asahi through
 * the real Discord flow. `stale` makes it a sign-in older than the
 * fresh-write window: the server clock goes 35 minutes back for the sign-in,
 * then forward again, so every later read (session and devices alike) dates
 * it the same.
 */
export async function signInPublic(page: Page, options: { stale?: boolean } = {}): Promise<void> {
  if (!options.stale) return discordSignIn(page, PUBLIC, 'public');
  await moveClock(-STALE_S);
  try {
    await discordSignIn(page, PUBLIC, 'public');
  } finally {
    await moveClock(STALE_S);
  }
}

/** `/__e2e/whoami` on the admin origin. */
async function whoami(): Promise<unknown> {
  return (await fetch(`${ADMIN}/__e2e/whoami`)).json();
}

/** The break-glass admin token (the secrets file `admin_token`). */
export async function adminToken(): Promise<string> {
  const token = field(field(await whoami(), 'break_glass'), 'token');
  if (typeof token !== 'string') throw new Error('whoami has no break-glass token');
  return token;
}

/**
 * Signs this context in on the admin origin, replacing its
 * session: `discord` through the fake Discord page as `account` (a staff
 * member: Asahi), `token` with the break-glass token, `tailscale` as the
 * allowed Tailscale login through the trusted edge. The edge stamps every
 * request with its secret and the Tailscale identity, so from then on the
 * whole context sends them too (a Tailscale session needs them on each request).
 */
export async function signInAdmin(page: Page, method: 'discord' | 'token' | 'tailscale', account = ASAHI_SNOWFLAKE): Promise<void> {
  if (method === 'discord') return discordSignIn(page, ADMIN, 'admin', account);
  let response: APIResponse;
  if (method === 'token') {
    response = await page.request.post(`${ADMIN}/api/admin/auth/token`, { headers: { Origin: ADMIN }, data: { token: await adminToken() } });
  } else {
    const tailscale = field(await whoami(), 'tailscale');
    const edge = {
      'X-Kanade-Edge-Auth': String(field(tailscale, 'edge_secret')),
      'X-Forwarded-For': '100.64.0.9',
      'Tailscale-User-Login': String(field(tailscale, 'login')),
    };
    await page.context().setExtraHTTPHeaders(edge);
    response = await page.request.post(`${ADMIN}/api/admin/auth/tailscale`, { headers: { Origin: ADMIN, ...edge } });
  }
  expect(response.status(), `the ${method} sign-in (${await response.text()})`).toBe(200);
}

/** The real admin sign-out of this context's session (`POST /api/admin/auth/logout`). */
export async function signOutAdmin(page: Page): Promise<void> {
  const response = await page.request.post(`${ADMIN}/api/admin/auth/logout`, { headers: await csrf(page.request) });
  expect(response.ok(), `the sign-out (${response.status()})`).toBe(true);
}

/** The browser forgets its sessions (both origins' cookies), as if signed out behind the page's back. */
export async function dropSessions(context: BrowserContext): Promise<void> {
  await context.clearCookies();
}

/** The server ends every member-portal session (expired, or signed out elsewhere). */
export async function endMemberSessions(): Promise<void> {
  const ended = await fetch(`${ADMIN}/__e2e/public/end`, { method: 'POST' });
  expect(ended.status, await ended.text()).toBe(200);
}

/** Restarts the server with these optional modules (serve reads them once, at startup). */
export async function setModules(modules: { roster?: boolean; character_data?: boolean; guides?: boolean }): Promise<void> {
  const set = await fetch(`${ADMIN}/__e2e/modules`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(modules) });
  expect(set.status, await set.text()).toBe(200);
}

/** The canned governor groups Limits shows (`three`: cloud, local, legacy; `default`: one gateway group). */
export async function setLimitGroups(groups: 'three' | 'default'): Promise<void> {
  const set = await fetch(`${ADMIN}/__e2e/limits`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ groups }) });
  expect(set.status).toBe(204);
}

/**
 * How the fake model gateway lists its models, on a restart (`kanata`: with Kanata metadata, the reset's;
 * `plain`: the same ids as a bare OpenAI listing; `no_listing`: `/v1/models` answers 404). The server
 * declares every model's capabilities, so the plain modes run on the declarations.
 */
export async function setGateway(mode: 'kanata' | 'plain' | 'no_listing'): Promise<void> {
  const set = await fetch(`${ADMIN}/__e2e/gateway`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ mode }) });
  expect(set.status, await set.text()).toBe(204);
}

/** A harness control that answers 204 (`examples/e2e_server/controls_p5.rs`). */
async function control(path: string, body: unknown): Promise<void> {
  const set = await fetch(`${ADMIN}${path}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  expect(set.status, `${path} ${JSON.stringify(body)}: ${await set.text()}`).toBe(204);
}

/**
 * What later scouter lookups do (`ok`, `paced`: each slower than the page's poll,
 * `assumed`: `ign`'s result assumes 100% critical rate, `hold`, or the next single lookup fails:
 * `busy`, `critical_rate`, `assumption_failed`, `manual_check`, `failed`, `timeout`), `queued` (two other members'
 * lookups wait, then finish one by one once a lookup is queued behind them) or `off` (a restart
 * without the scouter URL).
 */
export async function setEstimates(scenario: string, ign?: string): Promise<void> {
  await control('/__e2e/estimates', { scenario, ...(ign && { ign }) });
}

/**
 * A finished lookup of a run's party or one roster character, fetched `days` (1–30) before the
 * server clock (`aged`): at the saved `estimate_days` (7 by default) or more it reads "older than n days".
 */
export async function ageEstimate(target: { run: string } | { character: string }, days: number): Promise<void> {
  await control('/__e2e/estimates', { scenario: 'aged', days, ...target });
}

/**
 * Config's character-data health through real calls on a restart (`ok`, `alert`,
 * `refused`, `setup`: no NEXON key and no scouter URL), or `budget`: the day's NEXON calls spent.
 */
export async function setCharacterData(health: 'ok' | 'alert' | 'refused' | 'setup' | 'budget'): Promise<void> {
  await control('/__e2e/character-data', { health });
}

/** Asahi's own characters as the member boards draw them (`board`, ids `ch-b1`…`ch-b5`), or none. */
export async function setMemberCharacters(roster: 'board' | 'empty'): Promise<void> {
  await control('/__e2e/public/characters', { roster });
}

/** Moves the server clock on by `seconds` (it stands still otherwise). */
export async function advanceClock(seconds: number): Promise<void> {
  await moveClock(seconds);
}

/**
 * The real server's session cookies are `__Host-` (Secure). Chrome sends
 * Secure cookies to http://127.0.0.1, but Playwright's API requests send them
 * over http only to `localhost`, and the e2e origins are 127.0.0.1. So
 * every `context.request` (= `page.request`) call carries the
 * context's 127.0.0.1 cookies itself; Playwright still stores the Set-Cookie
 * answers. An explicit `Cookie` header from the caller wins.
 */
function sendSecureCookies(context: BrowserContext): void {
  const request = context.request;
  for (const method of ['fetch', 'get', 'post', 'put', 'patch', 'delete', 'head'] as const) {
    const send = request[method].bind(request) as (url: string, options?: { headers?: Record<string, string> }) => Promise<APIResponse>;
    const withCookies = async (url: string, options: { headers?: Record<string, string> } = {}) => {
      const cookies = (await context.cookies()).filter((cookie) => cookie.domain === '127.0.0.1').map((cookie) => `${cookie.name}=${cookie.value}`);
      return send(url, { ...options, headers: { ...(cookies.length > 0 && { cookie: cookies.join('; ') }), ...options.headers } });
    };
    Object.assign(request, { [method]: withCookies });
  }
}

/** The admin Config switch that opens and closes the public portal (D7-B). */
export async function setPortal(request: APIRequestContext, open: boolean): Promise<void> {
  const response = await request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(request), data: { self_service: { public_portal: open } } });
  expect(response.status()).toBe(200);
}

/** Opens an admin path with the service worker off. */
export async function go(page: Page, path: string): Promise<void> {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

/** The toast saying `text`. */
export const toast = (page: Page, text: string | RegExp) => page.getByRole('group', { name: 'Notification' }).filter({ hasText: text });

/** Every page this test opens starts in this colourway and theme (seeded before the app reads them). */
export async function useLook(page: Page, colorway: string, theme = 'light'): Promise<void> {
  await page.addInitScript(
    ([c, t]) => {
      localStorage.setItem('colorway', c!);
      localStorage.setItem('theme', t!);
    },
    [colorway, theme],
  );
}

/** The capture specs' frames and faces (blossom light is the face the user reviews the live site in). */
export const CAPTURE_VIEWPORTS = [
  { name: 'wide', width: 1280, height: 800 },
  { name: 'narrow', width: 390, height: 844 },
];
export const LOOKS = [
  { name: 'marigold-light', colorway: 'marigold', theme: 'light' },
  { name: 'marigold-dark', colorway: 'marigold', theme: 'dark' },
  { name: 'twilight-dark', colorway: 'twilight', theme: 'dark' },
  { name: 'blossom-light', colorway: 'blossom', theme: 'light' },
];

export interface Violation {
  directive: string;
  blocked: string;
  disposition: string;
  sample: string;
}

/** The JSON body's field, or undefined when the body is not an object. */
function field(body: unknown, key: string): unknown {
  return body !== null && typeof body === 'object' ? Reflect.get(body, key) : undefined;
}

/** The CSP report sink the server keeps for both origins. */
const REPORTS = `${ADMIN}/__e2e/reports`;

async function serverReports(): Promise<unknown[]> {
  return (await (await fetch(REPORTS)).json()) as unknown[];
}

/**
 * Fails loudly unless both origins are this worker's e2e server. Answers
 * Asahi's Discord admin session cookie every template copy holds (whoami's
 * `break_glass` has the token session too). The clock is checked after the
 * reset, which puts it back.
 */
async function verifyServer(): Promise<{ name: string; value: string }> {
  let cookie: { name: string; value: string } | undefined;
  for (const [origin, kind] of [
    [ADMIN, 'admin'],
    [PUBLIC, 'public'],
  ] as const) {
    let who: unknown;
    try {
      who = await (await fetch(`${origin}/__e2e/whoami`)).json();
    } catch {
      /* reported below */
    }
    if (field(who, 'server') !== 'kanade-e2e-server' || field(who, 'origin') !== kind)
      throw new Error(`${origin} is not the e2e server (${JSON.stringify(who)}); expected its ${kind} origin. Is something else on the e2e ports?`);
    const admin = field(who, 'admin_cookie');
    const name = field(admin, 'name');
    const value = field(admin, 'value');
    if (kind === 'admin') {
      if (typeof name !== 'string' || typeof value !== 'string') throw new Error(`${origin}/__e2e/whoami has no admin_cookie (${JSON.stringify(who)})`);
      cookie = { name, value };
    }
  }
  return cookie!;
}

/**
 * A direct admin write needs the session's CSRF token, as the PWA sends it
 * (API-5), and the page's `Origin`, which the real server also requires
 * (Playwright's API requests send neither `Origin` nor `Sec-Fetch-Site`).
 */
export async function csrf(request: APIRequestContext): Promise<Record<string, string>> {
  const session = await request.get(`${ADMIN}/api/admin/session`);
  return { 'X-Kanade-CSRF': session.headers()['x-kanade-csrf'] ?? '', Origin: ADMIN };
}

/** Puts the server back at the seed: a restart over a fresh template copy, its clock back at the pinned instant. */
export async function resetWeek(): Promise<void> {
  const reset = await fetch(`${ADMIN}/__e2e/reset`, { method: 'POST' });
  const body: unknown = reset.status === 200 ? await reset.json() : await reset.text();
  expect(reset.status, JSON.stringify(body)).toBe(200);
  expect(field(body, 'now'), 'the server clock after a reset').toBe(PINNED_NOW);
}

interface Sink {
  violations: Violation[];
  console: string[];
  /** Set by any document of the page, including ones navigated away from. */
  seen: boolean;
}

/** Every page records `securitypolicyviolation` events (CSP and Trusted Types, all enforced). */
async function watch(page: Page, sink: Sink) {
  await page.exposeFunction('__kanadeCspSeen', () => {
    sink.seen = true;
  });
  await page.addInitScript(() => {
    const w = window as unknown as { __csp: unknown[]; __kanadeCspSeen?: () => void };
    w.__csp = [];
    document.addEventListener('securitypolicyviolation', (e) => {
      w.__csp.push({ directive: e.violatedDirective, blocked: e.blockedURI, disposition: e.disposition, sample: e.sample });
      w.__kanadeCspSeen?.();
    });
  });
  page.on('console', (msg) => {
    const text = msg.text();
    if (/content security policy|trusted type|refused to/i.test(text)) sink.console.push(text);
  });
}

export async function collect(page: Page): Promise<Violation[]> {
  if (page.isClosed()) return [];
  try {
    return await page.evaluate(() => (window as unknown as { __csp?: Violation[] }).__csp ?? []);
  } catch {
    return [];
  }
}

/**
 * Waits for running finite animations and transitions (bounded), after two
 * frames so ones started by the last action have begun. Infinite ones never
 * finish and are skipped.
 */
export async function settle(page: Page, timeout = 2_000): Promise<void> {
  await page.evaluate(async (ms) => {
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const finite = document.getAnimations().filter((a) => a.effect?.getComputedTiming().endTime !== Infinity);
    await Promise.race([Promise.all(finite.map((a) => a.finished.catch(() => {}))), new Promise((resolve) => setTimeout(resolve, ms))]);
  }, timeout);
}

export const test = base.extend<{ cspControl: boolean; csp: Sink }>({
  /** Positive control: a test that deliberately violates the policy sets this. */
  cspControl: [false, { option: true }],
  // A spec's own `request` is the context's, so it carries the admin session the fixture adds
  // (the real API refuses an anonymous admin call).
  request: async ({ context }, use) => use(context.request),
  csp: [
    async ({ page, context, cspControl }, use) => {
      const admin = await verifyServer();
      await resetWeek();
      await fetch(REPORTS, { method: 'DELETE' });
      // The server's admin session (Asahi's Discord sign-in), as a browser that signed in holds it.
      // `__Host-` cookies are Secure and host-only; cookies ignore ports, so both origins send it.
      await context.addCookies([{ ...admin, domain: '127.0.0.1', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
      sendSecureCookies(context);
      const sink: Sink = { violations: [], console: [], seen: false };
      await watch(page, sink);
      await use(sink);
      sink.violations.push(...(await collect(page)));
      // report-uri POSTs land asynchronously. Only a page that saw a violation
      // waits for them; a clean page reads the store at once (any report that
      // already landed, e.g. from a worker, still fails the test below).
      if (sink.seen || sink.violations.length > 0 || sink.console.length > 0) {
        const landed = (text: string) => (cspControl ? text.includes('style-src-attr') && text.includes('require-trusted-types-for') : text !== '[]');
        for (let waited = 0; waited < 3_000 && !landed(JSON.stringify(await serverReports())); waited += 100)
          await new Promise((resolve) => setTimeout(resolve, 100));
      }
      const server = await serverReports();
      if (cspControl) {
        // The server must have received both kinds of report the control provoked.
        const text = JSON.stringify(server);
        expect(text).toContain('style-src-attr');
        expect(text).toContain('require-trusted-types-for');
        return;
      }
      expect.soft(sink.console, 'CSP / Trusted Types console messages').toEqual([]);
      expect.soft(sink.violations, 'securitypolicyviolation events').toEqual([]);
      expect(server, 'CSP / Trusted Types reports received by the server').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };

// The dropdown (Select): a combobox button with a listbox popover, or on phones
// a native <select> under the pill. These drive either by value, label or index.
type Pick = string | { label: string | RegExp } | { index: number };
const exactly = (label: string | RegExp) => (typeof label === 'string' ? new RegExp(`^\\s*${label.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*$`) : label);
// A label also names the open listbox (and the search box's "Filter …"): keep the trigger only.
const trigger = (box: Locator) => box.and(box.page().locator('button.dd, select'));
const isNative = (box: Locator) => trigger(box).evaluate((el) => el.tagName === 'SELECT');

/** Opens a Select and returns its listbox. */
export async function openList(box: Locator): Promise<Locator> {
  box = trigger(box);
  if ((await box.getAttribute('aria-expanded')) !== 'true') await box.click();
  const list = box.page().locator(`[role="listbox"][id="${await box.getAttribute('aria-controls')}"]`);
  await expect(list).toBeVisible();
  return list;
}

/** One option of an open Select, by value, label or index. */
export function optionIn(list: Locator, pick: Pick): Locator {
  const options = list.getByRole('option');
  if (typeof pick === 'string') return list.locator(`[role="option"][data-value="${pick.replace(/"/g, '\\"')}"]`);
  if ('index' in pick) return options.nth(pick.index);
  return options.filter({ has: list.page().locator('.dd-opt__label', { hasText: exactly(pick.label) }) });
}

/** Picks an option, as `selectOption` did for the native select. */
export async function choose(box: Locator, pick: Pick): Promise<void> {
  box = trigger(box);
  if (await isNative(box)) {
    await box.selectOption(typeof pick === 'string' ? pick : 'index' in pick ? { index: pick.index } : { label: pick.label as string });
    return;
  }
  await optionIn(await openList(box), pick).click();
  await expect(box).toHaveAttribute('aria-expanded', 'false');
}

/** The option labels in order (the Select is closed again afterwards). */
export async function optionLabels(box: Locator): Promise<string[]> {
  box = trigger(box);
  if (await isNative(box)) return box.locator('option').allTextContents();
  const list = await openList(box);
  const labels = (await list.locator('.dd-opt__label').allTextContents()).map((t) => t.trim());
  await box.press('Escape');
  return labels;
}

/** Asserts the chosen value (the trigger's data-value, or the native value). */
export async function expectValue(box: Locator, value: string): Promise<void> {
  box = trigger(box);
  if (await isNative(box)) await expect(box).toHaveValue(value);
  else await expect(box).toHaveAttribute('data-value', value);
}

/** Toggles options of a multi-select by label, then closes it. */
export async function toggleOptions(box: Locator, labels: string[]): Promise<void> {
  const list = await openList(box);
  for (const label of labels) await optionIn(list, { label }).click();
  await trigger(box).press('Escape');
}

/**
 * Options for `route.fetch` without the app's `If-None-Match`: a test that
 * rewrites a read's body needs the body, never a `304` for the copy the app holds.
 */
export function unconditional(route: Route): { headers: Record<string, string> } {
  const headers = { ...route.request().headers() };
  delete headers['if-none-match'];
  return { headers };
}

/** The server clock stands at `at` (RFC 3339) until moved again or reset (`/__e2e/clock`). */
export async function setClock(at: string): Promise<void> {
  const set = await fetch(`${ADMIN}/__e2e/clock`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ at }) });
  expect(set.status, await set.text()).toBe(200);
}

/**
 * Records every EventSource the page opens (call before `goto`), so
 * `streamOpen` can tell when one is up: the real server holds a stream open,
 * so it never finishes and never reaches the resource timeline.
 */
export async function trackStreams(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const Native = window.EventSource;
    const opened: EventSource[] = [];
    Object.defineProperty(window, '__e2eStreams', { value: opened });
    window.EventSource = class extends Native {
      constructor(url: string | URL, init?: EventSourceInit) {
        super(url, init);
        opened.push(this);
      }
    } as typeof EventSource;
  });
}

/** Waits until one of the page's event streams on `path` is open (`trackStreams` first). */
export async function streamOpen(page: Page, path: string): Promise<void> {
  const open = (p: string) => {
    const streams: unknown = Reflect.get(window, '__e2eStreams');
    return Array.isArray(streams) && streams.some((s) => s instanceof EventSource && s.readyState === EventSource.OPEN && new URL(s.url).pathname === p);
  };
  await expect.poll(() => page.evaluate(open, path), { message: `an open ${path} stream` }).toBe(true);
}

/** Signs this context in on the public origin as `account` (a seeded member's snowflake) through the fake Discord page. */
export async function signInPublicAs(page: Page, account: string): Promise<void> {
  await discordSignIn(page, PUBLIC, 'public', account);
}
