import { expect, test as base, type APIRequestContext, type Locator, type Page } from '@playwright/test';

/**
 * This worker's own mock (see the port scheme in `playwright.config.ts`):
 * admin on base + 2 × parallel index, public one above. Workers set
 * TEST_PARALLEL_INDEX before any test file loads.
 */
function origins(): { admin: string; public: string } {
  const port = Number(process.env.KANADE_E2E_ORIGIN_BASE ?? '4373') + 2 * Number(process.env.TEST_PARALLEL_INDEX ?? '0');
  return { admin: `http://127.0.0.1:${port}`, public: `http://127.0.0.1:${port + 1}` };
}
export const ADMIN = origins().admin;
export const PUBLIC = origins().public;
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

interface Sink {
  violations: Violation[];
  console: string[];
  /** Set by any document of the page, including ones navigated away from. */
  seen: boolean;
}

/** Every page records `securitypolicyviolation` events (enforced and report-only). */
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

async function serverReports(): Promise<unknown[]> {
  return [...(await reports(ADMIN)), ...(await reports(PUBLIC))];
}

export const test = base.extend<{ cspControl: boolean; csp: Sink }>({
  /** Positive control: a test that deliberately violates the policy sets this. */
  cspControl: [false, { option: true }],
  csp: [
    async ({ page, cspControl }, use) => {
      await verifyMock();
      await resetWeek();
      await clearReports();
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
      expect(server, 'CSP reports received by the server (enforced + report-only)').toEqual([]);
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
