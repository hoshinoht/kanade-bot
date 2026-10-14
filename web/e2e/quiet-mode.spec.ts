import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, csrf, expect, settle, test } from './support';

// "Quiet mode on" (B_States, B_CfgNotify): while Config → Notifications quiet
// mode is on, the chip replaces the Live chip in the page line, and in the
// phone's top bar where Live sits. It follows `GET /api/admin/summary`.

async function setQuiet(page: Page, on: boolean) {
  const reply = await page.request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(page.request), data: { notifications: { quiet_mode: on } } });
  expect(reply.status()).toBe(200);
}

/** The palette's "Refresh now": the next summary read, without reloading the page. */
async function refreshNow(page: Page) {
  const polled = page.waitForResponse((r) => r.url().includes('/api/admin/summary'));
  await page.keyboard.press('ControlOrMeta+k');
  await page.getByRole('dialog', { name: 'Command palette' }).getByRole('combobox').fill('Refresh now');
  await page.keyboard.press('Enter');
  await polled;
}

async function noSerious(page: Page, label: string) {
  await settle(page);
  const result = await new AxeBuilder({ page }).include('.pageline, .topbar').analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

/**
 * Summary reads answer only after 6 s (well inside the client's 10 s timeout),
 * so a chip that changes within the 2.5 s assertion came from the save itself.
 */
async function slowSummary(page: Page) {
  await page.route(/\/api\/admin\/summary(\?|$)/, async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 6_000));
    await route.continue().catch(() => {});
  });
}

const AT_ONCE = { timeout: 2_500 };

for (const size of [
  { width: 1280, height: 800 },
  { width: 1600, height: 900 },
]) {
  test(`page line: a Config save swaps the Live chip for quiet mode at once, and back, ${size.width}×${size.height}`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${ADMIN}/?sw=off`);
    const status = page.locator('.pageline').getByRole('group', { name: 'Status' });
    await expect(status.locator('.fresh--live')).toBeVisible();
    await expect(status.locator('.quiet')).toHaveCount(0);

    // Turned on in Config: the chip follows the save's answer, not the next summary read.
    await page.getByRole('link', { name: 'Config' }).click();
    await page.getByRole('tab', { name: 'Notifications' }).click();
    await slowSummary(page);
    const quietSwitch = page.getByRole('switch', { name: /^Quiet mode/ });
    await quietSwitch.click();
    const quiet = status.locator('.quiet');
    await expect(quiet).toHaveText('Quiet mode on', AT_ONCE);
    await expect(quiet.locator('[data-icon="bell-off"]')).toBeVisible();
    await expect(status.locator('.fresh')).toHaveCount(0);
    await noSerious(page, 'quiet page line');
    expect((await page.locator('.pageline').boundingBox())!.height).toBeLessThanOrEqual(36.5);

    // Turned off again: Live is back at once.
    await quietSwitch.click();
    await page.getByRole('dialog', { name: 'Turn quiet mode off?' }).getByRole('button', { name: 'Turn quiet mode off' }).click();
    await expect(status.locator('.fresh--live')).toBeVisible(AT_ONCE);
    await expect(status.locator('.quiet')).toHaveCount(0);
    await page.unrouteAll({ behavior: 'ignoreErrors' });

    // On from elsewhere (another admin): the next summary read shows it, on every page.
    await setQuiet(page, true);
    await page.getByRole('link', { name: 'Members' }).click();
    await refreshNow(page);
    await expect(page.locator('.pageline').getByRole('group', { name: 'Status' }).locator('.quiet')).toHaveText('Quiet mode on');
  });
}

test('phone top bar: quiet mode replaces the Live chip, and Live returns when it is off', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await setQuiet(page, true);
  await page.goto(`${ADMIN}/?sw=off`);
  const bar = page.locator('.topbar__fresh');
  await expect(bar.locator('.quiet')).toHaveText('Quiet mode on');
  await expect(bar.locator('[data-icon="bell-off"]')).toBeVisible();
  await expect(bar.locator('.fresh')).toHaveCount(0);
  // The top bar's controls all stay on screen beside the longer chip.
  for (const control of [page.getByRole('button', { name: 'Open the navigation' }), page.locator('.topbar__inbox'), bar]) {
    const box = (await control.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(390);
  }
  await noSerious(page, 'quiet top bar');

  await setQuiet(page, false);
  await refreshNow(page);
  await expect(bar.locator('.fresh--live')).toBeVisible();
  await expect(bar.locator('.quiet')).toHaveCount(0);
});
