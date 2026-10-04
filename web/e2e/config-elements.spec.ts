import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// The M3E Config elements: Pings countdown chips (B_CfgPings), Re-read channel
// chips and the job card (B_CfgReread), Theme tiles (B_CfgTheme) and "Find a
// setting" jumping to a card.
const panel = (page: Page) => page.locator('.settings__panel:not([hidden])');

async function saved(page: Page): Promise<number[]> {
  const config = (await (await page.request.get(`${ADMIN}/api/admin/config`)).json()) as { pings: { countdown_minutes: number[] } };
  return config.pings.countdown_minutes;
}

test('pings: countdown chips add and remove by keyboard, announce, and save the same list', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=pings&sw=off`);
  const before = await saved(page);
  expect(before.length).toBeGreaterThan(1);
  const list = panel(page).getByRole('list', { name: 'Countdowns' });
  await expect(list.getByRole('listitem')).toHaveText(before.map((m) => new RegExp(`^${m} min`)));
  const add = panel(page).getByRole('textbox', { name: 'Add a countdown (minutes)' });
  const live = page.locator('.vh[role="status"]').filter({ hasText: /^(Added|Removed|That)/ });

  // Enter in the field adds; the save bar names the change.
  await add.fill('7');
  await expect(page.getByText(`Countdowns ${before.join(', ')} → ${[...before, 7].join(', ')}`)).toBeVisible();
  await add.press('Enter');
  await expect(list.getByRole('listitem').last()).toHaveText(/^7 min/);
  await expect(add).toHaveValue('');
  await expect(add).toBeFocused();
  await expect(live).toHaveText('Added 7 min.');

  // A repeat is refused politely; a word is refused inline.
  await add.fill(String(before[0]));
  await panel(page).getByRole('button', { name: 'Add', exact: true }).click();
  await expect(live).toHaveText('That countdown is already listed.');
  await expect(list.getByRole('listitem')).toHaveCount(before.length + 1);
  await add.fill('soon');
  await add.press('Enter');
  await expect(panel(page).getByRole('alert')).toHaveText('Countdowns are whole minutes, separated by commas.');
  await expect(add).toHaveAttribute('aria-invalid', 'true');
  await add.fill('');

  // The × by keyboard: focus moves to the next chip's ×.
  const first = list.getByRole('button', { name: `Remove the ${before[0]} min countdown` });
  await first.focus();
  await page.keyboard.press('Enter');
  await expect(live).toHaveText(new RegExp(`^Removed ${before[0]} min\\.`));
  await expect(list.getByRole('button', { name: `Remove the ${before[1]} min countdown` })).toBeFocused();

  const expected = [...before.slice(1), 7];
  const request = page.waitForRequest((r) => r.url().endsWith('/api/admin/config') && r.method() === 'PATCH');
  await panel(page).getByRole('button', { name: 'Save pings', exact: true }).click();
  expect((await request).postDataJSON()).toEqual({ pings: { day_of_ping_time: expect.any(String), countdown_minutes: expected } });
  await expect(page.getByText('All changes saved')).toBeVisible();
  expect(await saved(page)).toEqual(expected);

  // Removing every chip leaves nothing to save: refused like an empty comma field.
  const removes = list.getByRole('button', { name: /^Remove the/ });
  while ((await removes.count()) > 0) await removes.first().click();
  await expect(add).toBeFocused();
  await panel(page).getByRole('button', { name: 'Save pings', exact: true }).click();
  await expect(panel(page).getByRole('alert')).toHaveText('Countdowns are whole minutes, separated by commas.');
  expect(await saved(page)).toEqual(expected);
});

test('re-read: channel chips, a running job card that finishes, and Cancel', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=rescan&sw=off`);
  const channels = panel(page).getByRole('group', { name: /^Channels/ });
  const boxes = channels.getByRole('checkbox');
  const total = await boxes.count();
  expect(total).toBeGreaterThan(2);
  await expect(channels).toHaveAccessibleName(`Channels · 0 of ${total}`);
  // Space on a focused chip toggles it, as on any checkbox.
  await boxes.nth(0).focus();
  await page.keyboard.press('Space');
  await boxes.nth(1).check();
  await expect(boxes.nth(0)).toBeChecked();
  await expect(channels).toHaveAccessibleName(`Channels · 2 of ${total}`);
  await expect(page.locator('.rescan__chk').nth(0)).toHaveCSS('border-radius', '16px');
  await expect(page.locator('.rescan__chk').nth(2)).toHaveCSS('border-radius', '8px');

  const go = panel(page).getByRole('button', { name: 'Re-read', exact: true });
  await go.click();
  const card = panel(page).getByRole('region', { name: 'Re-reading 2 channels' });
  await expect(card).toBeVisible();
  // Messages read of the job's total, and the start in guild time (the mock's pinned noon).
  await expect(card.locator('.rescan__pct')).toHaveText(/^\d{1,3}%$/);
  await expect(card.locator('.rescan__status')).toHaveText(/^\d+ of \d+ messages · started 12:00$/);
  await expect(card.getByRole('button', { name: 'Cancel' })).toBeVisible();
  await expect(card).toContainText('Found so far:');
  await expect(go).toHaveAttribute('aria-disabled', 'true');
  const finished = panel(page).getByRole('region', { name: 'Re-read 2 channels' });
  await expect(finished).toBeVisible({ timeout: 10_000 });
  await expect(finished.locator('.rescan__status')).toHaveText(/^Done: 2 channels read, 0 changes proposed\. · \d+ messages read$/);
  await expect(finished).toContainText('100%');
  await expect(finished.getByRole('button', { name: 'Cancel' })).toHaveCount(0);
  await expect(finished.getByRole('link', { name: 'Extractions' })).toHaveAttribute('href', '/extractions');

  // A second run, cancelled at once: the card says so and the key has focus back.
  await panel(page).getByRole('button', { name: 'Select all party channels' }).click();
  await expect(channels).toHaveAccessibleName(`Channels · ${total} of ${total}`);
  await go.click();
  await panel(page).getByRole('region', { name: `Re-reading ${total} channels` }).getByRole('button', { name: 'Cancel' }).click();
  const stopped = panel(page).getByRole('region', { name: `Stopped re-reading ${total} channels` });
  await expect(stopped).toBeVisible();
  await expect(stopped.locator('.rescan__status')).toHaveText(new RegExp(`^Cancelled after \\d of ${total} channels\\.`));
  await expect(go).toBeFocused();
  await expect(go).toHaveAttribute('aria-disabled', 'false');
});

