import type { BrowserContext, Page } from '@playwright/test';
import { ADMIN, PUBLIC, expect, test, HEADING } from './support';

async function controlled(page: Page, origin: string) {
  await page.goto(`${origin}/`);
  await page.evaluate(() => navigator.serviceWorker.ready);
  // The first load is not controlled (no clients.claim); the reload is.
  await page.reload();
  await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller !== null)).toBe(true);
}

async function precachedUrls(page: Page): Promise<string[]> {
  return page.evaluate(async () => {
    const urls: string[] = [];
    for (const name of await caches.keys()) {
      for (const request of await (await caches.open(name)).keys()) urls.push(new URL(request.url).pathname);
    }
    return urls;
  });
}

async function offline(context: BrowserContext, page: Page) {
  await context.setOffline(true);
  await page.reload();
}

test('public: SW precaches static assets only and serves the offline page', async ({ page, context }) => {
  await controlled(page, PUBLIC);
  const cached = await precachedUrls(page);
  expect(cached.length).toBeGreaterThan(5);
  expect(cached.filter((u) => u.startsWith('/api/') || u.startsWith('/csp-report'))).toEqual([]);
  await offline(context, page);
  await expect(page.getByRole('heading', { name: "You're offline" })).toBeVisible();
  await expect(page.getByRole('img', { name: /Kanade is taking a nap/ })).toBeVisible();
  await context.setOffline(false);
  await page.getByRole('link', { name: 'Try again' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.public);
});

test('admin: SW serves the shell offline and the app shows its own offline state', async ({ page, context }) => {
  await controlled(page, ADMIN);
  await page.evaluate(() => fetch('/api/admin/week'));
  expect((await precachedUrls(page)).filter((u) => u.startsWith('/api/'))).toEqual([]);
  await offline(context, page);
  await expect(page.getByRole('heading', { name: "You're offline" })).toBeVisible();
  await expect(page.getByText('Nothing private is stored on this device.')).toBeVisible();
  // Reconnecting starts an automatic poll. Hold its response so the retry
  // control cannot disappear between the online event and the click.
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  await page.route('**/api/admin/week?*', async (route) => {
    await gate;
    await route.continue();
  });
  await context.setOffline(false);
  try {
    await page.getByRole('button', { name: 'Try again' }).click();
  } finally {
    release();
  }
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);
});

test.describe('service workers blocked', () => {
  test.use({ serviceWorkers: 'block' });

  for (const [name, origin] of [
    ['public', PUBLIC],
    ['admin', ADMIN],
  ] as const) {
    test(`${name}: works without a service worker`, async ({ page }) => {
      await page.goto(`${origin}/`);
      await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING[name]);
      expect(await page.evaluate(() => navigator.serviceWorker?.controller ?? null)).toBeNull();
      await page.reload();
      // The admin week shows it is live; the public portal has no live data until member reads.
      if (name === 'admin') await expect(page.locator('[data-fresh="live"]')).toBeVisible();
      else await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.public);
    });
  }
});

for (const [name, origin, id, appName] of [
  ['public', PUBLIC, '/?app=kanade-public', 'Kanade — boss schedule'],
  ['admin', ADMIN, '/?app=kanade-admin', 'Kanade Admin'],
] as const) {
  test(`${name}: installable by Chrome's criteria with its own manifest id`, async ({ page }) => {
    await controlled(page, origin);
    const cdp = await page.context().newCDPSession(page);
    const { installabilityErrors } = (await cdp.send('Page.getInstallabilityErrors')) as {
      installabilityErrors: { errorId: string }[];
    };
    // Playwright contexts are incognito, which Chrome never offers to install from.
    expect(installabilityErrors.filter((e) => e.errorId !== 'in-incognito')).toEqual([]);
    const manifest = (await cdp.send('Page.getAppManifest')) as { url: string; errors: unknown[]; data?: string };
    expect(manifest.errors).toEqual([]);
    const data = JSON.parse(manifest.data ?? '{}') as { id: string; name: string; scope: string };
    expect(data).toMatchObject({ id, name: appName, scope: '/' });
  });
}
