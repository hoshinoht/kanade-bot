import { mkdirSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, PUBLIC, ageEstimate, csrf, expect, resetWeek, setCharacterData, setEstimates, setModules, settle, test, useLook } from './support';

// Character data (module `character_data`; boards in docs/research/2026-10-12-chardata-mockups)
// over the e2e server's seed: MikaBish (ch-s3) linked by today's 04:01 batch, MikaHero
// linked but retired, Mikadere stale, Hotaru7 expired, YuzuPala and TsubaKain linked, AsahiDK waiting for Asahi, the rest not linked;
// estimates on, answered by the fake scouter (`setEstimates` picks how). `setModules({character_data})` turns the module
// off, `setCharacterData(health)` drives Config's health through real calls (and `setup`: no key, no scouter).

const ATTRIBUTION = 'Data based on NEXON Open API';
const RUN = 'r-carling';

async function openEdit(page: Page, ign: string, who: RegExp = /^Mika/): Promise<Locator> {
  await page.goto(`${ADMIN}/members?sw=off`);
  await page.getByRole('button', { name: who }).first().click();
  const phone = page.viewportSize()!.width < 600;
  const pane = phone ? page.getByRole('dialog', { name: who }).first() : page.getByRole('complementary', { name: 'Member details' });
  await pane.getByRole('tab', { name: /^Characters/ }).click();
  await pane.getByRole('button', { name: `Edit ${ign}` }).click();
  const dialog = page.getByRole('dialog', { name: `Edit ${ign}` });
  await expect(dialog).toBeVisible();
  return dialog;
}

const stripOf = (dialog: Locator) => dialog.getByRole('region', { name: 'Character data' });
const estimates = (_page: Page, scenario: string, ign?: string) => setEstimates(scenario, ign);
const health = (_page: Page, value: 'ok' | 'alert' | 'refused' | 'setup') => setCharacterData(value);
/** The run's title: the real catalog's Malefic Star token. */
const CARLING = 'HCarling + HMaleficStar';
const MIKA = '100000000000001003';

test('chardata strip: linked shows the roster and NEXON side by side, attributed, Refresh waits a day', async ({ page }) => {
  const strip = stripOf(await openEdit(page, 'MikaBish'));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('linked');
  // The startup pass copied NEXON's level (283) into the roster row.
  await expect(strip.locator('.chardata__cols > :first-child')).toContainText('Lv. 283Bishop');
  await expect(strip.locator('.chardata__cols > :first-child')).toContainText('updated');
  await expect(strip.locator('.chardata__cols > :nth-child(2)')).toContainText('Lv. 283Bishop·CP 48.2M');
  await expect(strip.locator('.chardata__cols > :nth-child(2)')).toContainText('as of 29 Sep 2026 (SGT)');
  await expect(strip.getByText(ATTRIBUTION, { exact: true })).toBeVisible();
  const refresh = strip.getByRole('button', { name: 'Refresh' });
  await expect(refresh).not.toHaveAttribute('aria-disabled');
  await expect(strip).toContainText('Refreshed 29 Sep 04:01. Refresh asks NEXON now');
  // The real server links the public origin's own /privacy page.
  await expect(strip.getByRole('link', { name: 'Privacy' })).toHaveAttribute('href', `${PUBLIC.replace(/^http:/, 'https:')}/privacy`);
  await expect(strip.locator('.chardata__foot')).toContainText('Linked 21 Sep by Mika (notice 2) · refreshed daily');
  // No "Use these": the fetch itself copies NEXON's class and level into the roster.
  await expect(strip.getByRole('button', { name: /use these/i })).toHaveCount(0);
});

