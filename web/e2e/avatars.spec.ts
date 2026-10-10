import type { Locator, Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Member and admin portraits (`/api/admin/members/{id}/avatar`,
// `/api/admin/me/avatar`): the mock serves generated stand-in art for most
// members and the server's monogram for 1005, 1009 and 1014. Each portrait
// is a same-origin image under the strict CSP; the letter stays only when an
// image cannot load.

async function go(page: Page, path: string) {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

/** The portrait's image has loaded (decoded, non-zero size). */
async function loaded(avatar: Locator, src: RegExp) {
  const img = avatar.locator('img.avatar__img');
  await expect(img).toHaveAttribute('src', src);
  await expect.poll(() => img.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth > 0)).toBe(true);
  await expect(avatar).toHaveAttribute('aria-hidden', 'true');
}

test('members: rows and the sheet show each member’s portrait', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/members');
  const asahi = page.locator('.memberlist__row[data-member="1001"]');
  await loaded(asahi.locator('.memberlist__av'), /\/api\/admin\/members\/1001\/avatar$/);
  // A member without an avatar still gets an image: the server's monogram.
  await loaded(page.locator('.memberlist__row[data-member="1005"] .memberlist__av'), /\/members\/1005\/avatar$/);
  await asahi.click();
  const pane = page.getByRole('complementary', { name: 'Member details' });
  await loaded(pane.locator('.membersheet__avatar'), /\/members\/1001\/avatar$/);
});

test('members: on a phone the sheet shows the portrait beside the Discord account', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/members');
  await page.locator('.memberlist__row[data-member="1003"]').click();
  const sheet = page.getByRole('dialog');
  await loaded(sheet.locator('.membersheet__account .membersheet__avatar'), /\/members\/1003\/avatar$/);
});

test('inbox: thread messages show their authors’ portraits', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/inbox');
  const avatars = page.locator('.proposal__thread .msg__av');
  await expect(avatars.first()).toBeVisible();
  const withImage = page.locator('.proposal__thread .msg__av img.avatar__img');
  await expect(withImage.first()).toHaveAttribute('src', /\/api\/admin\/members\/\d+\/avatar$/);
  await loaded(withImage.first().locator('..'), /\/avatar$/);
});

test('account: the page and the account chip show the signed-in admin’s portrait', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/account');
  await loaded(page.locator('.account-id .account-id__portrait'), /\/api\/admin\/me\/avatar$/);
  await loaded(page.getByRole('button', { name: /^Account: Asahi/ }).locator('.account__initial'), /\/me\/avatar$/);
});

test('account: a portrait that cannot load falls back to the initial', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.route('**/api/admin/me/avatar', (route) => route.abort());
  await go(page, '/account');
  const head = page.locator('.account-id .account-id__portrait');
  await expect(head.locator('img')).toHaveCount(0);
  await expect(head).toHaveText('A');
});

test('history: a cleared Limits window is a view-only Limits row with the window as it was', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await go(page, '/limits');
  await page.getByRole('tab', { name: /Allowances/ }).click();
  const reset = page.getByRole('table', { name: 'Chatbot allowances' }).getByRole('button', { name: /^Reset .*'s window$/ }).first();
  const name = (await reset.getAttribute('aria-label'))!.replace(/^Reset (.*)'s window$/, '$1');
  await reset.click();
  await expect(page.getByRole('group', { name: 'Notification' }).filter({ hasText: `${name}'s window is reset.` })).toBeVisible();

  await go(page, '/history');
  const row = page.locator('.history-row--config');
  await expect(row).toHaveCount(1);
  await expect(row.locator('.chip', { hasText: 'Limits' })).toBeVisible();
  await expect(row).toContainText(`Limits · cleared ${name}'s chat window`);
  await row.click();
  const pane = page.getByRole('complementary', { name: 'Change details' });
  await expect(pane.getByRole('heading', { name: `${name}'s chat window cleared` })).toBeVisible();
  const field = pane.locator('.history-diff__field');
  await expect(field).toHaveCount(1);
  await expect(field.locator('.history-diff__now')).toHaveText(/^now\s*0$/);
  await expect(pane.getByRole('button', { name: 'Revert…' })).toHaveCount(0);
  await pane.getByRole('link', { name: 'Open Limits' }).click();
  await expect(page).toHaveURL(/\/limits/);
});
