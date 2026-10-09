import { PUBLIC, expect, test } from './support';

// The Rust server fills the link-preview tags (`src/api/assets.rs`); the mock
// serves the built shell as is, so only the static head is checked here.
test('public: the favicon and home-screen icon are the bot avatar', async ({ page }) => {
  await page.goto(`${PUBLIC}/?sw=off`);
  await expect(page.locator('link[rel="icon"]')).toHaveAttribute('href', '/identity/avatar');
  await expect(page.locator('link[rel="apple-touch-icon"]')).toHaveAttribute('href', '/identity/avatar');
  await expect(page.locator('meta[property="og:type"]')).toHaveAttribute('content', 'website');
  const avatar = await page.request.get(`${PUBLIC}/identity/avatar`);
  expect(avatar.status()).toBe(200);
});