test('chardata strip: Unlink… confirms inline from Cancel and deletes; Link… links at once and copies into the roster', async ({ page }) => {
  const dialog = await openEdit(page, 'MikaBish');
  const strip = stripOf(dialog);
  await strip.getByRole('button', { name: 'Unlink…' }).click();
  const confirm = strip.getByRole('group', { name: 'Unlink MikaBish?' });
  await expect(confirm).toContainText('Deletes the NEXON data kept for MikaBish now (Lv. 283 · CP 48.2M, as of 29 Sep 2026)');
  await expect(confirm).toContainText('The roster keeps the class and level copied from NEXON.');
  await expect(confirm).toContainText('No Undo');
  await expect(confirm.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await confirm.getByRole('button', { name: 'Cancel' }).click();
  await expect(strip.getByRole('button', { name: 'Unlink…' })).toBeFocused();
  await strip.getByRole('button', { name: 'Unlink…' }).click();
  await strip.getByRole('button', { name: 'Unlink', exact: true }).click();
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('not linked');
  await expect(strip.getByText(ATTRIBUTION)).toHaveCount(0);
  await expect(strip.getByRole('button', { name: 'Link…' })).toBeFocused();
  await expect(strip).toContainText('Link… links it now and fetches its NEXON data; Mika isn’t asked first.');

  await strip.getByRole('button', { name: 'Link…' }).click();
  const ask = strip.getByRole('group', { name: 'Link MikaBish now?' });
  await expect(ask).toContainText('Kanade fetches its NEXON data now');
  await expect(ask).toContainText('Mika isn’t asked first. Its class and level are copied to the roster and stay after an unlink.');
  await expect(ask.getByRole('button', { name: 'Link', exact: true })).toBeFocused();
  await ask.getByRole('button', { name: 'Link', exact: true }).click();
  // Active at once with the fake NEXON's data (MikaBish: Bishop, Lv. 278), no member confirmation.
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('linked');
  await expect(strip.locator('.chardata__cols > :nth-child(2)')).toContainText('Lv. 278Bishop');
  await expect(strip.getByRole('status').filter({ hasText: 'Linked MikaBish. Its NEXON data is in.' })).toHaveCount(1);
  // Linked by the admin (no "(notice 2)": the member confirmed nothing).
  await expect(strip.locator('.chardata__foot')).toContainText(/Linked 29 Sep by (?!Mika)[^(·]+ · refreshed daily/);
  // The roster row took NEXON's level, by system:nexon.
  const list = await (await page.request.get(`${ADMIN}/api/admin/members/${MIKA}/characters`)).json();
  const row = list.characters.find((c: { ign: string }) => c.ign === 'MikaBish');
  expect([row.class, row.level, row.updated_by]).toEqual(['Bishop', 278, { kind: 'system', id: 'nexon' }]);
});

test('chardata strip: stale, expired (Refresh tries now) and pending', async ({ page }) => {
  let strip = stripOf(await openEdit(page, 'Mikadere'));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('stale');
  await expect(strip).toContainText('NEXON no longer finds Mikadere under that name');
  await expect(strip.getByText(ATTRIBUTION)).toHaveCount(0);

  strip = stripOf(await openEdit(page, 'AsahiDK', /^Asahi/));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('pending');
  await expect(strip).toContainText('Waiting for Asahi to confirm with /character link');
  await expect(strip).toContainText('Since 29 Sep 11:40 · nothing fetched yet');
  await expect(strip).toContainText('Waiting for Asahi to confirm the NEXON notice with /character link; nothing is fetched until then.');

  strip = stripOf(await openEdit(page, 'Hotaru7', /^Hotaru/));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('expired');
  await expect(strip).toContainText('NEXON data expired and was deleted');
  await expect(strip.getByText(ATTRIBUTION)).toHaveCount(0);
  await strip.getByRole('button', { name: 'Refresh' }).click();
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('linked');
  await expect(strip.getByText(ATTRIBUTION, { exact: true })).toBeVisible();
  await expect(strip.getByRole('status').filter({ hasText: 'Refreshed Hotaru7 from NEXON.' })).toHaveCount(1);
});

test('chardata strip: retired (paused) keeps its data until it would expire', async ({ page }) => {
  const strip = stripOf(await openEdit(page, 'MikaHero'));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('refresh paused');
  await expect(strip).toContainText('deleted 19 Oct unless it is brought back');
  await expect(strip.getByText(ATTRIBUTION, { exact: true })).toBeVisible();
  await expect(strip.getByRole('button', { name: 'Refresh' })).toHaveAccessibleDescription(/MikaHero is retired, so its daily refresh is paused/);
});

test('chardata strip: no key reads not connected with Link… off; module off draws nothing and asks nothing', async ({ page }) => {
  const headers = await csrf(page.request);
  expect((await page.request.delete(`${ADMIN}/api/admin/characters/ch-s3/link`, { headers })).ok()).toBe(true);
  await health(page, 'setup');
  let dialog = await openEdit(page, 'MikaBish');
  const strip = stripOf(dialog);
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('not connected');
  await expect(strip).toContainText('Character data isn’t connected yet');
  const link = strip.getByRole('button', { name: 'Link…' });
  await expect(link).toHaveAttribute('aria-disabled', 'true');
  await expect(link).toHaveAccessibleDescription(/An operator sets the key in the environment/);
  // No scouter URL either: no estimate row.
  await expect(strip.locator('.estimate')).toHaveCount(0);

  await setModules({ character_data: false });
  const asked: string[] = [];
  page.on('request', (r) => /\/api\/admin\/(characters\/[^/]+\/(link|refresh)|estimates)/.test(r.url()) && asked.push(r.url()));
  dialog = await openEdit(page, 'MikaBish');
  await expect(dialog.getByRole('switch', { name: 'Retired' })).toBeVisible();
  await expect(stripOf(dialog)).toHaveCount(0);
  // One read finds the module off (the generic 404); nothing more is asked.
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await page.getByRole('button', { name: 'Edit MikaNL' }).click();
  await expect(stripOf(page.getByRole('dialog', { name: 'Edit MikaNL' }))).toHaveCount(0);
  expect(asked.length).toBe(1);
});

test('chardata strip: the phone sheet puts 44 px actions under the values', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const strip = stripOf(await openEdit(page, 'MikaBish'));
  const unlink = strip.getByRole('button', { name: 'Unlink…' });
  await unlink.scrollIntoViewIfNeeded();
  await settle(page);
  const box = (await unlink.boundingBox())!;
  expect(box.height).toBeGreaterThanOrEqual(44);
  const values = (await strip.locator('.chardata__cols > :nth-child(2)').boundingBox())!;
  expect(box.y).toBeGreaterThan(values.y + values.height);
  await expect(strip.getByText(ATTRIBUTION, { exact: true })).toBeVisible();
});

test('chardata estimate row: labelled, runs as a job, then shows catalog bosses', async ({ page }) => {
  const strip = stripOf(await openEdit(page, 'MikaBish'));
  const row = strip.locator('.estimate');
  await expect(row.locator('.estimate__label')).toHaveText('Clear-time estimate');
  await expect(row.locator('.estimate__head + .chardata__sub')).toContainText('For planning only, not eligibility.');
  await expect(row).toContainText('No lookup yet.');
  await row.getByRole('button', { name: 'Look up' }).click();
  const table = row.getByRole('table', { name: 'Clear-time estimate for MikaBish' });
  await expect(table).toBeVisible();
  await expect(table.getByRole('row')).toHaveCount(5);
  await expect(table.getByRole('row').nth(1)).toContainText('Limbo');
  await expect(row.locator('.chardata__foot')).toContainText('(preset 00010) · bosses outside the catalog are left out');
  await expect(row).not.toContainText('in memory');
  await expect(row).not.toContainText('older than');
  await expect(row.getByRole('button', { name: 'Look up again' })).toBeVisible();
});

test('chardata estimate row: running, queued behind it, busy, 422, 503 and failed', async ({ page }) => {
  await estimates(page, 'hold');
  let row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up' }).click();
  await expect(row.locator('.estimate__progress')).toContainText('Looking up MikaBish…');
  await expect(row.getByRole('button', { name: 'Looking up…' })).toHaveAttribute('aria-disabled', 'true');
  // Another character's lookup is never refused: it waits its turn, with its place. (A reset starts
  // with no stored results, so MikaNL has none and offers Look up, not Look up again.)
  row = stripOf(await openEdit(page, 'MikaNL')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up', exact: true }).click();
  await expect(row.locator('.estimate__progress')).toContainText('Queued · 1 ahead');
  await expect(row.getByRole('button', { name: 'Queued…' })).toHaveAttribute('aria-disabled', 'true');
  await expect(row.getByRole('status')).toHaveText('Queued · 1 ahead');
  await expect(row.locator('.estimate__error')).toHaveCount(0);

  const cases: [string, RegExp, string, boolean][] = [
    ['busy', /Look up · 0:\d\d$/, 'The scouter is busy.', true],
    ['critical_rate', /Look up again$/, 'Set up critical rate on the scouter first.', false],
    ['assumption_failed', /Try again$/, 'The estimate couldn’t be completed for this character.', false],
    ['manual_check', /Look up$/, 'Scouter needs a manual check.', true],
    ['timeout', /Try again$/, 'The scouter didn’t answer within 2 minutes (504).', false],
  ];
  for (const [scenario, button, text, off] of cases) {
    await resetWeek();
    await estimates(page, scenario);
    row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
    await row.getByRole('button', { name: 'Look up' }).click();
    await expect(row.locator('.estimate__error'), scenario).toContainText(text);
    const again = row.locator('.chardata__btn');
    await expect(again, scenario).toHaveText(button);
    if (off) await expect(again, scenario).toHaveAttribute('aria-disabled', 'true');
    else await expect(again, scenario).not.toHaveAttribute('aria-disabled');
  }
  await expect(row.getByRole('link', { name: 'Config › Character data' })).toHaveCount(0);

  await resetWeek();
  await estimates(page, 'off');
  const strip = stripOf(await openEdit(page, 'MikaBish'));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('linked');
  await expect(strip.locator('.estimate')).toHaveCount(0);
});

async function carling(page: Page): Promise<Locator> {
  await page.goto(`${ADMIN}/?sw=off`);
  const phone = page.viewportSize()!.width < 900;
  if (!phone) await page.locator('.board').evaluate((board) => (board.scrollLeft = board.scrollWidth));
  await page.locator(`[data-run="${RUN}"] .plan-card__open`).click();
  if (!phone) await page.getByRole('complementary', { name: CARLING }).getByRole('button', { name: 'Open in a larger view' }).click();
  const sheet = page.getByRole('dialog', { name: CARLING });
  await expect(sheet.getByRole('list', { name: 'Party' })).toBeVisible();
  return sheet;
}

test('chardata estimate panel: party lookup with totals, missing members and the labels', async ({ page }) => {
  const sheet = await carling(page);
  const panel = sheet.locator('.estimate-panel');
  await expect(panel.locator('.estimate__label')).toHaveText('Clear-time estimate');
  await expect(panel.locator('.estimate__head + .chardata__sub')).toContainText("For planning only, not eligibility. Members see their own and their run party’s estimates.");
  await expect(panel).toContainText('this run brings (at most 6)');
  // Mika brings MikaBish from the timing; only Ren (2), with no characters, is missing.
  await expect(panel).toContainText('Ren (2) has no character yet and will be listed as missing.');
  await panel.getByRole('button', { name: 'Look up party' }).click();
  await expect(panel.locator('.estimate__progress')).toContainText(/Looking up \d of \d · /);
  // The members table shows from the start and fills in; the total comes last.
  const members = panel.getByRole('table', { name: 'Effective % per member' });
  await expect(members).toBeVisible();
  await expect(panel.locator('.estimate__combined')).toContainText('Karling', { timeout: 20_000 });
  await expect(panel.locator('.estimate__combined')).toContainText('not complete');
  await expect(panel.locator('.estimate__missing')).toHaveText([/Ren \(2\) missing · no characters on the roster/]);
  await expect(panel.locator('p.chardata__foot')).toContainText('Ren (2) has no character (missing, never 0%).');
  await expect(panel.locator('.estimate__head + .chardata__sub')).toContainText(/Looked up \d+ \w{3} \d{2}:\d{2}\.$/);
  await expect(panel.locator('.estimate__head + .chardata__sub')).not.toContainText(/in memory|older than/);
  // Who carries: between the totals and the table, one 100% bar per boss in party order, each part
  // numbered as its table row; the member with no character is counted, never a 0% part.
  const chart = panel.locator('figure.share');
  await expect(chart).toBeVisible();
  expect(await chart.evaluate((el) => el.previousElementSibling?.classList.contains('estimate__combined') && el.nextElementSibling?.tagName === 'TABLE')).toBe(true);
  const bars = chart.getByRole('img');
  await expect(bars).toHaveCount(2);
  await expect(bars.first()).toHaveAttribute('aria-label', /^HCarling shares: .+%; 1 with no estimate$/);
  const rows = await members.locator('tbody tr:not(.estimate__missing)').count();
  for (const bar of await bars.all()) {
    const parts = bar.locator('.share__seg');
    await expect(parts).toHaveCount(rows);
    expect(await parts.locator('.share__n').allTextContents()).toEqual(Array.from({ length: rows }, (_, i) => String(i + 1)));
    const shares = (await parts.evaluateAll((els) => els.map((el) => el.getAttribute('title') ?? ''))).map((t) => Number(/· (\d+)% of/.exec(t)![1]));
    expect(shares.reduce((a, b) => a + b, 0)).toBe(100);
  }
  await expect(chart.locator('.share__note')).toHaveText(['1 with no estimate: Ren (2) (not in the total, never 0%)']);
  // The table's rows carry the same numbers; the missing row a hollow swatch.
  await expect(members.locator('tbody tr:not(.estimate__missing) .share__sw')).toHaveText(Array.from({ length: rows }, (_, i) => String(i + 1)));
  await expect(members.locator('.estimate__missing .share__sw--none')).toHaveCount(1);
});

test('chardata estimates: a result at least estimate_days old still shows, marked "older than n days"', async ({ page }) => {
  // The party looked up 8 days before the pinned Tue 29 Sep 12:00 (guild time): past the default 7.
  await ageEstimate({ run: RUN }, 8);
  const sheet = await carling(page);
  const panel = sheet.locator('.estimate-panel');
  await expect(panel.locator('.estimate__combined')).toContainText('Karling');
  await expect(panel.locator('.estimate__head + .chardata__sub')).toContainText(/Looked up 21 Sep \d{2}:\d{2} · older than 7 days\.$/);
  // Readiness names the same reading's time, marked the same way.
  const yuzu = slotOf(sheet, 'Yuzu').getByRole('group', { name: 'Readiness of YuzuPala' });
  await expect(yuzu.locator('.ready__scouter-at')).toHaveText(/^Estimate · 21 Sep \d{2}:\d{2} · older than 7 days$/);
  await expect(yuzu.locator('.ready__scouter').first()).toHaveText(/^Estimate \d+% · ~\d+ min/);

  // The roster strip: 3 days old is fresh at 7, stale once Config lowers it to 3.
  await ageEstimate({ character: 'ch-s3' }, 3);
  let row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
  await expect(row.locator('.chardata__foot')).toContainText(/^Looked up 26 Sep \d{2}:\d{2}\s+\(preset 00010\) · bosses outside/);
  await expect(row).not.toContainText('older than');
  const lowered = await page.request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(page.request), data: { character_data: { estimate_days: 3 } } });
  expect(lowered.status(), await lowered.text()).toBe(200);
  row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
  await expect(row.locator('.chardata__foot')).toContainText('(preset 00010) · older than 3 days · bosses outside the catalog are left out');
});

