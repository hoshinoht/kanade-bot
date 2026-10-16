import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, PUBLIC, ageEstimate, csrf, expect, setCharacterData, setEstimates, setMemberCharacters, setModules, settle, signInPublic, test } from './support';

// The member's own characters on the portal (module `roster`; boards in
// docs/research/2026-10-13-member-characters) over the seed's member Asahi:
// `setMemberCharacters('board')` writes her characters as the boards draw them
// (AsahiKanna linked main with one share card, AsahiDK pending on Ren's
// request, AsahiPath stale, AsahiBlaze expired, AsahiMech retired; ids
// `ch-b1`…`ch-b5`), `'empty'` gives her none. Every test ends with zero CSP/TT reports.

const ASAHI = '100000000000001001';

test.describe.configure({ mode: 'parallel' });

const ATTRIBUTION = 'Data based on NEXON Open API';
const toast = (page: Page) => page.getByRole('group', { name: 'Notification' });
const panel = (page: Page) => page.getByRole('tabpanel', { name: /^Characters/ });
const row = (page: Page, ign: string) => panel(page).locator('.rosterrow').filter({ hasText: ign });

async function roster(_page: Page, scenario: 'board' | 'empty' = 'board') {
  await setMemberCharacters(scenario);
}

async function modules(_page: Page, data: { roster?: boolean; character_data?: boolean }) {
  await setModules(data);
}

/** Signed in, with the boards' characters, on Account › Characters. */
async function openTab(page: Page, query = ''): Promise<Locator> {
  await signInPublic(page);
  await roster(page);
  await page.goto(`${PUBLIC}/account?tab=characters${query}&sw=off`);
  await expect(panel(page).locator('.rosterrow').first()).toBeVisible();
  return panel(page);
}

async function openEdit(page: Page, ign: string): Promise<Locator> {
  await row(page, ign).getByRole('button', { name: `Edit ${ign}` }).click();
  const dialog = page.getByRole('dialog', { name: `Edit ${ign}` });
  await expect(dialog).toBeVisible();
  return dialog;
}

test('Characters tab: the link waiting for you first, the roster with link chips, NEXON data', async ({ page }) => {
  const tab = await openTab(page);
  await expect(page.getByRole('tab', { name: /^Characters/ })).toHaveText('Characters4');
  const waiting = tab.getByRole('region', { name: 'Waiting for you' });
  await expect(waiting).toContainText('AsahiDK · Dark Knight');
  await expect(waiting).toContainText('Waiting since 29 Sep for you to confirm the NEXON notice. It lapses 13 Oct.');
  await expect(waiting).not.toContainText('asked');
  await expect(waiting.getByRole('button', { name: 'Review and link…' })).toBeVisible();
  await expect(tab.locator('.rosterpanel__summary')).toHaveText(/4\s*active\s*·\s*1\s*retired/);
  for (const [ign, chip] of [
    ['AsahiKanna', 'linked'],
    ['AsahiDK', 'waiting for you'],
    ['AsahiPath', 'stale'],
    ['AsahiBlaze', 'expired'],
    ['AsahiMech', 'not linked'],
  ])
    await expect(row(page, ign).locator('.status-chip')).toHaveText(chip);
  await expect(row(page, 'AsahiKanna').locator('.rosterrow__crown')).toHaveText('main');
  await expect(tab).toContainText('The same roster the admins see.');
  const data = tab.getByRole('region', { name: 'NEXON data' });
  await expect(data.getByRole('button', { name: 'My data…' })).toBeVisible();
  await expect(data.getByRole('link', { name: 'Privacy notice' })).toBeVisible();
  await expect(data).toContainText(ATTRIBUTION);
});

test('row menu: Make main and Retire with Undo; no Delete; the main waits, explained', async ({ page }) => {
  await openTab(page);
  await row(page, 'AsahiPath').getByRole('button', { name: 'More actions for AsahiPath' }).click();
  const menu = page.getByRole('menu', { name: 'AsahiPath' });
  await expect(menu.getByRole('menuitem')).toHaveText(['Make main', 'Retire']);
  await expect(menu).toContainText('Only an admin deletes a character. Retire keeps it for history.');
  await expect(menu.getByRole('menuitem', { name: /Delete/ })).toHaveCount(0);
  await menu.getByRole('menuitem', { name: 'Retire' }).click();
  await expect(toast(page).filter({ hasText: 'AsahiPath retired. Kept for history.' })).toBeVisible();
  await expect(row(page, 'AsahiPath')).toHaveClass(/rosterrow--retired/);
  await toast(page).filter({ hasText: 'AsahiPath retired' }).getByRole('button', { name: 'Undo' }).click();
  await expect(toast(page).filter({ hasText: 'Brought back AsahiPath.' })).toBeVisible();
  await expect(row(page, 'AsahiPath')).not.toHaveClass(/rosterrow--retired/);

  await row(page, 'AsahiKanna').getByRole('button', { name: 'More actions for AsahiKanna' }).click();
  const main = page.getByRole('menu', { name: 'AsahiKanna' });
  await expect(main.getByRole('menuitem', { name: 'Retire' })).toHaveAttribute('aria-disabled', 'true');
  await expect(main.getByRole('menuitem', { name: 'Retire' })).toHaveAccessibleDescription('AsahiKanna is your main. Make another character main first.');
  await page.keyboard.press('Escape');

  await row(page, 'AsahiDK').getByRole('button', { name: 'More actions for AsahiDK' }).click();
  await page.getByRole('menu', { name: 'AsahiDK' }).getByRole('menuitem', { name: 'Make main' }).click();
  await expect(toast(page).filter({ hasText: 'AsahiDK is your main now.' })).toBeVisible();
  await expect(row(page, 'AsahiDK').locator('.rosterrow__crown')).toBeVisible();
});

