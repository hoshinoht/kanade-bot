import { ADMIN, expect, test } from './support';

// Shared pane states (M3E B_Empty, B_States): an empty Inbox tab says why and
// shows the tab's last decisions; a pane whose read failed offers a retry.

test('an empty Extractor tab: the last read, the ways on, and the last three decisions', async ({ page }) => {
  await page.route('**/api/admin/inbox', (route) => route.fulfill({ json: [] }));
  await page.goto(`${ADMIN}/inbox`);
  await expect(page.getByRole('heading', { name: 'Nothing waiting' })).toBeVisible();
  await expect(page.getByText(/watched channel\. The last read was \w{3} \d+ \d\d:\d\d\sin #\S+\./)).toBeVisible();
  await expect(page.getByRole('link', { name: 'See recent extractions' })).toHaveAttribute('href', '/extractions');
  await expect(page.getByRole('link', { name: 'Re-read channels' })).toHaveAttribute('href', '/config?section=rescan');
  const recent = page.getByRole('region', { name: 'Recently decided' }).getByRole('link');
  await expect(recent.first()).toBeVisible();
  expect(await recent.count()).toBeLessThanOrEqual(3);
  await recent.first().click();
  await expect(page).toHaveURL(/tab=past&item=/);
  await expect(page.getByRole('listbox', { name: 'Past items' })).toBeVisible();
});

test('a failed read: the reason, Try again reloads the pane in place', async ({ page }) => {
  let fail = true;
  await page.route('**/api/admin/members', (route) =>
    fail ? route.fulfill({ status: 503, json: { error: 'unavailable', message: 'The server took too long to answer.' } }) : route.continue(),
  );
  await page.goto(`${ADMIN}/members`);
  const alert = page.getByRole('alert').filter({ hasText: 'Couldn’t load members' });
  await expect(alert).toContainText('The server took too long to answer.');
  fail = false;
  await alert.getByRole('button', { name: 'Try again' }).click();
  await expect(alert).toBeHidden();
  await expect(page.getByRole('list', { name: 'Members' }).getByRole('listitem').first()).toBeVisible();
});