test('chardata estimate panel: queued behind another lookup, a member that fails, and off', async ({ page }) => {
  const headers = await csrf(page.request);
  await estimates(page, 'hold');
  expect((await page.request.post(`${ADMIN}/api/admin/estimates`, { headers, data: { character_ids: ['ch-s3'] } })).status()).toBe(202);
  let panel = (await carling(page)).locator('.estimate-panel');
  // Never refused: the party waits its turn, its members listed as queued.
  await panel.getByRole('button', { name: 'Look up party' }).click();
  await expect(panel.locator('.estimate__progress')).toContainText('Queued · 1 ahead');
  await expect(panel.getByRole('button', { name: 'Queued…' })).toHaveAttribute('aria-disabled', 'true');
  await expect(panel.locator('.estimate__pending').first()).toContainText('queued');
  await expect(panel.locator('.estimate__error')).toHaveCount(0);

  await resetWeek();
  await estimates(page, 'failed');
  panel = (await carling(page)).locator('.estimate-panel');
  await panel.getByRole('button', { name: 'Look up party' }).click();
  // The last member fails in their row; the others are found and totalled.
  const failed = panel.locator('.estimate__failed');
  await expect(failed).toHaveCount(1, { timeout: 20_000 });
  await expect(failed).toContainText('failed · Lookup failed.');
  await expect(panel.locator('.estimate__combined')).toContainText('not complete', { timeout: 20_000 });
  await expect(panel.locator('.estimate__pending')).toHaveCount(0);
  await expect(panel.locator('p.chardata__foot')).toContainText('couldn’t be looked up: the totals leave them out.');
  await expect(panel.locator('.estimate__error')).toHaveCount(0);
  await expect(panel.getByRole('button', { name: 'Try again' })).toBeVisible();
  // The failed member keeps their number in the table but has no part, and is counted.
  const failedNumber = (await panel.locator('tbody tr').evaluateAll((rows) => rows.findIndex((row) => row.classList.contains('estimate__failed')))) + 1;
  await expect(failed.locator('.share__sw--none')).toHaveCount(1);
  const parts = panel.locator('figure.share .share__seg .share__n');
  expect(await parts.allTextContents()).not.toContain(String(failedNumber));
  await expect(panel.locator('figure.share .share__note').last()).toHaveText(/^2 with no estimate: .+ and Ren \(2\) \(not in the total, never 0%\)$/);

  await resetWeek();
  await estimates(page, 'off');
  const sheet = await carling(page);
  await expect(sheet.locator('.estimate-panel')).toHaveCount(0);
});