test('Add: Look up shows NEXON once, never kept; then the notice opens, and Link links it', async ({ page }) => {
  await openTab(page);
  await panel(page).getByRole('button', { name: 'Add character' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add character' });
  await expect(dialog).toContainText('Your character');
  await dialog.getByLabel('IGN').fill('AsahiLumi');
  await dialog.getByRole('button', { name: 'Look up' }).click();
  const card = dialog.getByRole('region', { name: 'NEXON lookup' });
  await expect(card).toContainText('found');
  await expect(card).toContainText('Shown only; kept only once you link it.');
  await expect(card).toContainText(ATTRIBUTION);
  await expect(dialog).toContainText('Next, Kanade shows you the NEXON notice for AsahiLumi.');
  await expect(dialog.getByRole('button', { name: /Delete/ })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Add character' }).click();
  await expect(toast(page).filter({ hasText: 'Added AsahiLumi.' })).toBeVisible();
  const notice = page.getByRole('dialog', { name: 'Link AsahiLumi to its NEXON Open API data?' });
  await expect(notice).toContainText('Nothing is fetched until you press Link.');
  await notice.getByRole('button', { name: 'Link AsahiLumi' }).click();
  await expect(notice.getByRole('status')).toContainText('Linked AsahiLumi');
  await expect(notice.getByRole('status')).toContainText('its class and level are copied to your roster character. See all of it in My data; remove it with Unlink.');
  await notice.getByRole('button', { name: 'Done' }).click();
  await expect(row(page, 'AsahiLumi').locator('.status-chip')).toHaveText('linked');
});

test('Add without NEXON connected asks class and level by hand', async ({ page }) => {
  await setCharacterData('setup');
  await openTab(page);
  await panel(page).getByRole('button', { name: 'Add character' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add character' });
  await expect(dialog.getByRole('combobox', { name: 'Class' })).toBeVisible();
  await expect(dialog.getByLabel('Level')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Look up' })).toHaveCount(0);
  await expect(row(page, 'AsahiMech').locator('.status-chip')).toHaveText('not connected');
});

test('the notice: v1 word for word; Cancel keeps the waiting link', async ({ page }) => {
  const tab = await openTab(page);
  await tab.getByRole('button', { name: 'Review and link…' }).click();
  const notice = page.getByRole('dialog', { name: 'Link AsahiDK to its NEXON Open API data?' });
  await expect(notice.locator('.linknotice__lead')).toHaveText('Nothing is fetched until you press Link.');
  for (const heading of ['What Kanade fetches', 'Why', 'How long', 'Who sees it']) await expect(notice.getByRole('heading', { name: heading })).toBeVisible();
  await expect(notice).toContainText('Refreshed once a day and kept 30 days at most. /character unlink deletes it at once; the class and level copied to your roster character stay.');
  await expect(notice).toContainText('Its class and level are copied to your roster character, which guild members see.');
  await expect(notice).toContainText('Kanade is not affiliated with or endorsed by NEXON.');
  await expect(notice.getByRole('link', { name: 'Privacy notice' })).toBeVisible();
  await expect(notice).toContainText(ATTRIBUTION);
  await expect(notice).toContainText('notice 2');
  await notice.getByRole('button', { name: 'Cancel' }).click();
  await expect(notice.getByRole('status')).toContainText('Nothing was linked.');
  await expect(notice.getByRole('status')).toContainText('AsahiDK is still waiting for you; Review and link when you’re ready.');
  await notice.getByRole('button', { name: 'Done' }).click();
  await expect(row(page, 'AsahiDK').locator('.status-chip')).toHaveText('waiting for you');
});

test('Link outcomes: busy, not found and data later, in copy.rs words', async ({ page }) => {
  await openTab(page);
  // The real busy answer is the day's spent NEXON budget, which refuses every link after it,
  // so Busy comes last, once the budget is spent.
  for (const [ign, words] of [
    ['NopeKnight', 'NEXON couldn’t find NopeKnight in MapleSEA.'],
    ['LaterKnight', 'NEXON didn’t answer just now, so nothing is kept yet'],
    ['BusyKnight', 'Kanade has used today’s NEXON lookups.'],
  ]) {
    if (ign === 'BusyKnight') await setCharacterData('budget');
    await panel(page).getByRole('button', { name: 'Add character' }).click();
    const dialog = page.getByRole('dialog', { name: 'Add character' });
    await dialog.getByLabel('IGN').fill(ign);
    await dialog.getByRole('button', { name: 'Add character' }).click();
    const notice = page.getByRole('dialog', { name: `Link ${ign} to its NEXON Open API data?` });
    await notice.getByRole('button', { name: `Link ${ign}` }).click();
    await expect(notice.getByRole('status')).toContainText(words);
    await notice.getByRole('button', { name: 'Done' }).click();
    await expect(notice).toBeHidden();
  }
});

test('fresh sign-in: Add and Link open Confirm it’s you, which comes back with the flow open', async ({ page }) => {
  // Signed in before the fresh-write window; never sign in again after it.
  await signInPublic(page, { stale: true });
  await roster(page);
  await page.goto(`${PUBLIC}/account?tab=characters&sw=off`);
  await panel(page).getByRole('button', { name: 'Add character' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add character' });
  await dialog.getByLabel('IGN').fill('AsahiLumi');
  await dialog.getByRole('button', { name: 'Add character' }).click();
  const confirm = page.getByRole('dialog', { name: 'Confirm it\'s you' });
  await expect(confirm).toContainText(/Adding a character needs a Discord sign-in from the last \d+ minutes/);
  await expect(confirm).toContainText('Nothing was added. After signing in you come back to Characters with Add character open again.');
  await expect(confirm).toContainText('Reading your characters and My data doesn’t need this.');
  await expect(confirm.getByRole('link', { name: 'Sign in again' })).toHaveAttribute('href', /next=%2Faccount%3Ftab%3Dcharacters%26add%3D1/);
  await confirm.getByRole('button', { name: 'Not now' }).click();
  await expect(row(page, 'AsahiLumi')).toHaveCount(0);

  await panel(page).getByRole('button', { name: 'Review and link…' }).click();
  await page.getByRole('dialog', { name: /Link AsahiDK/ }).getByRole('button', { name: 'Link AsahiDK' }).click();
  await expect(confirm).toContainText('Linking a character needs');
  await expect(confirm).toContainText('the notice for AsahiDK open again.');
  await expect(confirm.getByRole('link', { name: 'Sign in again' })).toHaveAttribute('href', /link%3Dch-b2/);
});

test('deep links reopen Add, Edit, the notice and My data', async ({ page }) => {
  await openTab(page, '&add=1');
  await expect(page.getByRole('dialog', { name: 'Add character' })).toBeVisible();
  await expect(page).not.toHaveURL(/add=1/);
  await page.goto(`${PUBLIC}/account?tab=characters&edit=ch-b3&sw=off`);
  await expect(page.getByRole('dialog', { name: 'Edit AsahiPath' })).toBeVisible();
  await page.goto(`${PUBLIC}/account?tab=characters&link=ch-b2&sw=off`);
  await expect(page.getByRole('dialog', { name: 'Link AsahiDK to its NEXON Open API data?' })).toBeVisible();
  await page.goto(`${PUBLIC}/account?tab=characters&data=1&sw=off`);
  await expect(page.getByRole('dialog', { name: 'What Kanade keeps for you' })).toBeVisible();
});

test('Edit: the linked strip has no Refresh; Unlink deletes at once with copy.rs words', async ({ page }) => {
  await openTab(page);
  const dialog = await openEdit(page, 'AsahiKanna');
  await expect(dialog).toContainText('Your character · updated');
  await expect(dialog.getByRole('switch', { name: 'Retired' })).toHaveAttribute('aria-disabled', 'true');
  const strip = dialog.getByRole('region', { name: 'Character data' });
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('linked');
  await expect(strip).toContainText('Lv. 286Kanna·CP 51.4M');
  await expect(strip).toContainText('with the daily batch. Unlink deletes it now; your roster character keeps its class and level.');
  await expect(strip.locator('.chardata__foot')).toContainText('Linked 19 Sep by you (notice 2) · refreshed daily');
  await expect(strip.getByRole('button', { name: 'Refresh' })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: /Delete/ })).toHaveCount(0);
  await strip.getByRole('button', { name: 'Unlink' }).click();
  await expect(
    toast(page).filter({ hasText: 'Unlinked AsahiKanna. Its NEXON data is deleted, and your 1 shared card now reads “Character data removed”. Your roster character keeps its class and level.' }),
  ).toBeVisible();
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('not linked');
  await strip.getByRole('button', { name: 'Link…' }).click();
  await expect(page.getByRole('dialog', { name: 'Link AsahiKanna to its NEXON Open API data?' })).toBeVisible();
});

test('the strip in its other states: waiting, stale, expired', async ({ page }) => {
  await openTab(page);
  let strip = (await openEdit(page, 'AsahiDK')).getByRole('region', { name: 'Character data' });
  await expect(strip).toContainText('Waiting for you to link it');
  await expect(strip).toContainText('Since 29 Sep');
  await expect(strip).toContainText('nothing fetched yet');
  await expect(strip).toContainText('Cancel there keeps it waiting; it lapses 13 Oct.');
  await expect(strip.getByRole('button', { name: 'Review and link…' })).toBeVisible();
  await page.keyboard.press('Escape');
  strip = (await openEdit(page, 'AsahiPath')).getByRole('region', { name: 'Character data' });
  await expect(strip.getByRole('button', { name: 'Link again…' })).toBeVisible();
  await expect(strip.getByRole('button', { name: 'Unlink' })).toBeVisible();
  await page.keyboard.press('Escape');
  strip = (await openEdit(page, 'AsahiBlaze')).getByRole('region', { name: 'Character data' });
  await expect(strip).toContainText('Kanade tries again with the next daily batch');
});

test('My data: everything kept, the waiting link first; Unlink and Unlink all', async ({ page }) => {
  const tab = await openTab(page);
  await tab.getByRole('button', { name: 'My data…' }).click();
  const dialog = page.getByRole('dialog', { name: 'What Kanade keeps for you' });
  const entries = dialog.locator('.mydata__entry');
  await expect(entries.first()).toContainText('AsahiDK');
  await expect(entries.first()).toContainText('waiting for you');
  await expect(entries.first()).toContainText('Waiting since 29 Sep. Nothing is fetched until you confirm.');
  const kanna = entries.filter({ hasText: 'AsahiKanna' });
  await expect(kanna).toContainText('Kanna · Lv. 286 · CP 51,400,000');
  await expect(kanna).toContainText('notice 2 · 1 shared card in #hstar-party');
  await expect(dialog).toContainText(ATTRIBUTION);
  await dialog.getByRole('button', { name: 'Unlink AsahiBlaze' }).click();
  await expect(toast(page).filter({ hasText: 'Unlinked AsahiBlaze. Its NEXON data is deleted; your roster character keeps its class and level.' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Unlink all' }).click();
  await expect(toast(page).filter({ hasText: /^Unlinked everything left: / })).toBeVisible();
  await expect(dialog.getByRole('button', { name: /^Unlink/ })).toHaveCount(0);
});

test('Unlink all: a 503 keeps the key, and Try again finishes the same action', async ({ page }) => {
  const tab = await openTab(page);
  const keys: string[] = [];
  let first = true;
  await page.route('**/api/public/me/characters/links', async (route) => {
    keys.push(route.request().headers()['idempotency-key'] ?? '');
    if (first) {
      first = false;
      await route.fulfill({ status: 503, contentType: 'application/json', body: '{"error":"unlink_incomplete","message":"Some links could not be removed."}' });
    } else await route.continue();
  });
  await tab.getByRole('button', { name: 'My data…' }).click();
  const dialog = page.getByRole('dialog', { name: 'What Kanade keeps for you' });
  await dialog.getByRole('button', { name: 'Unlink all' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Not everything was unlinked yet: Some links could not be removed. Try again finishes it.');
  await dialog.getByRole('button', { name: 'Try again' }).click();
  await expect(toast(page).filter({ hasText: /^Unlinked everything left: / })).toBeVisible();
  await expect(dialog.getByRole('alert')).toHaveCount(0);
  expect(keys).toHaveLength(2);
  expect(keys[0]).toBeTruthy();
  expect(keys[1]).toBe(keys[0]);
});

test('my scouter lookup: result, then the 30-minute limit names the next time', async ({ page }) => {
  await openTab(page);
  const dialog = await openEdit(page, 'AsahiKanna');
  const row = dialog.locator('.estimate');
  await expect(row).toContainText('Your own characters only.');
  const button = row.getByRole('button', { name: /^Look up/ });
  await button.click();
  await expect(row.getByRole('table')).toBeVisible({ timeout: 20_000 });
  await expect(row).toContainText('by you');
  await row.getByRole('button', { name: 'Look up again' }).click();
  const next = row.getByRole('button', { name: 'Next lookup at 12:30' });
  await expect(next).toHaveAttribute('aria-disabled', 'true');
  await expect(next).toHaveAccessibleDescription(/You can start one lookup every 30 minutes\. Next lookup at 12:30\./);
  await expect(row.getByRole('table')).toBeVisible();
});

test('my scouter lookup: queued behind the others’ lookups', async ({ page }) => {
  await setEstimates('queued');
  await openTab(page);
  const row = (await openEdit(page, 'AsahiPath')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up' }).click();
  await expect(row).toContainText(/Queued · \d+ ahead/);
  await expect(row).toContainText('the admins’ and other members’ too');
  await expect(row.getByRole('button', { name: 'Queued…' })).toHaveAttribute('aria-disabled', 'true');
});

test('my scouter lookup: a failure doesn’t count toward the 30 minutes', async ({ page }) => {
  await setEstimates('timeout');
  await openTab(page);
  const row = (await openEdit(page, 'AsahiBlaze')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up' }).click();
  await expect(row.getByRole('alert')).toContainText('Nothing was kept, and this try doesn’t count toward your 30 minutes.', { timeout: 20_000 });
  // Not counted: the next start is allowed at once.
  await setEstimates('ok');
  await row.getByRole('button', { name: 'Try again' }).click();
  await expect(row.getByRole('table')).toBeVisible({ timeout: 20_000 });
});

test('my scouter lookup: a failed 100% crit assumption says the estimate couldn’t be completed', async ({ page }) => {
  await setEstimates('assumption_failed');
  await openTab(page);
  const row = (await openEdit(page, 'AsahiBlaze')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up' }).click();
  const alert = row.getByRole('alert');
  await expect(alert).toContainText('The estimate couldn’t be completed for this character.', { timeout: 20_000 });
  await expect(alert).toContainText('The scouter tried assuming 100% critical rate for AsahiBlaze and couldn’t finish. Try again later.');
  await expect(alert).not.toContainText('Set up critical rate');
  await expect(row.getByRole('button', { name: 'Try again' })).toBeVisible();
});

test('run Party · characters: others show roster level and estimates, only you see your NEXON level', async ({ page }) => {
  await signInPublic(page);
  await roster(page);
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  const section = page.getByRole('region', { name: 'Party · characters' });
  await expect(section).toBeVisible();
  const rows = section.locator('.party-char');
  await expect(rows.first()).toContainText('You');
  await expect(rows.first()).toContainText('AsahiKanna');
  await expect(rows.first()).toContainText('NEXON · as of 29 Sep · only you see this');
  const others = rows.filter({ hasNotText: 'only you see this' });
  expect(await others.count()).toBeGreaterThan(0);
  for (const other of await others.all()) await expect(other).not.toContainText('NEXON');
  await expect(others.filter({ hasText: 'roster' }).first()).toBeVisible();
  await expect(section.getByText('Clear-time estimate')).toBeVisible();
  await expect(section.getByText(ATTRIBUTION)).toBeVisible();
  // Estimates on, nothing looked up yet: each character says so (never 0%).
  await expect(section.locator('.party-char__none').filter({ hasText: 'No estimate yet' }).first()).toBeVisible();
  await expect(section).not.toContainText('in the last 24 h');
  // The party read itself never carries another member's NEXON data.
  const party = (await (await page.request.get(`${PUBLIC}/api/public/runs/r-carling/characters`)).json()) as { slots: { you: boolean; character: { level_source: string } | null }[] };
  expect(party.slots.filter((s) => !s.you && s.character?.level_source === 'nexon')).toEqual([]);
});

test('run Party · characters on a phone: each slot gains its character line', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signInPublic(page);
  await roster(page);
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  const list = page.getByRole('list', { name: 'Party characters' });
  await expect(list.locator('.member-slot--chars').first()).toContainText('NEXON · as of 29 Sep · only you see this');
  await expect(page.locator('.party-chars__note')).toContainText('Clear-time estimate');
});

/** An admin looks the run's party up, as the portal's cached readings and run total need. */
async function lookedUpRun(page: Page, run: string) {
  const headers = await csrf(page.request);
  const { job } = await (await page.request.post(`${ADMIN}/api/admin/estimates`, { headers, data: { run_id: run } })).json();
  await expect.poll(async () => (await (await page.request.get(`${ADMIN}/api/admin/estimates/${job.id}`)).json()).state, { timeout: 30_000 }).toBe('done');
}

test('run Party · characters: the run total, then Who carries with its key, then the members', async ({ page }) => {
  await signInPublic(page);
  await roster(page);
  const before = (await (await page.request.get(`${PUBLIC}/api/public/runs/r-carling/characters`)).json()) as { totals: unknown };
  expect(before.totals).toBeNull();
  await lookedUpRun(page, 'r-carling');
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  const section = page.getByRole('region', { name: 'Party · characters' });
  const total = section.locator('.estimate__combined');
  await expect(total.locator('.chardata__col')).toHaveCount(2);
  await expect(total).toContainText('Karling');
  await expect(total).toContainText(/\d+(\.\d)? min ?· \d+\.\d%/);
  await expect(total).toContainText('technically possible');
  // Below the note: total, chart, then the members; no swatch on a member row.
  const order = await section.evaluate((el) => [...el.children].map((child) => child.className.split(' ')[0]));
  expect(order.slice(order.indexOf('party-chars__note'))).toEqual(['party-chars__note', 'estimate__combined', 'share', 'party-chars']);
  await expect(section.locator('.party-char .share__sw')).toHaveCount(0);
  const chart = section.locator('figure.share');
  const bars = chart.getByRole('img');
  await expect(bars).toHaveCount(2);
  for (const bar of await bars.all()) {
    const shares = (await bar.locator('.share__seg').evaluateAll((els) => els.map((el) => el.getAttribute('title') ?? ''))).map((t) => Number(/· (\d+)% of/.exec(t)![1]));
    expect(shares.reduce((a, b) => a + b, 0)).toBe(100);
  }
  // The key: party order, "You" for the signed-in member, the same numbers as the parts.
  const keys = chart.getByRole('list', { name: 'Who is which number' }).getByRole('listitem');
  const parts = await bars.first().locator('.share__n').allTextContents();
  expect(await keys.locator('.share__sw').allTextContents()).toEqual(parts);
  await expect(keys.filter({ hasText: 'You' })).toHaveCount(1);
  // Ren (2) brings no character: counted, never a 0% part (the portal names members as the party list does).
  await expect(chart.locator('.share__note').last()).toHaveText('1 with no estimate: Ren (not in the total, never 0%)');
});

test('run Party · characters: a lookup at least estimate_days old still shows, marked "older than n days"', async ({ page }) => {
  await signInPublic(page);
  await roster(page);
  // The party looked up 8 days before the pinned clock: past the default 7.
  await ageEstimate({ run: 'r-carling' }, 8);
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  const section = page.getByRole('region', { name: 'Party · characters' });
  await expect(section.locator('.estimate__combined')).toContainText('Karling');
  await expect(section.locator('.estimate__combined + .chardata__sub')).toHaveText('Run total · older than 7 days');
  const marked = section.locator('.party-char').filter({ has: page.locator('.party-char__none', { hasText: /^Estimate · older than 7 days$/ }) });
  await expect(marked.first()).toBeVisible();
  // Every member with a reading is marked; the readings and the chart still show.
  expect(await marked.count()).toBe(await section.locator('.party-char').filter({ has: page.locator('.ready__scouter') }).count());
  await expect(section.locator('figure.share')).toBeVisible();

  // Fresh again after a new lookup: no marker.
  await ageEstimate({ run: 'r-carling' }, 1);
  await page.reload();
  await expect(section.locator('.estimate__combined')).toContainText('Karling');
  await expect(section).not.toContainText('older than');
});

test('my scouter lookup: a result at least estimate_days old reads "older than n days"', async ({ page }) => {
  await openTab(page);
  await ageEstimate({ character: 'ch-b3' }, 8);
  const row = (await openEdit(page, 'AsahiPath')).locator('.estimate');
  await expect(row.getByRole('table')).toBeVisible();
  await expect(row.locator('.chardata__foot')).toContainText(/^Looked up 21 Sep \d{2}:\d{2} by .+ · older than 7 days · bosses outside the catalog are left out$/);
  await expect(row).not.toContainText('in memory');
});

test('other members’ characters, lookups and runs are the generic 404', async ({ page }) => {
  await signInPublic(page);
  await roster(page);
  // ch-s3 is Mika's MikaBish.
  for (const path of ['/api/public/me/characters/ch-s3/link', '/api/public/me/characters/ch-s3/link/notice', '/api/public/me/estimates?character_id=ch-s3'])
    expect((await page.request.get(`${PUBLIC}${path}`)).status(), path).toBe(404);
  const mine = (await (await page.request.get(`${PUBLIC}/api/public/me/characters`)).json()) as { characters: { id: string }[] };
  expect(mine.characters.map((c) => c.id)).not.toContain('ch-s3');
  // Once Asahi is off r-carling (an admin takes her off), it has no party characters for her.
  const { version } = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number };
  const removed = await page.request.patch(`${ADMIN}/api/admin/runs/r-carling/participants`, { headers: await csrf(page.request), data: { remove: ASAHI, version } });
  expect(removed.ok(), await removed.text()).toBe(true);
  expect((await page.request.get(`${PUBLIC}/api/public/runs/r-carling/characters`)).status()).toBe(404);
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  await expect(page.getByRole('complementary', { name: /^View only/ })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Party · characters' })).toHaveCount(0);
});

test('module off: no Characters tab, the deep link shows Profile, the run shows no characters', async ({ page }) => {
  await modules(page, { roster: false });
  await signInPublic(page);
  const asked: string[] = [];
  page.on('request', (r) => /\/api\/public\/(me\/characters|runs\/[^/]+\/characters)/.test(r.url()) && asked.push(r.url()));
  await page.goto(`${PUBLIC}/account?tab=characters&sw=off`);
  await expect(page.getByRole('tab', { name: 'Profile' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('tab', { name: /^Characters/ })).toHaveCount(0);
  await expect(page.getByRole('tab')).toHaveText(['Profile', /^Devices/, 'This browser']);
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  await expect(page.getByRole('complementary', { name: /^Your run/ })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Party · characters' })).toHaveCount(0);
  // One probe each, answered with the generic 404.
  expect(asked.length).toBeLessThanOrEqual(2);
});

test('character data off: the tab stays, with no chips, strip, estimates or NEXON section', async ({ page }) => {
  await modules(page, { character_data: false });
  const tab = await openTab(page);
  await expect(tab.locator('.rosterrow .status-chip')).toHaveCount(0);
  await expect(tab.getByRole('region', { name: 'NEXON data' })).toHaveCount(0);
  const dialog = await openEdit(page, 'AsahiKanna');
  await expect(dialog.getByRole('switch', { name: 'Retired' })).toBeVisible();
  await expect(dialog.getByRole('region', { name: 'Character data' })).toHaveCount(0);
});

test('no characters: the empty note and Add', async ({ page }) => {
  await signInPublic(page);
  await roster(page, 'empty');
  await page.goto(`${PUBLIC}/account?tab=characters&sw=off`);
  await expect(panel(page).getByRole('heading', { name: 'No characters yet' })).toBeVisible();
  await expect(panel(page)).toContainText('Your first character becomes your main; add alts after it. The admins see the same list.');
  await expect(panel(page).getByRole('region', { name: 'Waiting for you' })).toHaveCount(0);
  await expect(panel(page).getByRole('region', { name: 'NEXON data' })).toHaveCount(0);
  await panel(page).getByRole('button', { name: 'Add character' }).click();
  await expect(page.getByRole('dialog', { name: 'Add character' })).toContainText('Your first character becomes your main.');
});

// Captures of every board's screen for review: `KANADE_CAPTURE_DIR=<dir> bunx playwright test
// public-characters -g capture`, one PNG per board (or per state cell: `M_CharStates-empty.png`).
const CAPTURES = process.env.KANADE_CAPTURE_DIR ?? '';

type Shot = { file: string; phone?: boolean; setup?: (page: Page) => Promise<void>; path?: string; act?: (page: Page) => Promise<void> };

const editOf = (ign: string) => async (page: Page) => {
  await openEdit(page, ign);
};
const lookedUp = (ign: string, button = /^Look up/) => async (page: Page) => {
  const row = (await openEdit(page, ign)).locator('.estimate');
  await row.getByRole('button', { name: button }).click();
  await expect(row.locator('table, [role="alert"], .estimate__progress').first()).toBeVisible({ timeout: 20_000 });
};
const scenario = (name: string) => async () => {
  await setEstimates(name, name === 'assumed' ? 'AsahiKanna' : undefined);
};
const addAndLink = (ign: string) => async (page: Page) => {
  await panel(page).getByRole('button', { name: 'Add character' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add character' });
  await dialog.getByLabel('IGN').fill(ign);
  await dialog.getByRole('button', { name: 'Add character' }).click();
  await page.getByRole('dialog', { name: new RegExp(`^Link ${ign}`) }).getByRole('button', { name: `Link ${ign}` }).click();
  await expect(page.getByRole('dialog', { name: new RegExp(`^Link ${ign}`) }).getByRole('status')).toBeVisible();
};
const menuOf = (ign: string) => async (page: Page) => {
  await row(page, ign).getByRole('button', { name: `More actions for ${ign}` }).click();
};
const review = async (page: Page) => {
  await panel(page).getByRole('button', { name: 'Review and link…' }).click();
  await expect(page.locator('.linknotice')).toBeVisible();
};
const unfreshFirst = async (page: Page) => {
  await signInPublic(page, { stale: true });
};
const SHOTS: Shot[] = [
  { file: 'M_Chars' },
  { file: 'M_CharsMenu', act: menuOf('AsahiPath') },
  {
    file: 'M_CharsAdd',
    act: async (page) => {
      await panel(page).getByRole('button', { name: 'Add character' }).click();
      const dialog = page.getByRole('dialog', { name: 'Add character' });
      await dialog.getByLabel('IGN').fill('AsahiLumi');
      await dialog.getByRole('button', { name: 'Look up' }).click();
      await expect(dialog.getByRole('region', { name: 'NEXON lookup' })).toBeVisible();
    },
  },
  { file: 'M_CharEdit', act: lookedUp('AsahiKanna') },
  { file: 'M_LinkNotice', act: review },
  { file: 'M_MyData', act: async (page) => void (await panel(page).getByRole('button', { name: 'My data…' }).click()) },
  { file: 'M_Confirm', setup: unfreshFirst, act: async (page) => {
      await panel(page).getByRole('button', { name: 'Add character' }).click();
      const dialog = page.getByRole('dialog', { name: 'Add character' });
      await dialog.getByLabel('IGN').fill('AsahiLumi');
      await dialog.getByRole('button', { name: 'Add character' }).click();
      await expect(page.getByRole('dialog', { name: "Confirm it's you" })).toBeVisible();
    } },
  { file: 'M_CharStates-empty', setup: async (page) => roster(page, 'empty'), path: '/account?tab=characters' },
  { file: 'M_CharStates-not-connected', setup: async () => setCharacterData('setup'), act: async (page) => void (await panel(page).getByRole('button', { name: 'Add character' }).click()) },
  { file: 'M_CharStates-module-off', setup: async (page) => modules(page, { roster: false }), path: '/account?tab=characters' },
  { file: 'M_CharStates-after-a-write', act: async (page) => {
      await menuOf('AsahiPath')(page);
      await page.getByRole('menuitem', { name: 'Retire' }).click();
      await expect(toast(page).first()).toBeVisible();
    } },
  { file: 'M_CharStates2-pending', act: editOf('AsahiDK') },
  { file: 'M_CharStates2-stale', act: editOf('AsahiPath') },
  { file: 'M_CharStates2-expired', act: editOf('AsahiBlaze') },
  { file: 'M_CharStates2-not-linked-retired', act: editOf('AsahiMech') },
  { file: 'M_CharStates2-not-connected', setup: async () => setCharacterData('setup'), act: editOf('AsahiMech') },
  { file: 'M_LinkOutcomes-linked', act: addAndLink('AsahiLumi') },
  { file: 'M_LinkOutcomes-cancel', act: async (page) => {
      await review(page);
      await page.getByRole('button', { name: 'Cancel' }).click();
    } },
  { file: 'M_LinkOutcomes-busy', act: addAndLink('BusyKnight') },
  { file: 'M_LinkOutcomes-not-found', act: addAndLink('NopeKnight') },
  { file: 'M_LinkOutcomes-later', act: addAndLink('LaterKnight') },
  { file: 'M_EstStates-queued', setup: scenario('queued'), act: lookedUp('AsahiPath') },
  { file: 'M_EstStates-running', setup: scenario('hold'), act: lookedUp('AsahiPath') },
  { file: 'M_EstStates-result', act: lookedUp('AsahiKanna') },
  { file: 'M_EstStates-assumed', setup: scenario('assumed'), act: lookedUp('AsahiKanna') },
  { file: 'M_EstStates2-limited', act: async (page) => {
      await lookedUp('AsahiKanna')(page);
      await page.locator('.estimate').getByRole('button', { name: 'Look up again' }).click();
      await expect(page.getByRole('button', { name: /^Next lookup at/ })).toBeVisible();
    } },
  { file: 'M_EstStates2-failed', setup: scenario('timeout'), act: lookedUp('AsahiKanna') },
  { file: 'M_EstStates2-before-any-lookup', act: editOf('AsahiPath') },
  { file: 'M_RunParty', path: '/?run=r-carling' },
  { file: 'M_RunStates-character-data-off', setup: async (page) => modules(page, { character_data: false }), path: '/?run=r-carling' },
  { file: 'M_RunStates-module-off', setup: async (page) => modules(page, { roster: false }), path: '/?run=r-carling' },
  { file: 'M_PhoneChars', phone: true },
  { file: 'M_PhoneCharsAdd', phone: true, act: async (page) => void (await panel(page).getByRole('button', { name: 'Add character' }).click()) },
  { file: 'M_PhoneCharEdit', phone: true, act: lookedUp('AsahiKanna') },
  { file: 'M_PhoneLinkNotice', phone: true, act: review },
  { file: 'M_PhoneMyData', phone: true, act: async (page) => void (await panel(page).getByRole('button', { name: 'My data…' }).click()) },
  { file: 'M_PhoneConfirm', phone: true, setup: unfreshFirst, act: async (page) => {
      await review(page);
      await page.getByRole('button', { name: 'Link AsahiDK' }).click();
      await expect(page.getByRole('dialog', { name: "Confirm it's you" })).toBeVisible();
    } },
  { file: 'M_PhoneCharStates-empty', phone: true, setup: async (page) => roster(page, 'empty'), path: '/account?tab=characters' },
  { file: 'M_PhoneCharStates-main-menu', phone: true, act: menuOf('AsahiKanna') },
  { file: 'M_PhoneCharStates-module-off', phone: true, setup: async (page) => modules(page, { roster: false }), path: '/account?tab=characters' },
  { file: 'M_PhoneEstStates-queued', phone: true, setup: scenario('queued'), act: lookedUp('AsahiPath') },
  { file: 'M_PhoneEstStates-running', phone: true, setup: scenario('hold'), act: lookedUp('AsahiPath') },
  { file: 'M_PhoneEstStates2-assumed', phone: true, setup: scenario('assumed'), act: lookedUp('AsahiKanna') },
  { file: 'M_PhoneEstStates3-result', phone: true, act: lookedUp('AsahiKanna') },
  { file: 'M_PhoneEstStates3-failed', phone: true, setup: scenario('timeout'), act: lookedUp('AsahiKanna') },
  { file: 'M_PhoneRunParty', phone: true, path: '/?run=r-carling' },
];

test.describe('member characters capture', () => {
  test.skip(!CAPTURES, 'set KANADE_CAPTURE_DIR to write the captures');
  for (const shot of SHOTS)
    test(`capture ${shot.file}`, async ({ page }) => {
      mkdirSync(CAPTURES, { recursive: true });
      await page.setViewportSize(shot.phone ? { width: 390, height: 844 } : { width: 1280, height: 800 });
      await signInPublic(page);
      await roster(page);
      await shot.setup?.(page);
      await page.goto(`${PUBLIC}${shot.path ?? '/account?tab=characters'}${(shot.path ?? '?').includes('?') ? '&' : '?'}sw=off`);
      await page.waitForLoadState('networkidle');
      if (!shot.path) await expect(panel(page).locator('.rosterrow').first()).toBeVisible();
      await shot.act?.(page);
      await settle(page);
      await page.screenshot({ path: join(CAPTURES, `${shot.file}.png`), animations: 'disabled' });
    });
});
