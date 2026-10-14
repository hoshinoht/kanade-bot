import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Links to Discord open the app (`discord://`, same tab) unless this browser
// turns that off on Account › This browser; then they are the https links in a new tab.

const DEEP = /^discord:\/\/-\/channels\//;
const WEB = /^https:\/\/discord\.com\/channels\//;

async function openFirstItem(page: Page) {
  const options = page.getByRole('listbox', { name: 'Extractor items' }).getByRole('option');
  await options.first().click();
  const links = page.locator('.inbox__detail .msg a.msg__at');
  await expect(links.first()).toBeVisible();
  return links;
}

const sw = (page: Page) => page.getByRole('switch', { name: 'Open Discord links in the app' });

test('by default Inbox links open the Discord app in the same tab', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/inbox?sw=off`);
  const links = await openFirstItem(page);
  for (const link of await links.all()) {
    await expect(link).toHaveAttribute('href', DEEP);
    await expect(link).not.toHaveAttribute('target');
  }
  for (const card of await page.locator('.inbox__detail .proposal__card').all()) await expect(card).toHaveAttribute('href', DEEP);
});

test('turning the switch off on Account gives https links in a new tab, live and after a reload', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/account?tab=browser&sw=off`);
  await expect(page.getByRole('tab', { name: 'This browser' })).toHaveAttribute('aria-selected', 'true');
  await expect(sw(page)).toHaveAttribute('aria-checked', 'true');
  await expect(sw(page)).toHaveAccessibleDescription(/this browser.*Discord app/);
  await sw(page).click();
  await expect(sw(page)).toHaveAttribute('aria-checked', 'false');
  expect(await page.evaluate(() => localStorage.getItem('discord-links'))).toBe('web');

  // No reload: the shell's own navigation.
  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: /^Inbox/ }).click();
  const links = await openFirstItem(page);
  for (const link of await links.all()) {
    await expect(link).toHaveAttribute('href', WEB);
    await expect(link).toHaveAttribute('target', '_blank');
    await expect(link).toHaveAttribute('rel', /noopener/);
  }

  await page.reload();
  await expect((await openFirstItem(page)).first()).toHaveAttribute('href', WEB);
  await page.goto(`${ADMIN}/account?tab=browser&sw=off`);
  await expect(sw(page)).toHaveAttribute('aria-checked', 'false');

  // Keyboard turns it back on; absent key means on.
  await sw(page).focus();
  await page.keyboard.press('Space');
  await expect(sw(page)).toHaveAttribute('aria-checked', 'true');
  expect(await page.evaluate(() => localStorage.getItem('discord-links'))).toBeNull();
});

test('phone: the switch fits the Account frame', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/account?tab=browser&sw=off`);
  const s = sw(page);
  await s.scrollIntoViewIfNeeded();
  await expect(s).toBeInViewport({ ratio: 1 });
  const box = (await s.boundingBox())!;
  expect(box.width).toBeGreaterThanOrEqual(44);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  const card = page.locator('.account-row').filter({ has: s });
  expect(await card.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= window.innerHeight)).toBe(true);
});
