import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// The M3E shell (docs/v5/m3e-rail-design-spec.md, gates G1, G2, G6, G7): the
// navigation rail at ≥ 600 px, the 36 px page line, and on phones a 48 px top
// bar with a navigation drawer instead of any nav row.

const scrolls = (page: Page) => page.evaluate(() => document.scrollingElement!.scrollHeight - innerHeight);

test('rail: grouped destinations beside the page, no masthead, no 1180 px cap', async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.addInitScript(() => localStorage.removeItem('rail'));
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  await expect(page.locator('.masthead')).toHaveCount(0);
  const rail = page.locator('.navrail');
  const nav = rail.getByRole('navigation', { name: 'Sections' });
  await expect(nav.getByRole('link', { name: 'Week' })).toHaveAttribute('aria-current', 'page');
  await expect(nav.getByRole('link', { name: /^Inbox/ })).toContainText('9');
  // The badge is part of the link's name, said as words.
  await expect(nav.getByRole('link', { name: 'Inbox 9 waiting', exact: true })).toBeVisible();
  // The page uses the width (G6): main runs from the rail to the edge.
  const main = (await page.locator('#main').boundingBox())!;
  const railBox = (await rail.boundingBox())!;
  expect(main.x).toBeGreaterThanOrEqual(railBox.x + railBox.width - 1);
  expect(main.x + main.width).toBeGreaterThan(1590);
  // The page line: one 36 px row with the title, the count and the status.
  const line = page.locator('.pageline');
  await expect(line.locator('.pageline__title')).toHaveText('Week');
  await expect(line.getByRole('group', { name: 'Status' })).toContainText('Live');
  await expect(line.getByRole('button', { name: 'Commands' })).toBeVisible();
  expect((await line.boundingBox())!.height).toBeLessThanOrEqual(36.5);
});

test('rail: expanded from 1440 px, collapsible, and the choice is remembered', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('seeded')) {
      localStorage.removeItem('rail');
      sessionStorage.setItem('seeded', '1');
    }
  });
  await page.goto(`${ADMIN}/members?sw=off`);
  const rail = page.locator('.navrail');
  await expect(rail).toHaveCSS('width', '240px');
  await expect(rail.locator('.brand__name')).toBeVisible();
  await expect(rail.getByText('Schedule', { exact: true })).toBeVisible();
  await rail.getByRole('button', { name: 'Collapse the navigation' }).click();
  await expect(rail).toHaveCSS('width', '96px');
  await page.reload();
  await expect(page.locator('.navrail')).toHaveCSS('width', '96px');
  await page.locator('.navrail').getByRole('button', { name: 'Expand the navigation' }).click();
  await expect(page.locator('.navrail')).toHaveCSS('width', '240px');

  // Below 1440 px the rail is always collapsed and offers no toggle.
  await page.setViewportSize({ width: 1280, height: 800 });
  await expect(page.locator('.navrail')).toHaveCSS('width', '96px');
  await expect(page.locator('.navrail').getByRole('button', { name: /the navigation$/ })).toBeHidden();
});

