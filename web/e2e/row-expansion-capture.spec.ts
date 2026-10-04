import type { Page } from '@playwright/test';
import { ADMIN, REAL_ART, expect, settle, test } from './support';

const OUT = `e2e/.captures/${REAL_ART ? 'real' : 'synthetic'}/row-expansion`;

async function shot(page: Page, name: string) {
  await page.evaluate(async () => {
    await document.fonts.ready;
    const images = [...document.images].filter((image) => image.getBoundingClientRect().top < innerHeight && image.checkVisibility());
    await Promise.race([Promise.all(images.map((image) => image.decode().catch(() => {}))), new Promise((resolve) => setTimeout(resolve, 3000))]);
  });
  await settle(page);
  await page.screenshot({ path: `${OUT}/${name}.png`, animations: 'disabled' });
}

for (const size of [{ width: 1280, height: 800 }, { width: 390, height: 844 }]) {
  test(`capture row expansion ${size.width}×${size.height}`, async ({ page }) => {
    test.setTimeout(120_000);
    await page.setViewportSize(size);
    await page.addInitScript(() => {
      localStorage.setItem('colorway', 'blossom');
      localStorage.setItem('theme', 'light');
    });
    const tag = `${size.width}x${size.height}`;

    await page.goto(`${ADMIN}/fixed?sw=off`);
    const fixed = page.getByRole('button', { name: 'Edit Tuesday 22:00 — HCarling + HStar', exact: true });
    await expect(fixed).toBeVisible();
    await shot(page, `fixed-collapsed-${tag}`);
    await fixed.click();
    await expect(fixed).toHaveAttribute('aria-current', 'true');
    await shot(page, `fixed-selected-${tag}`);

    await page.goto(`${ADMIN}/?sw=off`);
    const run = page.locator('[data-run="r-carling"] .plan-card__open');
    await expect(run).toBeVisible();
    await shot(page, `week-collapsed-${tag}`);
    await run.click();
    await expect(run).toHaveAttribute('aria-current', 'true');
    await shot(page, `week-selected-${tag}`);

    await page.goto(`${ADMIN}/inbox?tab=self_service&sw=off`);
    const inbox = page.locator('[data-item="p-carling-link"]');
    await expect(inbox).toBeVisible();
    await shot(page, `inbox-collapsed-${tag}`);
    await inbox.click();
    await expect(inbox).toHaveAttribute('aria-selected', 'true');
    await shot(page, `inbox-selected-${tag}`);

    await page.goto(`${ADMIN}/history?sw=off`);
    const history = page.locator('[data-history="2"]').first();
    await expect(history).toBeVisible();
    await shot(page, `history-collapsed-${tag}`);
    await history.click();
    await expect(history).toHaveAttribute('aria-current', 'true');
    await shot(page, `history-selected-${tag}`);

    await page.goto(`${ADMIN}/members?sw=off`);
    const member = page.locator('[data-member="1003"]');
    await expect(member).toBeVisible();
    await shot(page, `members-collapsed-${tag}`);
    await member.click();
    await expect(member).toHaveAttribute('aria-current', 'true');
    await shot(page, `members-selected-${tag}`);

    await page.goto(`${ADMIN}/bosses?sw=off`);
    await expect(page.getByRole('link', { name: 'Radiant Malefic Star' })).toBeVisible();
    await shot(page, `bosses-collapsed-${tag}`);
    await page.getByRole('link', { name: 'Radiant Malefic Star' }).click();
    await expect(page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' })).toBeVisible();
    await shot(page, `bosses-selected-${tag}`);

    await page.goto(`${ADMIN}/config?section=pings&sw=off`);
    const config = page.getByRole('tab', { name: /^Models/ });
    await config.scrollIntoViewIfNeeded();
    await shot(page, `config-collapsed-${tag}`);
    await config.click();
    await expect(config).toHaveAttribute('aria-selected', 'true');
    await shot(page, `config-selected-${tag}`);
  });
}
