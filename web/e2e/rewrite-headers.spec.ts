import { ADMIN, expect, test } from './support';

// Config → Notifications: "Rewrite this week's headers" asks first, starts the
// manual run (the server answers at once) and shows a second trigger's refusal
// while that run is still going.

test("rewrite this week's headers: confirms, starts, refuses while running", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/config?section=notifications&sw=off`);
  const card = page.getByRole('region', { name: "This week's posted headers" });
  await expect(card).toContainText('nobody is pinged again');
  const trigger = card.getByRole('button', { name: "Rewrite this week's headers…" });

  // Cancel sends nothing.
  await trigger.click();
  const dialog = page.getByRole('dialog', { name: "Rewrite this week's headers?" });
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).toBeHidden();

  const posted = page.waitForResponse((r) => r.url().endsWith('/api/admin/headers/rewrite') && r.request().method() === 'POST');
  await trigger.click();
  await dialog.getByRole('button', { name: 'Rewrite headers' }).click();
  expect((await posted).status()).toBe(202);
  await expect(page.getByRole('group', { name: 'Notification' }).filter({ hasText: 'Rewriting 4 header(s); see the Rewrites log.' })).toBeVisible();
  await expect(card.getByRole('alert')).toHaveCount(0);

  // The run is still going: a second trigger is refused inline.
  await trigger.click();
  await dialog.getByRole('button', { name: 'Rewrite headers' }).click();
  await expect(card.getByRole('alert')).toHaveText('A header rewrite is already running; wait for it to finish (see the Rewrites log).');
});
