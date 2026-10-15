import { mkdirSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, PUBLIC, csrf, expect, resetWeek, setCharacterData, setEstimates, setModules, settle, test, useLook } from './support';

// Character data (module `character_data`; boards in docs/research/2026-10-12-chardata-mockups)
// over the e2e server's seed: MikaBish (ch-s3) linked by today's 04:01 batch, MikaHero
// linked but retired, Mikadere stale, Hotaru7 expired, YuzuPala and TsubaKain linked, AsahiDK pending, the rest not linked;
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

test('chardata strip: linked shows manual and NEXON side by side, attributed, Refresh waits a day', async ({ page }) => {
  const strip = stripOf(await openEdit(page, 'MikaBish'));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('linked');
  await expect(strip.locator('.chardata__cols > :first-child')).toContainText('Lv. 281Bishop');
  await expect(strip.locator('.chardata__cols > :first-child')).toContainText('manual, updated');
  await expect(strip.locator('.chardata__cols > :nth-child(2)')).toContainText('Lv. 283Bishop·CP 48.2M');
  await expect(strip.locator('.chardata__cols > :nth-child(2)')).toContainText('as of 29 Sep 2026 (SGT)');
  await expect(strip.getByText(ATTRIBUTION, { exact: true })).toBeVisible();
  const refresh = strip.getByRole('button', { name: 'Refresh' });
  await expect(refresh).not.toHaveAttribute('aria-disabled');
  await expect(strip).toContainText('Refreshed 29 Sep 04:01. Refresh asks NEXON now');
  // The real server links the public origin's own /privacy page.
  await expect(strip.getByRole('link', { name: 'Privacy' })).toHaveAttribute('href', `${PUBLIC.replace(/^http:/, 'https:')}/privacy`);
  await expect(strip.locator('.chardata__foot')).toContainText('Linked 21 Sep by Mika (notice 1) · refreshed daily');
  // No "Use these": nothing copies NEXON values into the roster.
  await expect(strip.getByRole('button', { name: /use these/i })).toHaveCount(0);
});

test('chardata strip: Unlink… confirms inline from Cancel and deletes; Link… asks the member', async ({ page }) => {
  const dialog = await openEdit(page, 'MikaBish');
  const strip = stripOf(dialog);
  await strip.getByRole('button', { name: 'Unlink…' }).click();
  const confirm = strip.getByRole('group', { name: 'Unlink MikaBish?' });
  await expect(confirm).toContainText('Deletes the NEXON data kept for MikaBish now (Lv. 283 · CP 48.2M, as of 29 Sep 2026)');
  await expect(confirm).toContainText('No Undo');
  await expect(confirm.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await confirm.getByRole('button', { name: 'Cancel' }).click();
  await expect(strip.getByRole('button', { name: 'Unlink…' })).toBeFocused();
  await strip.getByRole('button', { name: 'Unlink…' }).click();
  await strip.getByRole('button', { name: 'Unlink', exact: true }).click();
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('not linked');
  await expect(strip.getByText(ATTRIBUTION)).toHaveCount(0);
  await expect(strip.getByRole('button', { name: 'Link…' })).toBeFocused();

  await strip.getByRole('button', { name: 'Link…' }).click();
  const ask = strip.getByRole('group', { name: 'Ask Mika to link MikaBish?' });
  await expect(ask).toContainText('Nothing is fetched until Mika confirms with /character link');
  await expect(ask).toContainText('Kanade doesn’t message Mika');
  await expect(ask.getByRole('button', { name: 'Request link' })).toBeFocused();
  await ask.getByRole('button', { name: 'Request link' }).click();
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('pending');
  await expect(strip).toContainText('Waiting for Mika to confirm with /character link');
  await expect(strip).toContainText('The request lapses 13 Oct unless Mika links it first.');
  // Unlink withdraws a pending request.
  await strip.getByRole('button', { name: 'Unlink…' }).click();
  await strip.getByRole('group', { name: 'Withdraw the request for MikaBish?' }).getByRole('button', { name: 'Withdraw' }).click();
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('not linked');
});

test('chardata strip: stale, expired (Refresh tries now) and pending', async ({ page }) => {
  let strip = stripOf(await openEdit(page, 'Mikadere'));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('stale');
  await expect(strip).toContainText('NEXON no longer finds Mikadere under that name');
  await expect(strip.getByText(ATTRIBUTION)).toHaveCount(0);

  strip = stripOf(await openEdit(page, 'AsahiDK', /^Asahi/));
  await expect(strip.locator('.chardata__head .status-chip')).toHaveText('pending');
  await expect(strip).toContainText('Asked 29 Sep 11:40 by Asahi · nothing fetched yet');

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
  await expect(row).toContainText('No lookup in the last 24 h.');
  await row.getByRole('button', { name: 'Look up' }).click();
  const table = row.getByRole('table', { name: 'Clear-time estimate for MikaBish' });
  await expect(table).toBeVisible();
  await expect(table.getByRole('row')).toHaveCount(5);
  await expect(table.getByRole('row').nth(1)).toContainText('Limbo');
  await expect(row).toContainText('(preset 00010) · kept 24 h in memory only');
  await expect(row.getByRole('button', { name: 'Look up again' })).toBeVisible();
});

test('chardata estimate row: running, queued behind it, busy, 422, 503 and failed', async ({ page }) => {
  await estimates(page, 'hold');
  let row = stripOf(await openEdit(page, 'MikaBish')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up' }).click();
  await expect(row.locator('.estimate__progress')).toContainText('Looking up MikaBish…');
  await expect(row.getByRole('button', { name: 'Looking up…' })).toHaveAttribute('aria-disabled', 'true');
  // Another character's lookup is never refused: it waits its turn, with its place. (Results live in
  // the server's memory only, so after a reset MikaNL has none and offers Look up, not Look up again.)
  row = stripOf(await openEdit(page, 'MikaNL')).locator('.estimate');
  await row.getByRole('button', { name: 'Look up', exact: true }).click();
  await expect(row.locator('.estimate__progress')).toContainText('Queued · 1 ahead');
  await expect(row.getByRole('button', { name: 'Queued…' })).toHaveAttribute('aria-disabled', 'true');
  await expect(row.getByRole('status')).toHaveText('Queued · 1 ahead');
  await expect(row.locator('.estimate__error')).toHaveCount(0);

  const cases: [string, RegExp, string, boolean][] = [
    ['busy', /Look up · 0:\d\d$/, 'The scouter is busy.', true],
    ['critical_rate', /Look up again$/, 'Set up critical rate on the scouter first.', false],
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
  await expect(panel.locator('.estimate__head + .chardata__sub')).toContainText('kept 24 h in memory');
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

  await resetWeek();
  await estimates(page, 'off');
  const sheet = await carling(page);
  await expect(sheet.locator('.estimate-panel')).toHaveCount(0);
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

  await setModules({ character_data: false });
  await page.reload();
  await expect(page.getByRole('tabpanel', { name: 'Pings' })).toBeVisible();
  await expect(page.getByRole('tab', { name: /^Character data/ })).toHaveCount(0);
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
    await s.getByRole('button', { name: 'Request link' }).click();
    await shot(`${phone ? 'B_PhoneCharStates2' : 'B_CharStates'}-pending`, s);
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