test('re-read on Extractions: the same panel, without the link back to itself', async ({ page }) => {
  await page.goto(`${ADMIN}/extractions?sw=off`);
  await page.getByRole('button', { name: 'Re-read channels' }).click();
  await page.getByRole('checkbox', { name: '#limbo-trio' }).check();
  await page.getByRole('button', { name: 'Re-read', exact: true }).click();
  const finished = page.getByRole('region', { name: 'Re-read 1 channel' });
  await expect(finished).toBeVisible({ timeout: 10_000 });
  await expect(finished.getByRole('link', { name: 'Extractions' })).toHaveCount(0);
});

test('theme: colourway tiles and the mode group apply at once and persist', async ({ page }) => {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem('seeded')) {
      localStorage.setItem('colorway', 'blossom');
      localStorage.removeItem('theme');
      sessionStorage.setItem('seeded', '1');
    }
  });
  await page.goto(`${ADMIN}/config?section=theme&sw=off`);
  const ways = panel(page).getByRole('group', { name: 'Colourway' });
  // Labelled sets inside the one radio group; character names, stored keys unchanged.
  for (const set of ['Base', 'Blue Archive', 'Terminal', 'Dynamic']) await expect(ways.getByRole('group', { name: set })).toBeVisible();
  const toggle = (set: string) => ways.getByRole('button', { name: set, exact: true });
  // Only the current colourway's set starts open; collapsed sets hide their radios.
  await expect(toggle('Base')).toHaveAttribute('aria-expanded', 'true');
  for (const set of ['Blue Archive', 'Terminal', 'Dynamic']) await expect(toggle(set)).toHaveAttribute('aria-expanded', 'false');
  await expect(ways.getByRole('radio')).toHaveCount(4);
  await expect(ways.getByRole('radio', { name: 'Nazuna' })).toBeChecked();
  await ways.getByRole('radio', { name: 'Hinano' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'twilight');
  // Arrow keys skip collapsed sets: from the last open radio they wrap to the first.
  await ways.getByRole('radio', { name: 'Hinano' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'marigold');
  // Expanded, a set joins the ring; choosing in it leaves both sets open.
  await toggle('Blue Archive').click();
  await expect(toggle('Blue Archive')).toHaveAttribute('aria-expanded', 'true');
  await expect(ways.getByRole('group', { name: 'Blue Archive' }).getByRole('radio')).toHaveCount(5);
  await ways.getByRole('radio', { name: 'Hinano' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'hoshino');
  await expect(toggle('Base')).toHaveAttribute('aria-expanded', 'true');
  await expect(ways.getByRole('radio')).toHaveCount(9);
  const modes = panel(page).getByRole('group', { name: 'Mode' });
  await expect(modes.getByRole('radio', { name: 'System' })).toBeChecked();
  await modes.getByRole('radio', { name: 'Dark' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  // The contents list follows, and the choice outlives a reload.
  await expect(page.getByRole('tab', { name: /^Theme/ })).toContainText('hoshino');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-colorway', 'hoshino');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(panel(page).getByRole('group', { name: 'Colourway' }).getByRole('radio', { name: 'Hoshino' })).toBeChecked();
  // Expanded sets are not stored: after a reload only the current set is open.
  await expect(panel(page).getByRole('button', { name: 'Base', exact: true })).toHaveAttribute('aria-expanded', 'false');
  await expect(panel(page).getByRole('button', { name: 'Blue Archive', exact: true })).toHaveAttribute('aria-expanded', 'true');
  await expect(panel(page).getByRole('group', { name: 'Mode' }).getByRole('radio', { name: 'Dark' })).toBeChecked();
  expect(await page.evaluate(() => [localStorage.getItem('colorway'), localStorage.getItem('theme')])).toEqual(['hoshino', 'dark']);
});

test('find a setting: Enter jumps to the matching card, rings it and focuses its field', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/config?section=theme&sw=off`);
  const search = page.getByRole('searchbox', { name: 'Find a setting' });
  await search.fill('countdowns');
  await search.press('Enter');
  await expect(page).toHaveURL(/section=pings/);
  const card = panel(page).locator('form.settings__card');
  await expect(card).toHaveClass(/settings__found/);
  await expect(panel(page).getByRole('textbox', { name: 'Add a countdown (minutes)' })).toBeFocused();
  await expect(page.locator('.vh[role="status"]').filter({ hasText: /^Found/ })).toHaveText('Found Pings: Countdowns');
  // The ring fades and goes.
  await expect(card).not.toHaveClass(/settings__found/, { timeout: 3_000 });

  // A setting behind a pill tab: the tab opens and the card scrolls into the panel.
  await search.fill('capacity groups');
  await search.press('Enter');
  await expect(page).toHaveURL(/section=models/);
  await expect(panel(page).getByRole('tab', { name: /^Capacity/ })).toHaveAttribute('aria-selected', 'true');
  const groups = panel(page).locator('.settings__found');
  await expect(groups).toContainText('Capacity groups');
  await expect(groups).toBeInViewport();
  const frame = await page.evaluate(() => document.scrollingElement!.scrollTop);
  expect(frame).toBe(0);
});

test('find a setting: with reduced motion the ring is static', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${ADMIN}/config?sw=off`);
  const search = page.getByRole('searchbox', { name: 'Find a setting' });
  await search.fill('quiet mode');
  await search.press('Enter');
  await expect(page).toHaveURL(/section=notifications/);
  const found = panel(page).locator('.settings__found');
  await expect(found).toContainText('Quiet mode');
  await expect(found).toHaveCSS('animation-name', 'none');
  await expect(found).toHaveCSS('outline-style', 'solid');
  await expect(panel(page).getByRole('switch', { name: /Quiet mode/ })).toBeFocused();
});
