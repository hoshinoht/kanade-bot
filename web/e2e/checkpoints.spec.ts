import { ADMIN, expect, test } from './support';

// History → Checkpoints with backups (B_HistoryCk, O3): the mock lists three
// manifests, one for each anchor state the server reports, newest first.

test('history checkpoints: the verified chain and the backups that anchor it', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  let reads = 0;
  page.on('request', (request) => {
    if (/\/api\/admin\/history\/checkpoints$/.test(request.url())) reads += 1;
  });
  await page.goto(`${ADMIN}/history?sw=off`);
  await page.getByRole('tab', { name: 'Checkpoints' }).click();

  const card = page.locator('.history-verify');
  await expect(card).toContainText('Chain verified');
  await expect(card).toContainText(/\d+ records · head #\d+ · [0-9a-f]{12}/);
  await expect(card).toContainText('checked just now');
  await expect(card).not.toHaveClass(/history-verify--risk/);

  const table = page.getByRole('table', { name: 'Backups anchoring the history, newest first' });
  const rows = table.locator('tbody tr');
  await expect(rows).toHaveCount(3);
  await expect(table.getByRole('columnheader')).toHaveText(['Backup', 'Taken', 'History head', 'Revision', 'Anchored']);
  for (const row of await rows.all()) await expect(row.getByRole('rowheader')).toHaveText(/^kanade-\d{8}-\d{4}-pre-[0-9a-f]{7}\.sqlite$/);
  const anchors = await rows.locator('td:last-child .history-anchor').evaluateAll((chips) => chips.map((c) => c.className.match(/history-anchor--(\w+)/)![1]));
  expect(anchors.sort()).toEqual(['matches', 'mismatch', 'older_schema']);
  await expect(table.locator('.history-anchor--matches')).toContainText('matches');
  await expect(table.locator('.history-anchor--mismatch')).toContainText('mismatch');
  await expect(table.locator('.history-anchor--older_schema')).toContainText('older schema');
  // Each caveat is explained once under the table.
  const note = page.locator('.history-backups__note');
  await expect(note).toContainText('Older schema: restore it with the image of that schema.');
  await expect(note).toContainText('Mismatch: the history no longer holds this head.');

  // Verify again re-reads the endpoint (re-checking chain and manifests) and announces it.
  const before = reads;
  await card.getByRole('button', { name: 'Verify again' }).click();
  await expect.poll(() => reads).toBe(before + 1);
  await expect(page.getByRole('status').filter({ hasText: /^Chain verified: \d+ records, head #\d+\.$/ })).toBeAttached();
  await expect(rows).toHaveCount(3);
});

test('history checkpoints: the backups table fits a phone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/history?sw=off`);
  await page.getByRole('tab', { name: 'Checkpoints' }).click();
  await expect(page.locator('.history-backups tbody tr')).toHaveCount(3);
  await expect(page.getByRole('button', { name: 'Verify again' })).toBeVisible();
  // The document never scrolls; only the panel does.
  expect(await page.evaluate(() => document.documentElement.scrollHeight <= innerHeight + 1)).toBe(true);
});
