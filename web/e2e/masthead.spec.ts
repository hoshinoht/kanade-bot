import AxeBuilder from '@axe-core/playwright';
import { ADMIN, expect, test } from './support';

// The shell's chrome (was the masthead; now the navigation rail and, on a
// phone, the top bar): captures (git-ignored) go to
// e2e/.captures/masthead/<tag>-<viewport>.png; KANADE_CAPTURE_TAG=before
// records the old chrome for comparison. The account chip and its menu sit
// at the rail's foot.
const TAG = process.env.KANADE_CAPTURE_TAG ?? 'after';

for (const vp of [
  { name: 'wide', width: 1280, height: 800, chrome: '.navrail' },
  { name: 'narrow', width: 390, height: 844, chrome: '.topbar' },
]) {
  test(`capture masthead ${vp.name}`, async ({ page }) => {
    await page.setViewportSize({ width: vp.width, height: vp.height });
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    await page.evaluate(() => document.fonts.ready);
    const chrome = page.locator(vp.chrome);
    await chrome.screenshot({ path: `e2e/.captures/masthead/${TAG}-${vp.name}.png`, animations: 'disabled' });
    await page.screenshot({ path: `e2e/.captures/masthead/${TAG}-${vp.name}-page.png`, animations: 'disabled' });
  });
}

test('account menu: keyboard, copy the user id, sign out, and axe', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  await page.goto(`${ADMIN}/?sw=off`);
  const chip = page.getByRole('button', { name: /Asahi/ });
  await expect(chip).toHaveAttribute('aria-haspopup', 'menu');
  await chip.focus();
  await page.keyboard.press('Enter');
  const menu = page.getByRole('menu', { name: 'Account' });
  await expect(menu).toContainText('signed in with Discord');
  const account = menu.getByRole('menuitem', { name: 'Your account' });
  const copy = menu.getByRole('menuitem', { name: 'Copy user ID' });
  const signOut = menu.getByRole('menuitem', { name: 'Sign out' });
  await expect(account).toBeFocused();
  await expect(chip).toHaveAttribute('aria-expanded', 'true');
  await page.keyboard.press('ArrowDown');
  await expect(copy).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(signOut).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(account).toBeFocused();
  await page.keyboard.press('End');
  await expect(signOut).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();
  await expect(chip).toBeFocused();

  // Opening from ↑ lands on the last item; the open menu passes axe.
  await page.keyboard.press('ArrowUp');
  await expect(signOut).toBeFocused();
  const scan = await new AxeBuilder({ page }).include('.navrail').withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa']).analyze();
  expect(scan.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => v.id)).toEqual([]);
  await page.keyboard.press('Home');
  await expect(account).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('1001');
  await expect(page.getByRole('group', { name: 'Notification' }).filter({ hasText: 'Copied your user ID.' })).toBeVisible();

  // A click elsewhere closes it; Sign out from the menu ends the session.
  await chip.click();
  await expect(menu).toBeVisible();
  await page.locator('main').click({ position: { x: 5, y: 5 } });
  await expect(menu).toBeHidden();
  await chip.click();
  await signOut.click();
  await expect(page).toHaveURL(`${ADMIN}/login`);
});

test('account menu: a token session has no user id to copy', async ({ page }) => {
  await page.request.post(`${ADMIN}/__mock/session`, { data: { method: 'token' } });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.getByRole('button', { name: /Break-glass token/ }).click();
  const menu = page.getByRole('menu', { name: 'Account' });
  await expect(menu).toContainText('signed in with the admin token');
  await expect(menu.getByRole('menuitem')).toHaveText([/Your account/, /Sign out/]);
});
