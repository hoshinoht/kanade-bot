import { ADMIN, choose, expect, test } from './support';

// The Rewrites log: persona rewrites of reminder headers and nudges in the
// Extractions list-detail layout, filtered server-side by stage and verdict,
// each attempt with its rule or error code and usage against the reservation.

test('rewrites: list, filter by stage and verdict, open an attempt', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/rewrites?sw=off`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('8 rewrites');
  await expect(page).toHaveTitle(/^Rewrites — /);
  const nav = page.getByRole('navigation', { name: 'Sections' });
  await expect(nav.getByRole('link', { name: 'Rewrites' })).toHaveAttribute('aria-current', 'page');
  const list = page.getByRole('listbox', { name: /Rewrite attempts/ });
  await expect(list.getByRole('option')).toHaveCount(8);
  // Wide: the newest attempt is open from the start.
  await expect(page.getByRole('article').getByRole('heading', { level: 2 })).toBeVisible();

  await page.getByRole('button', { name: 'Filters (0)' }).click();
  const panel = page.getByRole('group', { name: 'Filters' });
  await choose(panel.getByLabel('Stage'), 'batch');
  await expect(page).toHaveURL(/[?&]stage=batch/);
  await panel.getByRole('checkbox', { name: 'unavailable' }).check();
  await expect(page).toHaveURL(/[?&]verdict=unavailable/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('2 of 8 rewrites');
  await expect(list.getByRole('option')).toHaveCount(2);
  await expect(page.getByRole('button', { name: /^Stage: daily batch/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /^Verdict: unavailable/ })).toBeVisible();
  await page.keyboard.press('Escape');

  await list.locator('[data-attempt="rw-over"]').click();
  await expect(page).toHaveURL(/[?&]attempt=rw-over/);
  const detail = page.getByRole('article');
  await expect(detail.getByText('Rewrite · #rwover')).toBeVisible();
  const verdict = detail.getByRole('complementary', { name: 'Verdict' });
  await expect(verdict).toContainText('unavailable (budget_exceeded)');
  await expect(verdict).toContainText('used 412 > reserved 287');
  await expect(verdict).toContainText('countdown:r-kalos:60');
  await expect(detail.getByText('Waku waku!', { exact: true })).toBeVisible();
  await detail.getByText('Reasoning · 98 tokens').click();
  await expect(detail.getByText(/Let me think about which one fits/)).toBeVisible();

  // The other token check: a reservation refused before sending.
  await list.locator('[data-attempt="rw-reserve"]').click();
  await expect(verdict).toContainText('reserved 16,391 > budget 16,384');
  await list.locator('[data-attempt="rw-over"]').click();

  // A deep link keeps the filters and the attempt.
  await page.reload();
  await expect(list.getByRole('option')).toHaveCount(2);
  await expect(verdict).toContainText('unavailable (budget_exceeded)');

  // Clear brings every attempt back.
  await page.getByRole('button', { name: 'Clear' }).click();
  await expect(list.getByRole('option')).toHaveCount(8);
});

test('rewrites: a refused filter shows its error, a missing attempt says so', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  expect((await page.request.get(`${ADMIN}/api/admin/rewrites?verdict=bogus`)).status()).toBe(422);
  await page.goto(`${ADMIN}/rewrites?verdict=bogus&sw=off`);
  const refused = page.getByRole('alert').filter({ hasText: 'Couldn’t load the rewrites' });
  await expect(refused).toContainText('Unknown verdict “bogus”.');
  await expect(page.getByRole('listbox', { name: /Rewrite attempts/ })).toHaveCount(0);

  await page.goto(`${ADMIN}/rewrites?attempt=rw-missing&sw=off`);
  await expect(page.getByRole('alert')).toContainText('No rewrite “rw-missing”');
});
