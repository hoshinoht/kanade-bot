import { ADMIN, expect, test } from './support';
import type { Locator, Page } from '@playwright/test';

// Shared pane states (M3E B_Empty, B_States): an empty Inbox tab says why and
// shows the tab's last decisions; a pane whose read failed offers a retry.

test('an empty Extractor tab: the last read, the ways on, and the last three decisions', async ({ page }) => {
  await page.route('**/api/admin/inbox', (route) => route.fulfill({ json: [] }));
  await page.goto(`${ADMIN}/inbox`);
  await expect(page.getByRole('heading', { name: 'Nothing waiting' })).toBeVisible();
  await expect(page.getByText(/watched channel\. The last read was \w{3} \d+ \d\d:\d\d\sin #\S+\./)).toBeVisible();
  await expect(page.getByRole('link', { name: 'See recent extractions' })).toHaveAttribute('href', '/extractions');
  await expect(page.getByRole('link', { name: 'Re-read channels' })).toHaveAttribute('href', '/config?section=rescan');
  const recent = page.getByRole('region', { name: 'Recently decided' }).getByRole('link');
  await expect(recent.first()).toBeVisible();
  expect(await recent.count()).toBeLessThanOrEqual(3);
  await recent.first().click();
  await expect(page).toHaveURL(/tab=past&item=/);
  await expect(page.getByRole('listbox', { name: 'Past items' })).toBeVisible();
});

test('a failed read: the reason, Try again reloads the pane in place', async ({ page }) => {
  let fail = true;
  await page.route('**/api/admin/members', (route) =>
    fail ? route.fulfill({ status: 503, json: { error: 'unavailable', message: 'The server took too long to answer.' } }) : route.continue(),
  );
  await page.goto(`${ADMIN}/members`);
  const alert = page.getByRole('alert').filter({ hasText: 'Couldn’t load members' });
  await expect(alert).toContainText('The server took too long to answer.');
  fail = false;
  await alert.getByRole('button', { name: 'Try again' }).click();
  await expect(alert).toBeHidden();
  await expect(page.getByRole('list', { name: 'Members' }).getByRole('listitem').first()).toBeVisible();
});

// Every list or settings pane fails the same way (B_States "Error and retry"):
// the failed pane says what did not load and why, and Try again reloads it in place.
const FAILED: { name: string; path: string; api: RegExp; thing: string; loaded: (page: Page) => Locator }[] = [
  { name: 'chat', path: '/chat', api: /\/api\/admin\/chat(\?|$)/, thing: 'interactions', loaded: (page) => page.locator('.chat__list [role="option"]').first() },
  { name: 'extractions', path: '/extractions', api: /\/api\/admin\/extractions(\?|$)/, thing: 'the calls', loaded: (page) => page.locator('.extract-list [role="option"]').first() },
  { name: 'history', path: '/history', api: /\/api\/admin\/history\?/, thing: 'the history', loaded: (page) => page.locator('.history-row').first() },
  { name: 'bosses', path: '/bosses', api: /\/api\/admin\/bosses$/, thing: 'the boss list', loaded: (page) => page.getByRole('navigation', { name: 'Boss catalog' }).getByRole('link').first() },
  { name: 'config', path: '/config', api: /\/api\/admin\/config$/, thing: 'the settings', loaded: (page) => page.locator('.settings__panel:not([hidden]) .settings__card').first() },
];

for (const { name, path, api, thing, loaded } of FAILED) {
  test(`${name}: a failed read fills its pane with the reason and Try again`, async ({ page }) => {
    let fail = true;
    await page.route(api, (route) =>
      fail && route.request().method() === 'GET'
        ? route.fulfill({ status: 503, json: { error: 'unavailable', message: 'The server took too long to answer.' } })
        : route.continue(),
    );
    await page.goto(`${ADMIN}${path}`);
    const alert = page.getByRole('alert').filter({ hasText: `Couldn’t load ${thing}` });
    await expect(alert).toContainText('The server took too long to answer.');
    await expect(alert.getByRole('button', { name: 'Copy details' })).toBeVisible();
    fail = false;
    await alert.getByRole('button', { name: 'Try again' }).click();
    await expect(alert).toBeHidden();
    await expect(loaded(page)).toBeVisible();
  });
}

// A slow first read shows words in the waiting pane, never a blank window.
const SLOW: { name: string; path: string; api: RegExp; words: string; loaded: (page: Page) => Locator }[] = [
  { name: 'members', path: '/members', api: /\/api\/admin\/members$/, words: 'Loading the roster…', loaded: (page) => page.getByRole('list', { name: 'Members' }).getByRole('listitem').first() },
  { name: 'reminders', path: '/reminders', api: /\/api\/admin\/reminders(\?|$)/, words: 'Loading reminders…', loaded: (page) => page.locator('.reminders-table').first() },
  { name: 'history', path: '/history', api: /\/api\/admin\/history\?/, words: 'Loading the history…', loaded: (page) => page.locator('.history-row').first() },
];

for (const { name, path, api, words, loaded } of SLOW) {
  test(`${name}: a slow first read says it is loading in the pane`, async ({ page }) => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    await page.route(api, async (route) => {
      await held;
      await route.continue();
    });
    await page.goto(`${ADMIN}${path}`);
    await expect(page.getByRole('status').filter({ hasText: words })).toBeVisible();
    release();
    await expect(loaded(page)).toBeVisible();
    await expect(page.getByRole('status').filter({ hasText: words })).toHaveCount(0);
  });
}

// The page line's Live chip (B_States "Loading, offline and stale"): a failed
// poll keeps the week and says Retrying; a week that never loaded says so in the window.
test('week: a failed refresh turns the Live chip into Retrying and keeps the board', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/?sw=off`);
  const chip = page.locator('.fresh');
  await expect(chip).toHaveAttribute('data-fresh', 'live');
  await page.route(/\/api\/admin\/week(\?|$)/, (route) => route.fulfill({ status: 503, json: { error: 'unavailable', message: 'Busy.' } }));
  await page.getByRole('button', { name: 'Refresh' }).click();
  await expect(chip).toHaveAttribute('data-fresh', 'stale');
  await expect(chip).toContainText('Retrying');
  await expect(page.locator('[data-run]').first()).toBeVisible();
});

test("week: a first read that fails says Kanade can't be reached, with Try again", async ({ page }) => {
  let fail = true;
  await page.route(/\/api\/admin\/week(\?|$)/, (route) =>
    fail ? route.fulfill({ status: 503, json: { error: 'unavailable', message: 'Busy.' } }) : route.continue(),
  );
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.getByRole('heading', { name: "Kanade can't be reached" })).toBeVisible();
  await expect(page.locator('.fresh')).toHaveAttribute('data-fresh', 'error');
  fail = false;
  await page.getByRole('button', { name: 'Try again' }).click();
  await expect(page.locator('[data-run]').first()).toBeVisible();
  await expect(page.locator('.fresh')).toHaveAttribute('data-fresh', 'live');
});
