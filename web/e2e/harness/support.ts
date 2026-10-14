import { setTimeout as sleep } from 'node:timers/promises';
import { expect, test as base, type Page } from '@playwright/test';

/**
 * This worker's own e2e server (see the port scheme in
 * `playwright.harness.config.ts`): admin on base + 2 × parallel index, public
 * one above. Workers set TEST_PARALLEL_INDEX before any test file loads.
 */
function origins(): { admin: string; public: string } {
  const port = Number(process.env.KANADE_E2E_HARNESS_BASE ?? '4393') + 2 * Number(process.env.TEST_PARALLEL_INDEX ?? '0');
  return { admin: `http://127.0.0.1:${port}`, public: `http://127.0.0.1:${port + 1}` };
}
export const ADMIN = origins().admin;
export const PUBLIC = origins().public;
/** Must match `playwright.harness.config.ts`; the fixture refuses a server on another clock. */
export const PINNED_NOW = '2026-09-29T04:00:00Z';
const REPORTS = `${ADMIN}/__e2e/reports`;

interface Violation {
  directive: string;
  blocked: string;
  disposition: string;
  sample: string;
}

interface Sink {
  violations: Violation[];
  console: string[];
  /** Set by any document of the page, including ones navigated away from. */
  seen: boolean;
}

/** What the page script keeps on `window`. */
interface CspWindow {
  __csp?: Violation[];
  __kanadeCspSeen?: () => void;
}

/** A JSON object's field, or undefined when the body is not an object. */
function field(body: unknown, key: string): unknown {
  return body !== null && typeof body === 'object' ? Reflect.get(body, key) : undefined;
}

/** Fails loudly unless both origins are this worker's e2e server. */
async function verifyServer(): Promise<void> {
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
  }
}

/** Both origins share one report sink. */
async function serverReports(): Promise<unknown[]> {
  const reports: unknown = await (await fetch(REPORTS)).json();
  return Array.isArray(reports) ? reports : [reports];
}

/** Every page records `securitypolicyviolation` events (CSP and Trusted Types, all enforced). */
async function watch(page: Page, sink: Sink): Promise<void> {
  await page.exposeFunction('__kanadeCspSeen', () => {
    sink.seen = true;
  });
  await page.addInitScript(() => {
    // The page's own global, extended by this script only.
    const w = window as unknown as CspWindow;
    w.__csp = [];
    document.addEventListener('securitypolicyviolation', (e) => {
      w.__csp?.push({ directive: e.violatedDirective, blocked: e.blockedURI, disposition: e.disposition, sample: e.sample });
      w.__kanadeCspSeen?.();
    });
  });
  page.on('console', (msg) => {
    const text = msg.text();
    if (/content security policy|trusted type|refused to/i.test(text)) sink.console.push(text);
  });
}

async function collect(page: Page): Promise<Violation[]> {
  if (page.isClosed()) return [];
  try {
    return await page.evaluate(() => {
      // Set up by `watch`'s init script.
      const w = window as unknown as CspWindow;
      return w.__csp ?? [];
    });
  } catch {
    return [];
  }
}

export const test = base.extend<{ csp: Sink }>({
  csp: [
    async ({ page }, use) => {
      await verifyServer();
      // A restart over a fresh copy of the seeded store, the clock back at the suite's instant.
      const reset = await fetch(`${ADMIN}/__e2e/reset`, { method: 'POST' });
      const body: unknown = reset.status === 200 ? await reset.json() : await reset.text();
      expect(reset.status, JSON.stringify(body)).toBe(200);
      expect(field(body, 'now'), 'the server clock after a reset').toBe(PINNED_NOW);
      await fetch(REPORTS, { method: 'DELETE' });
      const sink: Sink = { violations: [], console: [], seen: false };
      await watch(page, sink);
      await use(sink);
      sink.violations.push(...(await collect(page)));
      // report-uri POSTs land asynchronously: only a page that saw a violation waits for them.
      if (sink.seen || sink.violations.length > 0 || sink.console.length > 0)
        for (let waited = 0; waited < 3_000 && (await serverReports()).length === 0; waited += 100) await sleep(100);
      expect.soft(sink.console, 'CSP / Trusted Types console messages').toEqual([]);
      expect.soft(sink.violations, 'securitypolicyviolation events').toEqual([]);
      expect(await serverReports(), 'CSP / Trusted Types reports received by the server').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
