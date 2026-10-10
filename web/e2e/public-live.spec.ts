import type { Page } from '@playwright/test';
import { PUBLIC, expect, signInPublic, test } from './support';

// Member live updates (public-portal-plan § Live updates): the portal holds
// one `GET /api/public/events` stream while signed in and shown. A hint
// re-reads what it names (`schedule` and `mine` the weeks, `mine` the
// requests and timings already read, `allowance` the allowance); the week
// polls every 60 s while the stream is open (`weeks.test.ts` pins the
// cadence: the mock's held responses keep the browser's stream connecting,
// not open) and every 30 s once it drops.
// `POST /__mock/public/hint` stands in for the server's member-scoped hints.
declare global {
  interface Window {
    /** Every EventSource the page opened (`countStreams`). */
    __streams: EventSource[];
  }
}

/** Counts the page's EventSources and keeps them for their `readyState`. */
async function countStreams(page: Page) {
  await page.addInitScript(() => {
    const Native = window.EventSource;
    window.__streams = [];
    window.EventSource = class extends Native {
      constructor(url: string | URL, init?: EventSourceInit) {
        super(url, init);
        window.__streams.push(this);
      }
    };
  });
}

const states = (page: Page) => page.evaluate(() => window.__streams.map((s) => s.readyState));

async function hint(page: Page, topic: 'schedule' | 'mine' | 'allowance') {
  const reply = await page.request.post(`${PUBLIC}/__mock/public/hint`, { data: { topic } });
  expect(reply.status()).toBe(204);
}

/** Every GET the page makes to `path`, as it goes out. */
function reads(page: Page, path: string): string[] {
  const seen: string[] = [];
  page.on('request', (r) => {
    if (r.method() === 'GET' && new URL(r.url()).pathname === path) seen.push(r.url());
  });
  return seen;
}

test('each hint re-reads only what it names', async ({ page }) => {
  await signInPublic(page);
  // Account reads the allowance when it opens.
  const opened = page.waitForResponse((r) => r.url().endsWith('/api/public/me/allowance'));
  await page.goto(`${PUBLIC}/account?sw=off`);
  await opened;
  const allowance = reads(page, '/api/public/me/allowance');
  const week = reads(page, '/api/public/week');
  const requests = reads(page, '/api/public/requests/mine');
  await expect.poll(() => page.evaluate(() => performance.getEntriesByType('resource').some((e) => e.name.includes('/api/public/events')))).toBe(true);

  const allowanceRead = page.waitForResponse((r) => r.url().endsWith('/api/public/me/allowance'));
  await hint(page, 'allowance');
  await allowanceRead;
  expect(week).toEqual([]);

  const weekRead = page.waitForResponse((r) => r.url().endsWith('/api/public/week?week=next'));
  const requestsRead = page.waitForResponse((r) => r.url().endsWith('/api/public/requests/mine'));
  await hint(page, 'mine');
  await Promise.all([weekRead, requestsRead]);
  expect(allowance).toHaveLength(1);
  expect(requests).toHaveLength(1);
});

test('when the stream drops, the week goes back to its 30 s poll', async ({ page }) => {
  await countStreams(page);
  await page.clock.install();
  await signInPublic(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.locator('[data-run="r-limbo"]')).toHaveCount(1);
  // The stream is up: a hint re-reads the week at once.
  await expect.poll(async () => (await states(page)).some((state) => state !== 2)).toBe(true);
  const week = reads(page, '/api/public/week');
  await hint(page, 'schedule');
  // Hints for one reader within 250 ms wake it once (on the page's clock).
  await expect
    .poll(async () => {
      await page.clock.runFor(300);
      return week.length;
    })
    .toBeGreaterThan(1);
  week.length = 0;

  // The stream drops: the server refuses it from now on.
  await page.route(`${PUBLIC}/api/public/events`, (route) => route.fulfill({ status: 503, contentType: 'application/json', body: '{"error":"unavailable","message":"Try again later."}' }));
  // A hint ends the held response, and the reconnect is refused.
  await hint(page, 'allowance');
  await expect.poll(async () => (await states(page)).every((state) => state === 2)).toBe(true);
  // Within the 30 s cadence (the stream's would be 60 s).
  await page.clock.runFor(31_000);
  await expect.poll(() => week.length).toBeGreaterThan(0);
});

test('a hidden tab closes the stream; shown again it reconnects and re-reads', async ({ page }) => {
  await countStreams(page);
  await signInPublic(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.locator('[data-run="r-limbo"]')).toHaveCount(1);
  const live = async () => (await states(page)).some((state) => state !== 2);
  await expect.poll(live).toBe(true);
  const before = (await states(page)).length;

  const setHidden = (hidden: boolean) =>
    page.evaluate((h) => {
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (h ? 'hidden' : 'visible') });
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => h });
      document.dispatchEvent(new Event('visibilitychange'));
    }, hidden);
  await setHidden(true);
  await expect.poll(live).toBe(false);
  const week = reads(page, '/api/public/week');
  // A hint while hidden reaches nobody.
  await hint(page, 'schedule');
  await page.waitForTimeout(600); // nothing may happen: real time must pass
  expect(week).toEqual([]);

  await setHidden(false);
  await expect.poll(live).toBe(true);
  expect((await states(page)).length).toBeGreaterThan(before);
  await expect.poll(() => week.length).toBeGreaterThan(0);
});