test('chardata estimate panel: a member whose 100% crit assumption failed says so and offers Try again', async ({ page }) => {
  await estimates(page, 'assumption_failed');
  const panel = (await carling(page)).locator('.estimate-panel');
  await panel.getByRole('button', { name: 'Look up party' }).click();
  const failed = panel.locator('.estimate__failed');
  await expect(failed).toHaveCount(1, { timeout: 20_000 });
  await expect(failed).toContainText('failed · The estimate couldn’t be completed for this character.');
  await expect(failed.locator('.estimate__who-note')).toHaveAttribute('title', /^The scouter tried assuming 100% critical rate for \w+ and couldn’t finish\. Try again later\.$/);
  await expect(failed).not.toContainText('Set up critical rate');
  await expect(panel.getByRole('button', { name: 'Try again' })).toBeVisible();
});

test('chardata estimate panel: members fill in as each finishes, and readiness follows each', async ({ page }) => {
  // Each lookup takes longer than the page's poll, so the members come in one at a time.
  await estimates(page, 'paced');
  const sheet = await carling(page);
  const panel = sheet.locator('.estimate-panel');
  await panel.getByRole('button', { name: 'Look up party' }).click();
  const found = panel.locator('tbody tr:not(.estimate__pending):not(.estimate__missing)');
  // One member in, the rest still waiting, the lookup still running.
  await expect(found).toHaveCount(1, { timeout: 10_000 });
  await expect(panel.locator('.estimate__pending').first()).toBeVisible();
  await expect(panel.locator('.estimate__progress')).toBeVisible();
  // No chart and no colours until the lookup is done.
  await expect(panel.locator('figure.share')).toHaveCount(0);
  await expect(panel.locator('.share__sw')).toHaveCount(0);
  // Readiness is read again for that member, before the lookup ends.
  await expect(sheet.locator('.ready__scouter').first()).toBeVisible();
  await expect(panel.locator('.estimate__pending').first()).toBeVisible();
  await expect(panel.locator('.estimate__combined')).toContainText('Karling', { timeout: 20_000 });
  await expect(panel.locator('.estimate__pending')).toHaveCount(0);
});

test('chardata estimate: an assumed critical rate is labelled in the panel, the strip and readiness', async ({ page }) => {
  await estimates(page, 'assumed', 'MikaBish');
  const sheet = await carling(page);
  const panel = sheet.locator('.estimate-panel');
  await panel.getByRole('button', { name: 'Look up party' }).click();
  await expect(panel.locator('.estimate__combined')).toContainText('Karling', { timeout: 20_000 });
  await expect(panel.locator('.estimate__assumed')).toHaveText(['Assumes 100% critical rate (actual 89%)']);
  await expect(panel.locator('p.chardata__foot')).toContainText('at an assumed 100% critical rate.');
  await expect(panel.locator('.estimate__head + .chardata__sub')).toContainText('For planning only, not eligibility.');
  // Who carries hatches and stars MikaBish's parts and names her in its note.
  const hatched = panel.locator('figure.share .share__seg--assumed');
  await expect(hatched).toHaveCount(2);
  await expect(hatched.locator('.share__pct').first()).toHaveText(/^\d+%\*$/);
  await expect(panel.locator('figure.share .share__note').first()).toHaveText('* MikaBish at an assumed 100% critical rate');
  await expect(panel.locator('.estimate__who .share__sw--assumed')).toHaveCount(1);
  const chip = sheet.locator('.ready__scouter', { hasText: 'crit assumed' }).first();
  await expect(chip).toHaveText(/^Estimate \d+%( · ~\d+ min)? · crit assumed/);
  await expect(chip).toHaveAttribute('title', /Assumes 100% critical rate \(actual 89%\)$/);

  const row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up' }).click();
  await expect(row.locator('.estimate__assumed')).toHaveText('Assumes 100% critical rate (actual 89%)');
  await expect(row.getByRole('table', { name: 'Clear-time estimate for MikaBish · Assumes 100% critical rate (actual 89%)' })).toBeVisible();
});

