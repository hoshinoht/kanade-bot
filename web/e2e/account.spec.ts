import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, expect, settle, test } from './support';

// Account (A02, `GET /api/admin/me`): reached from the account menu. The
// mock's Discord session is Asahi (staff, on the roster); the token and
// Tailscale sessions are neutral. The fixture resets the session per test.

async function axe(page: Page, label: string) {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

test('a Discord session opens its account from the menu: access, named roles and the Limits allowance', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.getByRole('button', { name: /^Account: Asahi/ }).click();
  await page.getByRole('menu', { name: 'Account' }).getByRole('menuitem', { name: 'Your account' }).click();
  await expect(page).toHaveURL(`${ADMIN}/account`);
  await expect(page).toHaveTitle(/^Account — /);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Asahi');
  await expect(page.locator('.pageline__context')).toHaveText('signed in with Discord');
  const facts = page.locator('.account__facts');
  await expect(facts).toContainText('Staff — exempt from chatbot budgets');
  await expect(facts).toContainText('Yes — on the roster');
  await expect(facts.getByRole('list', { name: 'Server roles' }).getByRole('listitem')).toHaveText(['@staff', '@bossers']);
  await expect(facts).toContainText('Exempt (staff)');
  // Names only: no raw role ids on the page.
  await expect(facts).not.toContainText('300001');
  await axe(page, 'account discord');
});

for (const [method, how] of [
  ['token', 'the admin token'],
  ['tailscale', 'Tailscale'],
] as const) {
  test(`a ${method} session is neutral: not a Discord member`, async ({ page }) => {
    await page.request.post(`${ADMIN}/__mock/session`, { data: { method } });
    await page.goto(`${ADMIN}/account?sw=off`);
    await expect(page.getByRole('heading', { name: 'Not a Discord member' })).toBeVisible();
    await expect(page.locator('.pageline__context')).toHaveText(`signed in with ${how}`);
    await expect(page.locator('.account__facts')).toHaveCount(0);
    await axe(page, `account ${method}`);
  });
}

test('phone: the account page fits the frame', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/account?sw=off`);
  await expect(page.locator('.account__facts')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Asahi');
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= window.innerHeight)).toBe(true);
});
