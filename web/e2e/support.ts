import { expect, test as base, type APIRequestContext, type APIResponse, type BrowserContext, type Locator, type Page, type Route } from '@playwright/test';

/**
 * Which server this run drives: the real-API e2e server
 * (`playwright.harness.config.ts`, the specs listed in `ported.ts` plus
 * `harness/`) or the dev mock (`playwright.config.ts`, the rest). Each config
 * sets `KANADE_E2E_TARGET` before any worker starts.
 */
export const HARNESS = process.env.KANADE_E2E_TARGET === 'harness';

/**
 * This worker's own server (see the port schemes in the two configs): admin on
 * base + 2 × parallel index, public one above. Workers set
 * TEST_PARALLEL_INDEX before any test file loads.
 */
function origins(): { admin: string; public: string } {
  const base = HARNESS ? (process.env.KANADE_E2E_HARNESS_BASE ?? '4393') : (process.env.KANADE_E2E_ORIGIN_BASE ?? '4373');
  const port = Number(base) + 2 * Number(process.env.TEST_PARALLEL_INDEX ?? '0');
  return { admin: `http://127.0.0.1:${port}`, public: `http://127.0.0.1:${port + 1}` };
}
export const ADMIN = origins().admin;
export const PUBLIC = origins().public;
/** Must match both configs; the fixture refuses a server pinned elsewhere. */
export const PINNED_NOW = '2026-09-29T04:00:00Z';
export const REAL_ART = process.env.KANADE_REAL_ART === '1';
/** First-load page headings under the pinned clock: admin's week hides its
 * done and cancelled runs (v4); the public portal opens signed out on Sign in. */
export const HEADING = { admin: '7 runs', public: 'Sign in to see the boss week' } as const;
/** The member a public sign-in becomes on either server, and the admin the mock always is: the seed's Asahi. */
const ASAHI_SNOWFLAKE = '100000000000001001';

/**
 * The harness's Discord sign-in for `origin`: its fake Discord page approves
 * the user the `kanade_fake_discord` cookie names (Asahi). `page.request`
 * shares the context's cookie jar; the redirects are followed one hop at a
 * time so each hop sends the cookies the last one set (see `sendSecureCookies`).
 */
async function discordSignIn(page: Page, origin: string, realm: 'admin' | 'public'): Promise<void> {
  await page.context().addCookies([{ name: 'kanade_fake_discord', value: ASAHI_SNOWFLAKE, domain: '127.0.0.1', path: '/' }]);
  let url = `${origin}/api/${realm}/auth/discord/start`;
  for (let hop = 0; hop < 5; hop++) {
    const location = (await page.request.get(url, { maxRedirects: 0 })).headers()['location'];
    if (!location) break;
    url = new URL(location, url).href;
  }
  expect(new URL(url).searchParams.get('login_error'), `the Discord sign-in outcome (${url})`).toBeNull();
  expect((await page.request.get(`${origin}/api/${realm}/session`)).status(), `the ${realm} session after sign-in`).toBe(200);
}

/**
 * Signs this page's browser context in on the public origin as Asahi: the
 * mock's `/__mock/public/sign-in` sets the cookie without Discord, the
 * harness runs the real Discord flow.
 */
export async function signInPublic(page: Page): Promise<void> {
  if (HARNESS) return discordSignIn(page, PUBLIC, 'public');
  const response = await page.request.post(`${PUBLIC}/__mock/public/sign-in`);
  expect(response.status()).toBe(204);
}

/**
 * The harness's admin is the template's break-glass token session; this
 * replaces it with Asahi's Discord session, as the mock's admin always is
 * (approving Kanade's proposals needs a Discord admin). A no-op on the mock.
 */
export async function signInAdminAsAsahi(page: Page): Promise<void> {
  if (HARNESS) await discordSignIn(page, ADMIN, 'admin');
}

/**
 * The real server's session cookies are `__Host-` (Secure). Chrome sends
 * Secure cookies to http://127.0.0.1, but Playwright's API requests send them
 * over http only to `localhost`, and the harness origins are 127.0.0.1. So on
 * the harness, every `context.request` (= `page.request`) call carries the
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

/** The CSP report sinks: the harness keeps one for both origins, the mock one per origin. */
const REPORT_SINKS = HARNESS ? [`${ADMIN}/__e2e/reports`] : [ADMIN, PUBLIC].map((o) => `${o}/__mock/reports`);

async function serverReports(): Promise<unknown[]> {
  const lists = await Promise.all(REPORT_SINKS.map(async (url) => (await (await fetch(url)).json()) as unknown[]));
  return lists.flat();
}

/**
 * Fails loudly unless both origins are this worker's server on the suite's
 * pinned clock. On the harness, answers the break-glass admin session cookie
 * every template copy holds.
 */
async function verifyServer(): Promise<{ name: string; value: string } | undefined> {
  let cookie: { name: string; value: string } | undefined;
  for (const [origin, kind] of [
    [ADMIN, 'admin'],
    [PUBLIC, 'public'],
  ] as const) {
    let who: unknown;
    try {
      who = await (await fetch(`${origin}/${HARNESS ? '__e2e' : '__mock'}/whoami`)).json();
    } catch {
      /* reported below */
    }
    const ours = HARNESS ? field(who, 'server') === 'kanade-e2e-server' : field(who, 'mock') === 'kanade-pwa-mock';
    if (!ours || field(who, 'origin') !== kind)
      throw new Error(
        `${origin} is not the e2e ${HARNESS ? 'server' : 'mock'} (${JSON.stringify(who)}); expected its ${kind} origin. ` +
          'Is something else on the e2e ports?',
      );
    // The harness clock moves only on request, and a reset puts it back: checked after the reset.
    if (!HARNESS && field(who, 'now') !== PINNED_NOW) throw new Error(`${origin} is a pwa-mock pinned at ${JSON.stringify(field(who, 'now'))}, not ${PINNED_NOW}`);
    const admin = field(who, 'admin_cookie');
    const name = field(admin, 'name');
    const value = field(admin, 'value');
    if (HARNESS && kind === 'admin') {
      if (typeof name !== 'string' || typeof value !== 'string') throw new Error(`${origin}/__e2e/whoami has no admin_cookie (${JSON.stringify(who)})`);
      cookie = { name, value };
    }
  }
  return cookie;
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

/** Puts the server back at the seed: the harness restarts over a fresh template copy (its clock back at the pinned instant). */
export async function resetWeek(): Promise<void> {
  if (!HARNESS) {
    await fetch(`${ADMIN}/api/admin/reset`, { method: 'POST' });
    return;
  }
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
  // On the harness a spec's own `request` is the context's, so it carries the admin session the
  // fixture adds (the real API refuses an anonymous admin call; the mock allowed one).
  request: async ({ request, context }, use) => use(HARNESS ? context.request : request),
  csp: [
    async ({ page, context, cspControl }, use) => {
      const admin = await verifyServer();
      await resetWeek();
      await Promise.all(REPORT_SINKS.map((url) => fetch(url, { method: 'DELETE' })));
      // The harness's break-glass admin session, as a browser that signed in holds it (the mock needs none).
      // `__Host-` cookies are Secure and host-only; cookies ignore ports, so both origins send it.
      if (admin) {
        await context.addCookies([{ ...admin, domain: '127.0.0.1', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
        sendSecureCookies(context);
      }
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