test('chardata estimate row: waiting behind other lookups shows its place, then the result', async ({ page }) => {
  await estimates(page, 'queued');
  const row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up' }).click();
  await expect(row.locator('.estimate__progress')).toContainText('Queued · 2 ahead');
  await expect(row.locator('.estimate__progress')).toContainText('Queued · 1 ahead');
  await expect(row.getByRole('table', { name: 'Clear-time estimate for MikaBish' })).toBeVisible({ timeout: 20_000 });
});

test('chardata estimate panel: the phone Party list ends with it', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const sheet = await carling(page);
  const panel = sheet.getByRole('list', { name: 'Party' }).locator('.estimate-panel');
  await expect(panel).toBeVisible();
  const button = panel.getByRole('button', { name: 'Look up party' });
  await button.scrollIntoViewIfNeeded();
  expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
});

test('chardata health: ok, problems, no key and refused; module off hides the section', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=character-data&sw=off`);
  const panel = page.getByRole('tabpanel', { name: /^Character data/ });
  await expect(panel.getByRole('heading', { name: 'Character data' })).toBeVisible();
  await expect(panel.locator('[aria-labelledby="chardata-nexon-h"]')).toContainText('162 of 800 calls');
  await expect(panel.locator('[aria-labelledby="chardata-nexon-h"]')).toContainText('linked');
  // The configured scouter URL: the e2e server's fake scouter.
  await expect(panel.locator('[aria-labelledby="chardata-scouter-h"]')).toContainText(/http:\/\/127\.0\.0\.1:\d+/);
  await expect(page.getByRole('tab', { name: /^Character data/ })).toContainText('ok');
  // Estimates are on (a scouter URL): the section has its one setting and a save bar.
  await expect(panel.locator('.settings__lead')).toHaveText('Health is informational: it never changes /healthz.');
  await expect(panel.getByRole('form', { name: 'Estimates' })).toBeVisible();
  await expect(page.locator('.savebar')).toContainText('All changes saved');

  await health(page, 'alert');
  await page.reload();
  await expect(panel.locator('[aria-labelledby="chardata-nexon-h"]')).toContainText('backoff until 12:30');
  await expect(panel.locator('[aria-labelledby="chardata-scouter-h"]')).toContainText('verification_required');
  await expect(panel.locator('.chardata-health__post')).toHaveCount(2);
  await expect(panel.locator('.chardata-health__post').first()).toContainText('stuck · #hstar-party · ref 3fa91c');
  await expect(panel.getByRole('link', { name: 'Open in Discord' })).toHaveCount(1);
  await expect(page.getByRole('tab', { name: /^Character data/ })).toContainText('⚠ 2');

  await health(page, 'refused');
  await page.reload();
  await expect(panel.locator('[aria-labelledby="chardata-nexon-h"]')).toContainText('key_refused');
  await expect(panel.locator('[aria-labelledby="chardata-scouter-h"]')).toContainText('unreachable');

  // Last: no key also means no scouter URL (estimates off until the next reset).
  await health(page, 'setup');
  await page.reload();
  await expect(panel.locator('[aria-labelledby="chardata-nexon-h"]')).toContainText('no_key');
  await expect(panel.locator('[aria-labelledby="chardata-nexon-h"]')).toContainText('KANADE_NEXON_API_KEY_FILE');
  await expect(panel.locator('[aria-labelledby="chardata-scouter-h"]')).toContainText('KANADE_SCOUTER_URL is unset');
  await expect(panel.locator('[aria-labelledby="chardata-env-h"]')).toContainText('unset');
  // Estimates off: no setting, no save bar, the read-only lead.
  await expect(panel.getByRole('form', { name: 'Estimates' })).toHaveCount(0);
  await expect(page.locator('.savebar')).toHaveCount(0);
  await expect(panel.locator('.settings__lead')).toHaveText('Read-only and informational: it never changes /healthz.');

  await setModules({ character_data: false });
  await page.reload();
  await expect(page.getByRole('tabpanel', { name: 'Pings' })).toBeVisible();
  await expect(page.getByRole('tab', { name: /^Character data/ })).toHaveCount(0);
});

test('chardata config: Refresh estimates every n days saves with the bar, refuses out of range, and lands in History', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/config?section=character-data&sw=off`);
  const panel = page.getByRole('tabpanel', { name: /^Character data/ });
  const card = panel.getByRole('form', { name: 'Estimates' });
  const field = card.getByRole('spinbutton', { name: 'Refresh estimates every' });
  const bar = page.locator('.savebar');
  await expect(field).toHaveValue('7');
  await expect(card).toContainText('1 to 30 · default 7');
  await expect(card).toContainText('Estimates older than this show “older than 7 days”. Party members of runs in the next 7 days are refreshed automatically.');
  await expect(bar).toContainText('All changes saved');
  await expect(bar.getByRole('button', { name: 'Save character data' })).toBeDisabled();

  // Edited: the ring, "was 7", the helper follows, and the bar names the change; Discard puts it back.
  await field.fill('3');
  await expect(field).toHaveClass(/settings__changed/);
  await expect(card).toContainText('was 7 · 1 to 30');
  await expect(card).toContainText('“older than 3 days”');
  await expect(bar).toContainText('1 unsaved change · Estimate refresh 7 → 3 days');
  await bar.getByRole('button', { name: 'Discard' }).click();
  await expect(field).toHaveValue('7');
  await expect(bar).toContainText('All changes saved');

  // Out of range: the server refuses it (422) inline, and the typed value stays.
  await field.fill('31');
  const refused = page.waitForResponse((r) => r.url().endsWith('/api/admin/config') && r.request().method() === 'PATCH');
  await bar.getByRole('button', { name: 'Save character data' }).click();
  expect((await refused).status()).toBe(422);
  await expect(card.getByRole('alert')).toContainText('1 to 30 days');
  await expect(field).toHaveAttribute('aria-invalid', 'true');
  await expect(field).toHaveValue('31');

  await field.fill('3');
  const saved = page.waitForRequest((r) => r.url().endsWith('/api/admin/config') && r.method() === 'PATCH');
  await bar.getByRole('button', { name: 'Save character data' }).click();
  expect((await saved).postDataJSON()).toEqual({ character_data: { estimate_days: 3 } });
  await expect(page.getByText('Estimate refresh saved.')).toBeVisible();
  await expect(bar).toContainText('All changes saved');
  await expect(field).toHaveValue('3');
  await expect(card.getByRole('alert')).toHaveCount(0);
  await expect(card).toContainText('1 to 30 · default 7');

  // One History row, view-only, with its before and after.
  await page.goto(`${ADMIN}/history?sw=off`);
  const row = page.locator('.history-row--config').filter({ hasText: 'Config · Character data' });
  await expect(row).toHaveCount(1);
  await expect(row).toContainText('Config · Character data — estimate_days');
  await row.click();
  const pane = page.getByRole('complementary', { name: 'Change details' });
  const changed = pane.locator('.history-diff__field');
  await expect(changed.locator('dt')).toHaveText('estimate_days');
  await expect(changed.locator('.history-diff__was')).toHaveText(/^was\s*7$/);
  await expect(changed.locator('.history-diff__now')).toHaveText(/^now\s*3$/);
  await expect(pane.getByRole('link', { name: 'Open Character data in Config' })).toHaveAttribute('href', '/config?section=character-data');
});

