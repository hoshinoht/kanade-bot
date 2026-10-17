import { ADMIN, expect, resetWeek, test } from './support';

// The guides boards' state (`POST /__e2e/guides/board`, examples/e2e_server/README.md), which the
// Guides fidelity pairs draw on: six guides in every list status, and a reset back to the seed's two.
test('the guides board control draws the boards states, and a reset brings back the seed', async ({ page }) => {
  const board = await page.request.post(`${ADMIN}/__e2e/guides/board`);
  expect(board.status(), await board.text()).toBe(204);
  await page.goto(`${ADMIN}/guides?sw=off`);
  const rows = page.getByRole('listbox', { name: 'Guides' }).getByRole('option');
  await expect(rows).toHaveCount(6);
  for (const [title, chip] of [
    ['Carling + Star night', 'Needs attention'],
    ['Black Mage prep', 'Blocked'],
    ['Limbo party roles', 'Publishing'],
    ['Radiant Malefic Star', 'Out of date'],
    ['Kalos basics', 'Published'],
    ['Weekly bossing etiquette', 'Not published'],
  ])
    await expect(rows.filter({ hasText: title })).toContainText(chip);

  await resetWeek();
  await page.reload();
  await expect(rows).toHaveCount(2);
});
