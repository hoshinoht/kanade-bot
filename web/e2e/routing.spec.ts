import { ADMIN, expect, test, HEADING, choose } from './support';

const SECTIONS = {
  Schedule: ['Week', 'Fixed', 'Bosses'],
  Kanade: ['Inbox', 'Extractions', 'Chat', 'Rewrites', 'Limits'],
  Operate: ['Members', 'Reminders', 'Config', 'History'],
};

test('admin: grouped nav has every v4 section as a real route', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  const nav = page.getByRole('navigation', { name: 'Sections' });
  for (const [group, labels] of Object.entries(SECTIONS)) {
    const links = nav.getByRole('group', { name: group });
    await expect(links.getByRole('link')).toHaveText(labels.map((l) => new RegExp(`^\\s*${l}`)));
  }
  for (const label of ['Inbox', 'Extractions', 'Chat', 'Rewrites', 'Limits']) {
    await expect(nav.getByRole('link', { name: new RegExp(`^${label}`) })).toBeVisible();
  }
  await expect(nav.getByRole('link', { name: 'Week' })).toHaveAttribute('aria-current', 'page');

  await nav.getByRole('link', { name: 'History' }).click();
  await expect(page).toHaveURL(`${ADMIN}/history`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 changes');
  await expect(page).toHaveTitle(/^History — /);
  await expect(page.locator('#main')).toBeFocused();
  await expect(nav.getByRole('link', { name: 'History' })).toHaveAttribute('aria-current', 'page');

  for (const [label, heading] of [
    ['Fixed', '8 weekly timings'],
    ['Bosses', /^11 bosses/],
    ['Members', '13 bossers'],
    ['Reminders', /queued · \d+ sent$/],
    ['Inbox', '11 changes waiting'],
    ['Extractions', '34 model calls'],
    ['Chat', '16 interactions'],
    ['Rewrites', '8 rewrites'],
    ['Limits', 'gateway is at capacity'],
  ] as const) {
    await nav.getByRole('link', { name: label }).click();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(heading);
  }

  await page.goBack();
  await expect(page).toHaveURL(`${ADMIN}/rewrites`);
  // v4's /audit now lands on History's Sign-ins audit log.
  await page.goto(`${ADMIN}/audit?sw=off`);
  await expect(page).toHaveURL(`${ADMIN}/history?tab=sign-ins`);
  await nav.getByRole('link', { name: 'Week' }).click();
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
});

test('admin: deep links, detail routes and unknown paths', async ({ page }) => {
  await page.goto(`${ADMIN}/extractions/x-kalos?sw=off`);
  await expect(page.getByRole('link', { name: 'Extractions' }).first()).toHaveAttribute('aria-current', 'page');
  await page.goto(`${ADMIN}/extractions/42?sw=off`);
  await expect(page.getByRole('alert')).toContainText('No call “42”');
  await page.goto(`${ADMIN}/chat/7?sw=off`);
  await expect(page.getByRole('alert')).toContainText('No interaction “7”');
  await page.goto(`${ADMIN}/no/such/page?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText("That doesn't exist");
  await page.getByRole('link', { name: 'Back to the week' }).click();
  await expect(page).toHaveURL(`${ADMIN}/`);
});

// Phones (top bar + navigation drawer) and the rail's states: e2e/shell.spec.ts.

test('admin: this week / next week toggle, filters, the glance pane and the footer', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);
  // From 1200 px the at-a-glance pane sits beside the board (WeekRail1).
  const glance = page.getByRole('complementary', { name: 'At a glance' });
  // Pinned clock: Tue 29 Sep 12:00, so the next run is tonight's HCarling + HStar.
  await expect(glance).toContainText('10 h');
  await expect(glance.locator('.week-glance__time')).toHaveText('22:00');
  await expect(glance.getByRole('heading', { name: /^Party/ })).toBeVisible();
  await expect(glance.getByRole('link', { name: /Inbox/ })).toHaveAttribute('href', '/inbox');
  await expect(glance.getByRole('link', { name: /Model/ })).toContainText('busy');
  await glance.getByRole('button', { name: 'Open sheet' }).click();
  await expect(page.getByRole('complementary', { name: 'HCarling + HStar' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(glance).toBeVisible();

  // Narrower, the footer carries the same facts (O5).
  await page.setViewportSize({ width: 1100, height: 800 });
  await expect(glance).toHaveCount(0);
  const foot = page.locator('.week-window__foot');
  await expect(foot.getByRole('button', { name: /^Next/ })).toContainText('10 h');
  await expect(foot).toContainText('unanswered');
  await expect(foot.getByRole('link', { name: /Inbox/ })).toHaveAttribute('href', '/inbox');
  await expect(foot.getByRole('link', { name: /Model/ })).toContainText('busy');
  await page.setViewportSize({ width: 1280, height: 800 });

  await page.getByRole('link', { name: 'Next week' }).click();
  await expect(page).toHaveURL(`${ADMIN}/?week=next`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('3 runs');
  await expect(page.getByRole('link', { name: 'Next week' })).toHaveAttribute('aria-current', 'page');
  await page.goBack();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);

  await page.getByRole('button', { name: 'Filters (0)' }).click();
  const filters = page.getByRole('search', { name: 'Filter the week' });
  await choose(filters.getByLabel('Member'), { label: 'Sora' });
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('3 runs, filtered');
  await filters.getByLabel('Boss').fill('bm');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 run, filtered');
  await filters.getByRole('button', { name: 'Clear' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);

  // v4: done and cancelled runs wait behind "show them".
  await expect(page.locator('[data-run="r-seren"]')).toHaveCount(0);
  await page.getByRole('button', { name: 'Show them' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 runs');
  await expect(page.locator('[data-run="r-seren"]')).toBeVisible();
  await page.getByRole('button', { name: 'Hide the past' }).click();
  await expect(page.locator('[data-run="r-baldrix"]')).toHaveCount(0);
});

test('admin: login window wears the identity and signs in with Discord', async ({ page }) => {
  await page.goto(`${ADMIN}/login?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('YuukiSakuna');
  await expect(page.getByRole('link', { name: 'powered by kanade' })).toBeVisible();
  for (const img of await page.locator('.gate img').all()) {
    expect(await img.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth > 0)).toBe(true);
  }
  // The mock stands in for Discord: start → callback → a landing page that returns to `next`.
  await page.getByRole('link', { name: 'Sign in with Discord' }).click();
  await expect(page).toHaveURL(`${ADMIN}/`);
  await expect(page.locator('.brand__name')).toHaveText('YuukiSakuna');
});

test('identity: nothing cached falls back to generated art, never a 404', async ({ page }) => {
  const identity = await (await page.request.get(`${ADMIN}/api/identity`)).json();
  expect(identity).toEqual({ name: 'YuukiSakuna', avatar: '/identity/avatar', banner: '/identity/banner', cached: false, bot_user_id: '1543532497948909578' });
  for (const path of ['/identity/avatar', '/identity/banner']) {
    const response = await page.request.get(`${ADMIN}${path}`);
    expect(response.status()).toBe(200);
    expect(response.headers()['content-type']).toBe('image/svg+xml');
  }
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('link[rel="icon"]')).toHaveAttribute('href', '/identity/avatar');
});