for (const size of [
  { width: 1280, height: 800 },
  { width: 1000, height: 670 },
  { width: 1280, height: 600 },
]) {
  test(`rail: every destination fits without scrolling at ${size.width}×${size.height}`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${ADMIN}/inbox?sw=off`);
    const rail = page.locator('.navrail');
    await expect(rail.getByRole('link', { name: /^Inbox/ })).toHaveAttribute('aria-current', 'page');
    expect(await rail.evaluate((el) => el.scrollHeight - el.clientHeight)).toBeLessThanOrEqual(0);
    for (const link of await rail.getByRole('navigation').getByRole('link').all()) {
      const box = (await link.boundingBox())!;
      expect(box.y + box.height).toBeLessThanOrEqual(size.height);
      // The whole item is the link: at least 44 px tall (spec "Accessibility").
      expect(box.height).toBeGreaterThanOrEqual(44);
    }
    const account = (await rail.getByRole('button', { name: /^Account/ }).boundingBox())!;
    expect(account.y + account.height).toBeLessThanOrEqual(size.height);
  });
}

test('phone: 48 px top bar, Inbox one tap away, and no rail or nav row', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  const bar = page.locator('.topbar');
  expect((await bar.boundingBox())!.height).toBeLessThanOrEqual(50);
  await expect(bar.locator('.topbar__title')).toHaveText('Week');
  await expect(bar.locator('[data-fresh="live"]')).toBeVisible();
  await expect(page.locator('.navrail')).toHaveCount(0);
  await expect(page.getByRole('navigation', { name: 'Sections' })).toHaveCount(0);
  const inbox = bar.getByRole('link', { name: 'Inbox 9 waiting', exact: true });
  await expect(inbox).toContainText('9');
  await inbox.click();
  await expect(page).toHaveURL(`${ADMIN}/inbox`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 changes waiting');
  await expect(inbox).toHaveAttribute('aria-current', 'page');
  // The phone's chrome above the window: top bar, page line and margins
  // (≈ 100 px; the masthead, pinned nav and page-head card took ≈ 185).
  const window = (await page.locator('.inbox').boundingBox())!;
  expect(window.y).toBeLessThanOrEqual(110);
  expect(await scrolls(page)).toBeLessThanOrEqual(0);
});

test('phone: the navigation drawer traps focus, closes every way, and gives focus back', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/members?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 bossers');
  const menu = page.getByRole('button', { name: 'Open the navigation' });
  await expect(menu).toHaveAttribute('aria-expanded', 'false');
  await menu.click();
  const drawer = page.getByRole('dialog', { name: 'Navigation' });
  await expect(drawer).toBeVisible();
  await expect(menu).toHaveAttribute('aria-expanded', 'true');
  const nav = drawer.getByRole('navigation', { name: 'Sections' });
  for (const group of ['Schedule', 'Kanade', 'Operate']) await expect(nav.getByRole('group', { name: group })).toBeVisible();
  await expect(nav.getByRole('link', { name: 'Members' })).toBeFocused();

  // Focus is trapped: Tab and Shift+Tab never leave the drawer.
  const inside = () => page.evaluate(() => !!document.activeElement?.closest('dialog.drawer'));
  for (let i = 0; i < 20; i++) {
    await page.keyboard.press('Tab');
    expect(await inside()).toBe(true);
  }
  for (let i = 0; i < 20; i++) {
    await page.keyboard.press('Shift+Tab');
    expect(await inside()).toBe(true);
  }

  // Escape closes and focus returns to the menu button.
  await page.keyboard.press('Escape');
  await expect(drawer).toBeHidden();
  await expect(menu).toBeFocused();

  // The × closes too, with focus back on the menu.
  await menu.click();
  await drawer.getByRole('button', { name: 'Close the navigation' }).click();
  await expect(drawer).toBeHidden();
  await expect(menu).toBeFocused();

  // A tap on the scrim closes it.
  await menu.click();
  await expect(drawer).toBeVisible();
  await page.mouse.click(380, 400);
  await expect(drawer).toBeHidden();

  // Following a link navigates, closes, and lands on the new page.
  await menu.click();
  await drawer.getByRole('link', { name: 'History' }).click();
  await expect(page).toHaveURL(`${ADMIN}/history`);
  await expect(drawer).toBeHidden();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 changes');
  await expect(page.locator('#main')).toBeFocused();
  await expect(page.locator('.topbar__title')).toHaveText('History');
  expect(await scrolls(page)).toBeLessThanOrEqual(0);
});

test('phone: the drawer carries the account and the time zone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  await page.getByRole('button', { name: 'Open the navigation' }).click();
  const drawer = page.getByRole('dialog', { name: 'Navigation' });
  await expect(drawer.getByText('Asia/Kuala_Lumpur')).toBeVisible();
  await drawer.getByRole('button', { name: /Account: Asahi/ }).click();
  await expect(drawer.getByRole('menu', { name: 'Account' }).getByRole('menuitem', { name: 'Sign out' })).toBeVisible();
});

test('phone landscape uses the phone frame (the rail would not fit 390 px of height)', async ({ page }) => {
  await page.setViewportSize({ width: 844, height: 390 });
  await page.goto(`${ADMIN}/inbox?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 changes waiting');
  await expect(page.locator('.topbar')).toBeVisible();
  await expect(page.locator('.navrail')).toHaveCount(0);
  await page.getByRole('button', { name: 'Open the navigation' }).click();
  await page.getByRole('dialog', { name: 'Navigation' }).getByRole('link', { name: 'Config' }).click();
  await expect(page).toHaveURL(`${ADMIN}/config`);
  expect(await scrolls(page)).toBeLessThanOrEqual(0);
});

