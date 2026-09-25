import { expect, test as base, type APIRequestContext, type Page } from '@playwright/test';

export const ADMIN = process.env.KANADE_E2E_ADMIN ?? 'http://127.0.0.1:4373';
export const PUBLIC = process.env.KANADE_E2E_PUBLIC ?? 'http://127.0.0.1:4374';
/** Must match `playwright.config.ts`; the fixture refuses a mock pinned elsewhere. */
export const PINNED_NOW = '2026-09-29T04:00:00Z';
export const REAL_ART = process.env.KANADE_REAL_ART === '1';
/** Week headings under the pinned mock clock: admin hides its done and cancelled runs (v4). */
export const HEADING = { admin: '7 runs', public: '9 runs' } as const;

export interface Violation {
  directive: string;
  blocked: string;
  disposition: string;
  sample: string;
}

async function reports(origin: string): Promise<unknown[]> {
  const response = await fetch(`${origin}/__mock/reports`);
  return (await response.json()) as unknown[];
}

async function clearReports(): Promise<void> {
  await Promise.all([ADMIN, PUBLIC].map((o) => fetch(`${o}/__mock/reports`, { method: 'DELETE' })));
}

/** Fails loudly unless both origins are a pwa-mock with the suite's pinned clock. */
async function verifyMock(): Promise<void> {
  for (const [origin, kind] of [
    [ADMIN, 'admin'],
    [PUBLIC, 'public'],
  ] as const) {
    let who: { mock?: string; now?: string | null; origin?: string } = {};
    try {
      who = (await (await fetch(`${origin}/__mock/whoami`)).json()) as typeof who;
    } catch {
      /* reported below */
    }
    if (who.mock !== 'kanade-pwa-mock' || who.now !== PINNED_NOW || who.origin !== kind)
      throw new Error(
        `${origin} is not the e2e mock (${JSON.stringify(who)}); expected a ${kind} pwa-mock pinned at ${PINNED_NOW}. ` +
          'Is a dev server on the e2e ports?',
      );
  }
}

/** A direct admin write needs the session's CSRF token, as the PWA sends it (API-5). */
export async function csrf(request: APIRequestContext): Promise<Record<string, string>> {
  const session = await request.get(`${ADMIN}/api/admin/session`);
  return { 'X-Kanade-CSRF': session.headers()['x-kanade-csrf'] ?? '' };
}

export async function resetWeek(): Promise<void> {
  await fetch(`${ADMIN}/api/admin/reset`, { method: 'POST' });
}

/** Every page records `securitypolicyviolation` events (enforced and report-only). */
async function watch(page: Page, sink: { violations: Violation[]; console: string[] }) {
  await page.addInitScript(() => {
    const w = window as unknown as { __csp: unknown[] };
    w.__csp = [];
    document.addEventListener('securitypolicyviolation', (e) => {
      w.__csp.push({ directive: e.violatedDirective, blocked: e.blockedURI, disposition: e.disposition, sample: e.sample });
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

export const test = base.extend<{ cspControl: boolean; csp: { violations: Violation[]; console: string[] } }>({
  /** Positive control: a test that deliberately violates the policy sets this. */
  cspControl: [false, { option: true }],
  csp: [
    async ({ page, cspControl }, use) => {
      await verifyMock();
      await resetWeek();
      await clearReports();
      const sink = { violations: [] as Violation[], console: [] as string[] };
      await watch(page, sink);
      await use(sink);
      sink.violations.push(...(await collect(page)));
      // report-uri POSTs are sent asynchronously; give them a moment to land.
      await page.waitForTimeout(400);
      const server = [...(await reports(ADMIN)), ...(await reports(PUBLIC))];
      if (cspControl) {
        // The server must have received both kinds of report the control provoked.
        const text = JSON.stringify(server);
        expect(text).toContain('style-src-attr');
        expect(text).toContain('require-trusted-types-for');
        return;
      }
      expect.soft(sink.console, 'CSP / Trusted Types console messages').toEqual([]);
      expect.soft(sink.violations, 'securitypolicyviolation events').toEqual([]);
      expect(server, 'CSP reports received by the server (enforced + report-only)').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