const slotOf = (sheet: Locator, name: string) =>
  sheet.locator('.slot').filter({ has: sheet.page().locator('.slot__name', { hasText: new RegExp(`^${name.replace(/[()]/g, '\\$&')}$`) }) });

test('chardata readiness: level and CP per slot, a level verdict per boss, attributed and never blocking', async ({ page }) => {
  const sheet = await carling(page);
  await expect(sheet.locator('.ready__note')).toContainText('It never blocks a pick or a save.');
  await expect(sheet.locator('.ready__note')).toContainText(ATTRIBUTION);
  const yuzu = slotOf(sheet, 'Yuzu').getByRole('group', { name: 'Readiness of YuzuPala' });
  await expect(yuzu).toContainText('Lv. 276 · CP 52.1M');
  await expect(yuzu).toContainText('NEXON · as of 29 Sep');
  // One level verdict per boss of the run (HCarling + HMaleficStar); CP is shown, never judged.
  await expect(yuzu.getByRole('listitem')).toHaveText([/^HCarling\s*Lv ok$/, /^HMaleficStar\s*under Lv\. \d+$/]);
  await expect(sheet.locator('.ready__bosses').filter({ hasText: /floor|CP/i })).toHaveCount(0);
  const tsubame = slotOf(sheet, 'Tsubame').getByRole('group', { name: 'Readiness of TsubaKain' });
  await expect(tsubame.getByRole('listitem')).toHaveText([/^HCarling\s*Lv ok$/, /^HMaleficStar\s*Lv ok$/]);
  const hotaru = slotOf(sheet, 'Hotaru').getByRole('group', { name: 'Readiness of Hotaru7' });
  await expect(hotaru).toContainText('Lv. 275 · CP —');
  await expect(hotaru).toContainText('manual · NEXON data expired');
  await expect(hotaru.getByRole('listitem').first()).toHaveText(/^HCarling\s*Lv ok$/);
  await expect(slotOf(sheet, 'Ren (1)').getByRole('group', { name: 'Readiness of RenShade' })).toContainText('manual · not linked');
  // Mika brings the timing's pick (MikaBish); no one on this run is left without a character.
  await expect(slotOf(sheet, 'Mika').getByRole('group', { name: 'Readiness of MikaBish' })).toContainText('Lv. 283 · CP 48.2M');
  await expect(sheet.getByRole('link', { name: /floor/i })).toHaveCount(0);
  // Never blocking: a pick under an entry level saves as before and the lines follow it.
  await sheet.getByRole('button', { name: /^Mika's character on HCarling/ }).click();
  await sheet.getByRole('dialog', { name: `Mika's character for ${CARLING}` }).getByRole('option', { name: /Mikadere/ }).click();
  await expect(slotOf(sheet, 'Mika').getByRole('group', { name: 'Readiness of Mikadere' }).getByRole('listitem')).toHaveText([/^HCarling\s*under Lv\. \d+$/, /^HMaleficStar\s*under Lv\. \d+$/]);
});

test('chardata readiness: after the run lookup ends, the estimate per boss replaces the slot CP', async ({ page }) => {
  const sheet = await carling(page);
  const yuzu = slotOf(sheet, 'Yuzu').getByRole('group', { name: 'Readiness of YuzuPala' });
  await expect(yuzu).toContainText('Lv. 276 · CP 52.1M');
  await expect(yuzu.locator('.ready__scouter')).toHaveCount(0);
  await expect(sheet.locator('.ready__estimate')).toHaveCount(0);
  await sheet.locator('.estimate-panel').getByRole('button', { name: 'Look up party' }).click();
  // No page reload: readiness is read again when the lookup ends.
  const chip = yuzu.locator('.ready__scouter').first();
  await expect(chip).toHaveText(/^Estimate \d+% · ~\d+ min/, { timeout: 20_000 });
  await expect(chip).toHaveAttribute('title', 'Party-able');
  await expect(yuzu.locator('.ready__values')).toHaveText('Lv. 276');
  await expect(yuzu).not.toContainText('CP 52.1M');
  await expect(yuzu.locator('.ready__scouter-at')).toContainText(/^Estimate · \d+ \w{3} \d{2}:\d{2}$/);
  // Level chips unchanged; the label and note show once for the panel.
  await expect(yuzu.getByRole('listitem').first().locator('.status-chip').first()).toHaveText('Lv ok');
  await expect(sheet.locator('.ready__estimate')).toHaveText(/^Clear-time estimate · For planning only, not eligibility\./);
});

test('chardata readiness: the phone list says it never blocks; module off asks nothing', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  let sheet = await carling(page);
  const party = sheet.getByRole('list', { name: 'Party' });
  await expect(party.locator('.ready__note')).toContainText('Readiness · never blocks');
  await expect(slotOf(sheet, 'Yuzu').getByRole('group', { name: 'Readiness of YuzuPala' })).toContainText('Lv. 276 · CP 52.1M');

  await page.setViewportSize({ width: 1280, height: 800 });
  await setModules({ character_data: false });
  const asked: string[] = [];
  page.on('request', (r) => /\/readiness/.test(r.url()) && asked.push(r.url()));
  sheet = await carling(page);
  await expect(slotOf(sheet, 'Yuzu')).toBeVisible();
  await expect(sheet.locator('.ready, .ready__note, .ready__rules')).toHaveCount(0);
  expect(asked.length).toBeLessThanOrEqual(1);
});

// For user approval (not assertions): every built board's screen at its board size.
const CAPTURES = '/private/tmp/cdw-captures';
test('chardata capture', async ({ page }) => {
  test.skip(!process.env.KANADE_CHARDATA_CAPTURES, 'set KANADE_CHARDATA_CAPTURES=1 to write the review captures');
  test.setTimeout(300_000);
  mkdirSync(CAPTURES, { recursive: true });
  await useLook(page, 'blossom');
  const reset = resetWeek;
  const shot = async (name: string, target?: Locator) => {
    await target?.scrollIntoViewIfNeeded();
    await settle(page);
    await page.screenshot({ path: `${CAPTURES}/${name}.png`, animations: 'disabled' });
  };
  const strip = async (ign: string, who?: RegExp) => stripOf(await openEdit(page, ign, who));
  for (const phone of [false, true]) {
    await page.setViewportSize(phone ? { width: 390, height: 844 } : { width: 1280, height: 800 });
    const P = phone ? 'B_Phone' : 'B_';
    await reset();
    let s = await strip('MikaBish');
    await shot(`${P}CharLinked`, s.locator('.chardata__foot'));
    await shot(`${P}CharEstimate`, s.locator('.estimate'));
    await s.getByRole('button', { name: 'Unlink…' }).click();
    await shot(`${P}CharUnlink`, s.locator('.chardata__confirm'));
    await s.getByRole('button', { name: 'Unlink', exact: true }).click();
    await s.getByRole('button', { name: 'Link…' }).click();
    await shot(`${P}CharLink`, s.locator('.chardata__confirm'));
    await s.getByRole('button', { name: 'Link', exact: true }).click();
    await shot(`${phone ? 'B_PhoneCharStates2' : 'B_CharStates'}-linked-at-once`, s);
    await reset();
    await page.request.delete(`${ADMIN}/api/admin/characters/ch-s3/link`, { headers: await csrf(page.request) });
    s = await strip('MikaBish');
    await shot(`${phone ? 'B_PhoneCharStates2' : 'B_CharStates'}-not-linked`, s);
    await health(page, 'setup');
    s = await strip('MikaBish');
    await shot(`${phone ? 'B_PhoneCharStates' : 'B_CharStates'}-not-connected`, s);
    await setModules({ character_data: false });
    const off = await openEdit(page, 'MikaBish');
    await shot(`${phone ? 'B_PhoneCharStates' : 'B_CharStates'}-off`, off.getByRole('switch', { name: 'Retired' }));
    await reset();
    await shot(`${phone ? 'B_PhoneCharStates3' : 'B_CharStates2'}-stale`, await strip('Mikadere'));
    await shot(`${phone ? 'B_PhoneCharStates4' : 'B_CharStates2'}-expired`, await strip('Hotaru7', /^Hotaru/));
    await shot(`${phone ? 'B_PhoneCharStates4' : 'B_CharStates2'}-retired`, await strip('MikaHero'));
    s = await strip('Hotaru7', /^Hotaru/);
    await page.route('**/api/admin/characters/*/refresh', async (route) => {
      // Held long enough to capture "Refreshing…".
      const { promise, resolve } = Promise.withResolvers<void>();
      setTimeout(resolve, 4_000);
      await promise;
      await route.continue();
    });
    await s.getByRole('button', { name: 'Refresh' }).click();
    await shot(`${phone ? 'B_PhoneCharStates3' : 'B_CharStates2'}-refreshing`, s);
    // Let the held refresh answer before the route goes.
    await expect(s.getByRole('button', { name: 'Refresh', exact: true })).toBeVisible({ timeout: 10_000 });
    await page.unroute('**/api/admin/characters/*/refresh');
    for (const [scenario, name] of [
      ['hold', 'running'],
      ['busy', 'busy'],
      ['critical_rate', '422'],
      ['assumption_failed', '422-assumption'],
      ['manual_check', '503'],
      ['timeout', 'failed'],
    ]) {
      await reset();
      await estimates(page, scenario);
      s = await strip('MikaBish');
      const row = s.locator('.estimate');
      await row.getByRole('button', { name: 'Look up' }).click();
      await expect(row.locator('.estimate__progress, .estimate__error')).toBeVisible();
      await shot(`${P}EstStates-${name}`, row);
    }
    await reset();
    s = await strip('Mikadere');
    await shot(`${P}EstStates-idle`, s.locator('.estimate'));

    // The run sheet's Estimate panel.
    await reset();
    let sheet = await carling(page);
    let panel = sheet.locator('.estimate-panel');
    await shot(`${phone ? 'B_PhoneEstRunStates' : 'B_EstRunStates'}-idle`, panel);
    await panel.getByRole('button', { name: 'Look up party' }).click();
    await expect(panel.getByRole('table', { name: 'Effective % per member' })).toBeVisible({ timeout: 20_000 });
    await shot(`${P}CharEstimateRun`, panel);
    for (const [scenario, name] of [
      ['hold', 'running'],
      ['critical_rate', '422'],
      ['assumption_failed', '422-assumption'],
      ['failed', 'failed'],
    ]) {
      await reset();
      await estimates(page, scenario);
      sheet = await carling(page);
      panel = sheet.locator('.estimate-panel');
      await panel.getByRole('button', { name: 'Look up party' }).click();
      await expect(panel.locator('.estimate__progress, .estimate__table').first()).toBeVisible({ timeout: 20_000 });
      if (scenario !== 'hold') await expect(panel.locator('.estimate__failed')).toBeVisible({ timeout: 20_000 });
      await shot(`${phone ? 'B_PhoneEstRunStates' : 'B_EstRunStates'}-${name}`, panel);
    }
    await reset();
    await estimates(page, 'hold');
    await page.request.post(`${ADMIN}/api/admin/estimates`, { headers: await csrf(page.request), data: { character_ids: ['ch-s3'] } });
    panel = (await carling(page)).locator('.estimate-panel');
    await panel.getByRole('button', { name: 'Look up party' }).click();
    await expect(panel.locator('.estimate__progress')).toContainText('Queued');
    await shot(`${phone ? 'B_PhoneEstRunStates2' : 'B_EstRunStates'}-queued`, panel);

    // Run sheet readiness (the board's sheet has no Estimate panel).
    await reset();
    await estimates(page, 'off');
    // As the board: Mika brings MikaBish.
    await page.request.put(`${ADMIN}/api/admin/runs/${RUN}/characters/${MIKA}`, { headers: await csrf(page.request), data: { character_id: 'ch-s3', version: 0 } });
    sheet = await carling(page);
    await expect(sheet.locator('.ready').first()).toBeVisible();
    await shot(`${P}CharReady`, phone ? sheet.locator('.ready__hint') : undefined);

    // The same readiness once a party lookup is kept: the estimate per boss replaces the CP.
    await reset();
    await page.request.put(`${ADMIN}/api/admin/runs/${RUN}/characters/${MIKA}`, { headers: await csrf(page.request), data: { character_id: 'ch-s3', version: 0 } });
    const started = await page.request.post(`${ADMIN}/api/admin/estimates`, { headers: await csrf(page.request), data: { run_id: RUN } });
    const jobId = (await started.json()).job.id as string;
    await expect.poll(async () => (await (await page.request.get(`${ADMIN}/api/admin/estimates/${jobId}`)).json()).state, { timeout: 20_000 }).toBe('done');
    sheet = await carling(page);
    await expect(sheet.locator('.ready__scouter').first()).toBeVisible();
    await shot(`${P}CharReady-scouter`, phone ? sheet.locator('.ready__hint') : undefined);

    // Config › Character data.
    for (const [value, name] of [
      ['ok', 'CfgCharHealth'],
      ['alert', 'CfgCharHealthAlert'],
      ['setup', 'CfgCharHealthSetup'],
      ['refused', 'CfgCharHealthRefused'],
    ] as const) {
      await reset();
      await health(page, value);
      await page.goto(`${ADMIN}/config?section=character-data&sw=off`);
      await expect(page.locator('[aria-labelledby="chardata-nexon-h"]')).toBeVisible();
      await shot(`${P}${name}`);
    }
  }
});

// For user approval (not assertions): the strip row and the run panel queued, part-way
// (members filling in) and with an assumed critical rate, desktop and phone.
const QUEUE_CAPTURES = '/private/tmp/eq-captures';
test('chardata estimate queue capture', async ({ page }) => {
  test.skip(!process.env.KANADE_CHARDATA_CAPTURES, 'set KANADE_CHARDATA_CAPTURES=1 to write the review captures');
  test.setTimeout(300_000);
  mkdirSync(QUEUE_CAPTURES, { recursive: true });
  const shot = async (name: string, target: Locator) => {
    await target.scrollIntoViewIfNeeded();
    await settle(page);
    await page.screenshot({ path: `${QUEUE_CAPTURES}/${name}.png`, animations: 'disabled' });
  };
  const reset = resetWeek;
  const holdMikaBish = async () => {
    await estimates(page, 'hold');
    await page.request.post(`${ADMIN}/api/admin/estimates`, { headers: await csrf(page.request), data: { character_ids: ['ch-s3'] } });
    await estimates(page, 'ok');
  };
  for (const phone of [false, true]) {
    await page.setViewportSize(phone ? { width: 390, height: 844 } : { width: 1280, height: 800 });
    const P = phone ? 'phone-' : 'desktop-';

    // Strip row: queued behind two, looking up, assumed.
    await reset();
    await estimates(page, 'queued');
    let row = stripOf(await openEdit(page, 'Mikadere')).locator('.estimate');
    await row.getByRole('button', { name: 'Look up' }).click();
    await expect(row.locator('.estimate__progress')).toContainText('Queued · 2 ahead');
    await shot(`${P}strip-queued`, row);
    await reset();
    await estimates(page, 'hold');
    row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
    await row.getByRole('button', { name: 'Look up' }).click();
    await expect(row.locator('.estimate__progress')).toContainText('Looking up MikaBish…');
    await shot(`${P}strip-running`, row);
    await reset();
    await estimates(page, 'assumed', 'MikaBish');
    row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
    await row.getByRole('button', { name: 'Look up' }).click();
    await expect(row.locator('.estimate__assumed')).toBeVisible({ timeout: 20_000 });
    await shot(`${P}strip-assumed`, row);

    // Run panel: queued behind another lookup, members filling in, a failed member, assumed.
    await reset();
    await holdMikaBish();
    let sheet = await carling(page);
    let panel = sheet.locator('.estimate-panel');
    await panel.getByRole('button', { name: 'Look up party' }).click();
    await expect(panel.locator('.estimate__progress')).toContainText('Queued · 1 ahead');
    await shot(`${P}panel-queued`, panel);
    await reset();
    sheet = await carling(page);
    panel = sheet.locator('.estimate-panel');
    await panel.getByRole('button', { name: 'Look up party' }).click();
    await expect(panel.locator('tbody tr:not(.estimate__pending):not(.estimate__missing)')).toHaveCount(2, { timeout: 10_000 });
    await shot(`${P}panel-partial`, panel);
    await reset();
    await estimates(page, 'critical_rate');
    sheet = await carling(page);
    panel = sheet.locator('.estimate-panel');
    await panel.getByRole('button', { name: 'Look up party' }).click();
    await expect(panel.locator('.estimate__combined')).toBeVisible({ timeout: 20_000 });
    await shot(`${P}panel-partial-failed-member`, panel);
    await reset();
    await estimates(page, 'assumed', 'MikaBish');
    sheet = await carling(page);
    panel = sheet.locator('.estimate-panel');
    await panel.getByRole('button', { name: 'Look up party' }).click();
    await expect(panel.locator('.estimate__combined')).toBeVisible({ timeout: 20_000 });
    await shot(`${P}panel-assumed`, panel);
    const chip = sheet.locator('.ready__scouter', { hasText: 'crit assumed' }).first();
    await expect(chip).toBeVisible();
    await shot(`${P}readiness-assumed`, chip);
  }
});