for (const size of [
  { width: 1280, height: 800 },
  { width: 1000, height: 670 },
]) {
  test(`page line: the time zone is in the status chip's tooltip at ${size.width}×${size.height}`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${ADMIN}/members?sw=off`);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 bossers');
    const status = page.locator('.pageline').getByRole('group', { name: 'Status' });
    await expect(status).toBeVisible();
    await expect(status).toHaveAttribute('title', 'Every time here is Asia/Kuala_Lumpur');
  });
}

test('phone: a drawer link to the page already shown closes it and gives focus back to the menu', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/members?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 bossers');
  const menu = page.getByRole('button', { name: 'Open the navigation' });
  await menu.click();
  const drawer = page.getByRole('dialog', { name: 'Navigation' });
  await drawer.getByRole('link', { name: 'Members' }).click();
  await expect(drawer).toBeHidden();
  await expect(page).toHaveURL(`${ADMIN}/members`);
  await expect(menu).toBeFocused();
});

test('phone: a swipe in from the left edge opens the drawer; a vertical stroke does not', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/members?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 bossers');
  const strip = page.locator('.edgeswipe');
  await expect(strip).toHaveCSS('touch-action', 'pan-y');
  const drawer = page.getByRole('dialog', { name: 'Navigation' });
  // One touch stroke on the strip (it unmounts once the drawer opens, so
  // the three events go to the element found at the start).
  const stroke = (to: { x: number; y: number }) =>
    strip.evaluate((el, to) => {
      const base = { pointerType: 'touch', pointerId: 7, isPrimary: true, bubbles: true };
      el.dispatchEvent(new PointerEvent('pointerdown', { ...base, clientX: 6, clientY: 400 }));
      el.dispatchEvent(new PointerEvent('pointermove', { ...base, clientX: to.x, clientY: to.y }));
      el.dispatchEvent(new PointerEvent('pointerup', { ...base, clientX: to.x, clientY: to.y }));
    }, to);
  await stroke({ x: 10, y: 520 });
  await expect(drawer).toBeHidden();
  await stroke({ x: 90, y: 410 });
  await expect(drawer).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Open the navigation' })).toBeFocused();
});

test('page line: on the ground, with only the title group in a 12 px outlined surface shape', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('colorway', 'blossom');
    localStorage.setItem('theme', 'light');
  });
  for (const size of [
    { width: 1280, height: 800 },
    { width: 1000, height: 670 },
  ]) {
    await page.setViewportSize(size);
    for (const path of ['/', '/bosses', '/history', '/fixed', '/chat/c-move']) {
      await page.goto(`${ADMIN}${path}?sw=off`);
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
      const line = page.locator('.pageline');
      const head = line.locator('.pageline__head');
      const look = await line.evaluate((el) => {
        const shape = el.querySelector('.pageline__head')!;
        const probe = document.createElement('i');
        probe.style.setProperty('color', 'var(--surface)');
        document.body.append(probe);
        const surface = getComputedStyle(probe).color;
        probe.remove();
        const line = getComputedStyle(el);
        const cs = getComputedStyle(shape);
        return {
          lineBg: line.backgroundColor,
          lineBorder: line.borderTopWidth,
          bg: cs.backgroundColor,
          surface,
          border: `${cs.borderTopWidth} ${cs.borderTopStyle}`,
          radius: cs.borderTopLeftRadius,
        };
      });
      // The line itself is bare; the title group is the one contained shape.
      expect(look.lineBg, path).toBe('rgba(0, 0, 0, 0)');
      expect(look.lineBorder, path).toBe('0px');
      expect(look.bg, path).toBe(look.surface);
      expect(look.border, path).toBe('2px solid');
      expect(look.radius, path).toBe('12px');
      await expect(head.getByRole('heading', { level: 1 })).toBeVisible();
      // The status, Commands and the page's own controls stay outside it.
      await expect(head.getByRole('group', { name: 'Status' })).toHaveCount(0);
      await expect(head.locator('.btn, .mchip, .seg, select')).toHaveCount(0);
      await expect(line.getByRole('group', { name: 'Status' })).toBeVisible();
      expect((await head.boundingBox())!.height, `${path} at ${size.width}`).toBeLessThanOrEqual(36.5);
      expect((await line.boundingBox())!.height, `${path} at ${size.width}`).toBeLessThanOrEqual(36.5);
    }
  }
});

// Live data has more models than the mock: two more in the summary.
async function fourModels(page: Page) {
  await page.route(/\/api\/admin\/chat(\?.*)?$/, async (route) => {
    const response = await route.fetch();
    const json = (await response.json()) as { summary: Record<string, unknown>[] };
    json.summary.push(
      { model: 'glm-5.3-flash:cloud', count: 40, answered: 37, refused: 1, errors: 2, p50_ms: 2200, tool_calls: 31 },
      { model: 'gpt-oss:120b-cloud', count: 2, answered: 2, refused: 0, errors: 0, p50_ms: 10_300, tool_calls: 1 },
    );
    await route.fulfill({ response, json });
  });
}

for (const size of [
  { width: 1280, height: 800 },
  { width: 1000, height: 670 },
]) {
  test(`chat page line: four models stay one row at ${size.width}×${size.height}; the rest open as a table`, async ({ page }) => {
    await page.setViewportSize(size);
    await fourModels(page);
    await page.goto(`${ADMIN}/chat?sw=off`);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 interactions');
    const line = page.locator('.pageline');
    const chips = line.getByRole('list', { name: 'Busiest models, for these rows' }).getByRole('listitem');
    await expect(chips).toHaveCount(2);
    // Busiest first (by interactions): the stub's 40-interaction model leads.
    await expect(chips.first()).toContainText('glm-5.3-flash:cloud');
    await expect(chips.first()).toContainText(/37\s✓\s*(answered)? · p50 2\.2 s/);
    expect((await line.boundingBox())!.height).toBeLessThanOrEqual(36.5);
    // Chips that do not fit are clipped, never wrapped onto a second row.
    const top = (await line.boundingBox())!.y;
    for (const chip of await chips.all()) {
      const box = await chip.boundingBox();
      if (box && (await chip.isVisible())) expect(box.y).toBeLessThan(top + 36);
    }

    const more = line.getByRole('button', { name: '+2 models · 4 errors' });
    await expect(more).toHaveAttribute('aria-expanded', 'false');
    await more.click();
    await expect(more).toHaveAttribute('aria-expanded', 'true');
    const table = page.getByRole('table', { name: 'Per model, for these rows' });
    await expect(table.getByRole('rowheader')).toHaveText(['glm-5.3-flash:cloud', 'kanata/chat', 'gpt-oss:120b-cloud', 'kanata/chat-cloud']);
    await expect(table.getByRole('row', { name: /glm-5\.3-flash:cloud/ })).toContainText('2,200 ms');
    await page.keyboard.press('Escape');
    await expect(table).toBeHidden();
    await expect(more).toBeFocused();
    // Keyboard opens it too; a click elsewhere closes it.
    await page.keyboard.press('Enter');
    await expect(table).toBeVisible();
    await page.getByRole('heading', { level: 1 }).click();
    await expect(table).toBeHidden();
  });
}

test('chat page line on a phone: the strip under the top bar, the table within the screen', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await fourModels(page);
  await page.goto(`${ADMIN}/chat?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 interactions');
  const line = page.locator('.pageline');
  await expect(line.locator('.pageline__head')).toHaveCSS('border-top-width', '2px');
  await line.getByRole('button', { name: /models · 4 errors$/ }).click();
  const panel = page.locator('.modelstats__panel');
  await expect(panel).toBeVisible();
  const box = (await panel.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight - innerHeight)).toBeLessThanOrEqual(0);
});
