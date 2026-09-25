import { ADMIN, expect, test, HEADING } from './support';

const SECTIONS = {
  Schedule: ['Week', 'Fixed', 'Bosses'],
  Kanade: ['Inbox', 'Extractions', 'Chat', 'Limits'],
  Operate: ['Members', 'Reminders', 'Config', 'History'],
};

test('admin: grouped nav has every v4 section as a real route', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  const nav = page.getByRole('navigation', { name: 'Sections' });
  for (const [group, labels] of Object.entries(SECTIONS)) {
    const links = nav.getByRole('group', { name: group });
    await expect(links.getByRole('link')).toHaveText(labels.map((l) => new RegExp(`^${l}`)));
  }
  for (const label of ['Inbox', 'Extractions', 'Chat', 'Limits']) {
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
    ['Reminders', /queued, \d+ sent$/],
    ['Inbox', '9 changes waiting'],
    ['Extractions', '34 model calls'],
    ['Chat', '13 interactions'],
    ['Limits', 'extract is at capacity'],
  ] as const) {
    await nav.getByRole('link', { name: label }).click();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(heading);
  }

  await page.goBack();
  await expect(page).toHaveURL(`${ADMIN}/chat`);
  // v4's /audit now lands on History.
  await page.goto(`${ADMIN}/audit?sw=off`);
  await expect(page).toHaveURL(`${ADMIN}/history`);
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

test('admin: phone nav keeps Week and Inbox pinned and folds the rest into More', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  const phone = page.locator('.nav__phone');
  await expect(phone.getByRole('link', { name: 'Week' })).toBeVisible();
  await expect(phone.getByRole('link', { name: /^Inbox/ })).toBeVisible();
  await expect(phone.getByRole('link', { name: 'History' })).toBeHidden();
  await phone.getByText('More').click();
  await phone.getByRole('link', { name: 'History' }).click();
  await expect(page).toHaveURL(`${ADMIN}/history`);
  await expect(phone.locator('details')).not.toHaveAttribute('open', '');
  // The frame still never scrolls the document.
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight - innerHeight)).toBeLessThanOrEqual(0);
});

test('admin: this week / next week toggle, filters and tiles', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);
  const tiles = page.getByRole('group', { name: 'Right now' });
  // Pinned clock: Tue 29 Sep 12:00, so the next run is tonight's HCarling + HStar.
  await expect(tiles.getByRole('button', { name: /Next/ })).toContainText('in 10 h');
  await expect(tiles.getByRole('button', { name: /Next/ })).toContainText('HCarling + HStar · Tue 29 Sep 22:00');
  await expect(tiles.getByText('Unanswered')).toBeVisible();
  await expect(tiles.getByRole('link', { name: /Inbox/ })).toHaveAttribute('href', '/inbox');
  await expect(tiles.getByRole('link', { name: /Model/ })).toContainText('busy');

  await page.getByRole('link', { name: 'Next week' }).click();
  await expect(page).toHaveURL(`${ADMIN}/?week=next`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('3 runs');
  await expect(page.getByRole('link', { name: 'Next week' })).toHaveAttribute('aria-current', 'page');
  await page.goBack();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(HEADING.admin);

  const filters = page.getByRole('search', { name: 'Filter the week' });
  await filters.getByLabel('Member').selectOption({ label: 'Sora' });
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
  expect(identity).toEqual({ name: 'YuukiSakuna', avatar: '/identity/avatar', banner: '/identity/banner', cached: false });
  for (const path of ['/identity/avatar', '/identity/banner']) {
    const response = await page.request.get(`${ADMIN}${path}`);
    expect(response.status()).toBe(200);
    expect(response.headers()['content-type']).toBe('image/svg+xml');
  }
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('link[rel="icon"]')).toHaveAttribute('href', '/identity/avatar');
});
