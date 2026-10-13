import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { EstimateJob, EstimateStarted } from '@kanade/api-types';
import type { Locator, Page } from '@playwright/test';
import { boardExists, composite, diff, markdown, MOCKUPS, renderBoard, settle, skeleton } from './fidelity-kit';
import { ADMIN, expect, PINNED_NOW, PUBLIC, test, choose, csrf, openList, setPortal, signInPublic } from './support';

// Layout fidelity against the M3E boards: `bun run fidelity [pair ...] [--keep]`
// (scripts/fidelity.ts builds with KANADE_FIDELITY=1 so the data-fid tags
// survive, runs this spec, then rebuilds clean). Writes, per pair, under the
// git-ignored e2e/.captures/fidelity/: report.md, report.json, composite.png
// (board left, app right, findings outlined), board.json and app.json.
// It reports; it never fails on findings.
const ENABLED = process.env.KANADE_FIDELITY === '1';
const ONLY = (process.env.KANADE_FIDELITY_ONLY ?? '').split(',').filter(Boolean);
const OUT = join(import.meta.dirname, '.captures', 'fidelity');

interface Pair {
  name: string;
  board: string;
  path: string;
  /** The member portal (the public origin, drawn in Otonose) instead of the admin app. */
  app?: 'public';
  /** Before the first load: sign in, close the portal, hold back a read. */
  setup?: (page: Page) => Promise<void>;
  /** Where the board is a strip of the frame (Mast): open the app at this size, measured against the board's frame. */
  viewport?: { width: number; height: number };
  /** Opens the state the board shows and waits for it. */
  ready: (page: Page) => Promise<void>;
  /** Compare only these regions: the board's subject is part of the frame (the rail), the rest draws another page. */
  scope?: string[];
  /** Left out on both sides with everything inside them: the frame another pair already compares. */
  omit?: string[];
  /** Before the board is measured: take out what the user decided not to build, tag what the board left untagged (`editBoard`). */
  prepare?: (board: Page) => Promise<void>;
}

/**
 * Regions newer boards added: the rail's own (rail option B) and the run sheet's
 * Stage in draft… button (B_RunSheetStage); boards drawn before them leave them untagged.
 */
const LATER_FIDS = ['rail', 'rail-more', 'rail-more-menu', 'drawer-more', 'sheet-stage'];

/**
 * Module `character_data` off (the mock starts with it on): boards drawn before it
 * (the run sheet's Estimate panel and readiness lines) compare against their own screen.
 */
const chardataOff = async (page: Page) => {
  await page.request.post(`${ADMIN}/__mock/modules`, { data: { character_data: false } });
};
const withChardataOff = (setup?: (page: Page) => Promise<void>) => async (page: Page) => {
  await chardataOff(page);
  await setup?.(page);
};

// Member portal pairs (boards: KANADE_MOCKUPS=…/docs/research/2026-10-09-public-portal-mockups).
const signedOut = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
};
const signedIn = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.getByRole('tab', { name: /^Devices/ })).toBeVisible();
};
const devicesShown = async (page: Page) => {
  await signedIn(page);
  await expect(page.getByText('This device')).toBeVisible();
};
/** The member Week with its first read in: the board's cards, or the phone list. */
const weekShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-run]').first()).toBeVisible();
};
/** The first of the member's own runs (`mine`) or someone else's, opened from the Week. */
const openRun = (mine: boolean) => async (page: Page) => {
  await weekShown(page);
  await page.locator(`main .member-card--${mine ? 'mine' : 'other'}`).first().click();
  await expect(page.locator(mine ? 'main [data-fid="week-answer"]' : 'main [data-fid="week-lock"]')).toBeVisible();
};
const listShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-row]').first()).toBeVisible();
};
const myRunsShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="myruns-row"]').first()).toBeVisible();
};
const timingsShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="timings-row"]').first()).toBeVisible();
};
const bossesShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="boss-row"]').first()).toBeVisible();
};
const guideShown = (name: string) => async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.getByRole('heading', { level: 2, name: new RegExp(`^${name}`) })).toBeVisible();
};
/** States: the week's first read held back, so the skeleton shows inside the Week. */
const holdWeek = async (page: Page) => {
  await signInPublic(page);
  await page.route('**/api/public/week*', () => {});
};
const weekLoading = async (page: Page) => {
  await expect(page.locator('main [data-fid="week-board"][aria-busy="true"]')).toBeVisible();
};
/** States-Offline: the week arrived, then the network went: the notice over the last-seen week. */
const weekOffline = async (page: Page) => {
  await weekShown(page);
  await page.context().setOffline(true);
  await expect(page.locator('main [data-fid="week-offline"]')).toBeVisible();
};
/** States-Toasts: two refusals of a refresh, each said once as a toast. */
const weekToasts = async (page: Page) => {
  await weekShown(page);
  await page.route('**/api/public/week*', (route) => route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"internal","message":"The server failed."}' }));
  const refresh = page.getByRole('button', { name: 'Refresh the week' });
  await refresh.click();
  await expect(page.getByRole('group', { name: 'Notification' })).toHaveCount(1);
  await refresh.click();
  await expect(page.getByRole('group', { name: 'Notification' })).toHaveCount(2);
};
/** Every colourway set open, as the boards draw them (only the current one opens by itself). */
const allSets = async (page: Page) => {
  await signedIn(page);
  const sets = page.getByRole('group', { name: 'Colourway' }).locator('button[aria-expanded="false"]');
  while ((await sets.count()) > 0) await sets.first().click();
  // Opening a set scrolls it into view; the boards show the panel from its top.
  await page.getByRole('tabpanel').evaluate((panel) => panel.scrollTo(0, 0));
};
const ended = async (page: Page) => {
  await devicesShown(page);
  await page.request.post(`${PUBLIC}/__mock/public/end`);
  // Reopening the tab re-reads the devices: the 401 ends the screen.
  await page.getByRole('tab', { name: 'Profile' }).click();
  await page.getByRole('tab', { name: /^Devices/ }).click();
  await expect(page.getByRole('heading', { level: 1, name: "You've been signed out" })).toBeVisible();
};
/** A Discord link's slot: Wed 30 Sep 21:30 guild time, the last day of the mock's boss week. */
const MOVE_TO = '2026-09-30T13:30:00Z';
/** Last week's Kalos (the mock's `p-kalos`) at its old Fri 22:00. */
const PAST_MOVE_TO = '2026-09-18T14:00:00Z';
const moveShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="move-result"]')).toBeVisible();
};
const moveNotice = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="move-notice"]')).toBeVisible();
};
/** Ren took Asahi off HCarling + HStar after the link was sent. */
const removedFromCarling = async (page: Page) => {
  await signInPublic(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/remove`, { data: { run: 'r-carling' } })).ok()).toBe(true);
};
/** Signed in longer ago than the fresh window: the next write asks "Confirm it's you". */
const unfresh = async (page: Page) => {
  await signInPublic(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/unfresh`)).ok()).toBe(true);
};
const confirmAnswer = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await page.locator('main [data-fid="week-answer"]').getByRole('button', { name: 'Maybe' }).click();
  await expect(page.getByRole('dialog', { name: "Confirm it's you" })).toBeVisible();
};
/** The board's draft: Leave the member's run, with a note (phones have no summary aside). */
const requestForm = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="request-note"]')).toBeVisible();
  await page.getByLabel('Note for the admins · optional').fill('Something came up on Tuesday evening.');
};
const requestConfirm = async (page: Page) => {
  await requestForm(page);
  await page.getByRole('button', { name: 'Send request' }).click();
  await expect(page.getByRole('dialog', { name: "Confirm it's you" })).toBeVisible();
};
/** Two more requests sent elsewhere fill the three open: the press is refused, the toast says so and the key is off. */
const requestLimit = async (page: Page) => {
  await requestForm(page);
  const session = await page.request.get(`${PUBLIC}/api/public/session`);
  const token = session.headers()['x-kanade-csrf'] ?? '';
  for (const run of ['r-limbo', 'r-fa']) {
    const sent = await page.request.post(`${PUBLIC}/api/public/requests`, {
      headers: { 'X-Kanade-CSRF': token, 'Idempotency-Key': `fid-limit-${run}` },
      data: { kind: 'join', run_id: run },
    });
    expect(sent.status()).toBe(201);
  }
  await page.getByRole('button', { name: 'Send request' }).click();
  await expect(page.locator('main [data-fid="request-limit"]')).toBeVisible();
  await expect(page.getByRole('group', { name: 'Notification' })).toBeVisible();
};
const requestsShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="requests-row"]').first()).toBeVisible();
};
const SIGN_IN = [
  ['', ''],
  ['-Expired', 'state'],
  ['-Failed', 'discord'],
  ['-Limited', 'rate_limited'],
  ['-Unavailable', 'unavailable'],
] as const;
const PUBLIC_PAIRS: Pair[] = [
  ...SIGN_IN.flatMap(([variant, code]) => [
    { name: `pub-signin${variant.toLowerCase()}`, board: `SignIn${variant}`, path: code ? `/?login_error=${code}` : '/', ready: signedOut },
    { name: `pub-phone-gate${variant.toLowerCase()}`, board: `PhoneGate${variant}`, path: code ? `/?login_error=${code}` : '/', ready: signedOut },
  ]),
  ...(['', 'Phone'] as const).map((phone) => ({
    name: phone ? 'pub-phone-closed' : 'pub-closed',
    board: `${phone}Closed`,
    path: '/',
    setup: (page: Page) => setPortal(page.request, false),
    ready: signedOut,
  })),
  { name: 'pub-denied', board: 'Denied', path: '/?login_error=not_eligible', ready: signedOut },
  { name: 'pub-phone-denied', board: 'PhoneDenied', path: '/?login_error=not_eligible', ready: signedOut },
  { name: 'pub-ended', board: 'Ended', path: '/account?tab=devices', setup: signInPublic, ready: ended },
  { name: 'pub-phone-ended', board: 'PhoneEnded', path: '/account?tab=devices', setup: signInPublic, ready: ended },
  { name: 'pub-mast', board: 'Mast', path: '/', setup: signInPublic, viewport: { width: 1280, height: 800 }, ready: weekShown },
  {
    name: 'pub-phone-drawer',
    board: 'PhoneDrawer',
    path: '/',
    setup: signInPublic,
    ready: async (page) => {
      await weekShown(page);
      await page.getByRole('button', { name: 'Open the navigation' }).click();
      const drawer = page.getByRole('dialog', { name: 'Navigation' });
      await expect(drawer).toBeVisible();
      // The panel slides in; measure it where it lands.
      await expect.poll(() => drawer.getByRole('navigation', { name: 'Main' }).evaluate((nav) => nav.getBoundingClientRect().x)).toBeGreaterThanOrEqual(0);
    },
  },
  { name: 'pub-account', board: 'Account', path: '/account', setup: signInPublic, ready: signedIn },
  { name: 'pub-phone-account', board: 'PhoneAccount', path: '/account', setup: signInPublic, ready: signedIn },
  { name: 'pub-account-devices', board: 'Account-Devices', path: '/account?tab=devices', setup: signInPublic, ready: devicesShown },
  { name: 'pub-phone-devices', board: 'PhoneDevices', path: '/account?tab=devices', setup: signInPublic, ready: devicesShown },
  { name: 'pub-account-browser', board: 'Account-Browser', path: '/account?tab=browser', setup: signInPublic, ready: allSets },
  // The phone board opens only the current set (Base), as the app does by itself.
  { name: 'pub-phone-browser', board: 'PhoneBrowser', path: '/account?tab=browser', setup: signInPublic, ready: signedIn },
  // The member Week, its run pane and list, My runs, and the States drawn over the Week.
  { name: 'pub-main', board: 'Main', path: '/', setup: signInPublic, ready: weekShown },
  { name: 'pub-week-run-mine', board: 'Week-RunMine', path: '/', setup: signInPublic, ready: openRun(true) },
  { name: 'pub-week-run-other', board: 'Week-RunOther', path: '/', setup: signInPublic, ready: openRun(false) },
  { name: 'pub-week-list', board: 'WeekList', path: '/?view=list', setup: signInPublic, ready: listShown },
  {
    name: 'pub-week-list-selected',
    board: 'WeekList-Selected',
    path: '/?view=list',
    setup: signInPublic,
    ready: async (page) => {
      await listShown(page);
      await page.locator('main [data-row].week-runs__row--mine').first().click();
      await expect(page.locator('main [data-fid="week-answer"]')).toBeVisible();
    },
  },
  { name: 'pub-myruns', board: 'MyRuns', path: '/mine', setup: signInPublic, ready: myRunsShown },
  { name: 'pub-myruns-next', board: 'MyRuns-Next', path: '/mine?week=next', setup: signInPublic, ready: myRunsShown },
  { name: 'pub-states', board: 'States', path: '/', setup: holdWeek, ready: weekLoading },
  { name: 'pub-states-offline', board: 'States-Offline', path: '/', setup: signInPublic, ready: weekOffline },
  { name: 'pub-states-toasts', board: 'States-Toasts', path: '/', setup: signInPublic, ready: weekToasts },
  { name: 'pub-phone-week', board: 'PhoneWeek', path: '/', setup: signInPublic, ready: weekShown },
  { name: 'pub-phone-run', board: 'PhoneRun', path: '/', setup: signInPublic, ready: openRun(true) },
  { name: 'pub-phone-run-other', board: 'PhoneRunOther', path: '/', setup: signInPublic, ready: openRun(false) },
  { name: 'pub-phone-states', board: 'PhoneStates', path: '/', setup: holdWeek, ready: weekLoading },
  { name: 'pub-phone-states-offline', board: 'PhoneStates-Offline', path: '/', setup: signInPublic, ready: weekOffline },
  { name: 'pub-phone-states-toasts', board: 'PhoneStates-Toasts', path: '/', setup: signInPublic, ready: weekToasts },
  { name: 'pub-phone-myruns', board: 'PhoneMyRuns', path: '/mine', setup: signInPublic, ready: myRunsShown },
  // Frame 1 of each board (the list); Always in, attendance and suggestions have no API yet (expected gaps).
  { name: 'pub-myruns-timings-owner', board: 'MyRuns-Timings-Owner', path: '/mine?week=timings', setup: signInPublic, ready: timingsShown },
  { name: 'pub-phone-myruns-timings-owner', board: 'PhoneMyRuns-Timings-Owner', path: '/mine?week=timings', setup: signInPublic, ready: timingsShown },
  // Bosses and guides (the admin Bosses window, shared): the list, Carling's guide (Hard, Phases, Destiny), Kai (event).
  { name: 'pub-bosses', board: 'Bosses', path: '/bosses', setup: signInPublic, ready: bossesShown },
  { name: 'pub-guide', board: 'Guide', path: '/bosses/Carling', setup: signInPublic, ready: guideShown('Carling') },
  { name: 'pub-guide-phases', board: 'Guide-Phases', path: '/bosses/Carling?tab=phases', setup: signInPublic, ready: guideShown('Carling') },
  { name: 'pub-guide-destiny', board: 'Guide-Destiny', path: '/bosses/Carling?difficulty=Destiny', setup: signInPublic, ready: guideShown('Carling') },
  { name: 'pub-guide-event', board: 'Guide-Event', path: '/bosses/Kai', setup: signInPublic, ready: guideShown('Kai') },
  { name: 'pub-phone-bosses', board: 'PhoneBosses', path: '/bosses', setup: signInPublic, ready: bossesShown },
  { name: 'pub-phone-guide', board: 'PhoneGuide', path: '/bosses/Carling', setup: signInPublic, ready: guideShown('Carling') },
  { name: 'pub-phone-guide-phases', board: 'PhoneGuide-Phases', path: '/bosses/Carling?tab=phases', setup: signInPublic, ready: guideShown('Carling') },
  { name: 'pub-phone-guide-destiny', board: 'PhoneGuide-Destiny', path: '/bosses/Carling?difficulty=Destiny', setup: signInPublic, ready: guideShown('Carling') },
  // Member writes: the run's Move page (a Discord link), its stale shapes, the answer's fresh sign-in, the request form and My requests.
  { name: 'pub-move', board: 'Move', path: `/runs/r-carling?move_to=${MOVE_TO}`, setup: signInPublic, ready: moveShown },
  { name: 'pub-move-notin', board: 'Move-NotIn', path: `/runs/r-carling?move_to=${MOVE_TO}`, setup: removedFromCarling, ready: moveNotice },
  { name: 'pub-move-weekover', board: 'Move-WeekOver', path: `/runs/p-kalos?move_to=${PAST_MOVE_TO}`, setup: signInPublic, ready: moveNotice },
  { name: 'pub-confirm-answer', board: 'ConfirmAnswer', path: '/?run=r-carling', setup: unfresh, ready: confirmAnswer },
  { name: 'pub-request-form', board: 'RequestForm', path: '/requests/new?run=r-carling', setup: signInPublic, ready: requestForm },
  { name: 'pub-request-form-confirm', board: 'RequestForm-Confirm', path: '/requests/new?run=r-carling', setup: unfresh, ready: requestConfirm },
  { name: 'pub-request-form-limit', board: 'RequestForm-Limit', path: '/requests/new?run=r-carling', setup: signInPublic, ready: requestLimit },
  { name: 'pub-requests', board: 'Requests', path: '/requests', setup: signInPublic, ready: requestsShown },
  { name: 'pub-requests-expired', board: 'Requests-Expired', path: '/requests?open=req-carling-swap', setup: signInPublic, ready: requestsShown },
  { name: 'pub-phone-move', board: 'PhoneMove', path: `/runs/r-carling?move_to=${MOVE_TO}`, setup: signInPublic, ready: moveShown },
  { name: 'pub-phone-move-notin', board: 'PhoneMove-NotIn', path: `/runs/r-carling?move_to=${MOVE_TO}`, setup: removedFromCarling, ready: moveNotice },
  { name: 'pub-phone-move-weekover', board: 'PhoneMove-WeekOver', path: `/runs/p-kalos?move_to=${PAST_MOVE_TO}`, setup: signInPublic, ready: moveNotice },
  { name: 'pub-phone-request', board: 'PhoneRequest', path: '/requests/new?run=r-carling', setup: signInPublic, ready: requestForm },
  { name: 'pub-phone-requests', board: 'PhoneRequests', path: '/requests?open=req-kalos-weekly', setup: signInPublic, ready: requestsShown },
  // `/privacy` (boards P_Privacy, P_PhonePrivacy: KANADE_MOCKUPS=…/docs/research/2026-10-12-chardata-mockups).
  { name: 'pub-privacy', board: 'P_Privacy', path: '/privacy', ready: signedOut },
  { name: 'pub-phone-privacy', board: 'P_PhonePrivacy', path: '/privacy', ready: signedOut },
].map((pair) => ({ ...pair, app: 'public' as const }));

// Member characters (boards: KANADE_MOCKUPS=…/docs/research/2026-10-13-member-characters, with a
// `render` link to the M3E mockups' renderer). Asahi's characters as the boards draw them
// (`/__mock/public/characters {"roster": "board"}`). The states boards (M_CharStates*,
// M_LinkOutcomes, M_EstStates*, M_RunStates, M_PhoneCharStates, M_PhoneEstStates*) are grids of
// cells over several screens: e2e/public-characters.spec.ts covers each of their states instead.
const boardRoster = async (page: Page) => {
  await signInPublic(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/characters`, { data: { roster: 'board' } })).status()).toBe(204);
};
/** M_CharsMenu draws no waiting request: an admin withdraws Ren's first. */
const boardRosterNoRequest = async (page: Page) => {
  await boardRoster(page);
  expect((await page.request.delete(`${ADMIN}/api/admin/characters/ch-b2/link`, { headers: await csrf(page.request) })).ok()).toBe(true);
};
const boardRosterUnfresh = async (page: Page) => {
  await boardRoster(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/unfresh`)).ok()).toBe(true);
};
/** The board draws the party's cached estimates: an admin looks the run's party up first (each read advances the mock's job). */
const boardRosterWithReadings = async (page: Page) => {
  await boardRoster(page);
  const headers = await csrf(page.request);
  const started: EstimateStarted = await (await page.request.post(`${ADMIN}/api/admin/estimates`, { headers, data: { run_id: 'r-carling' } })).json();
  await expect
    .poll(
      async () => {
        const job: EstimateJob = await (await page.request.get(`${ADMIN}/api/admin/estimates/${started.job.id}`)).json();
        return job.state;
      },
      { timeout: 30_000 },
    )
    .toBe('done');
};
const charsShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('[data-fid="mychars-panel"] .rosterrow').first()).toBeVisible();
};
const charsMenu = async (page: Page) => {
  await charsShown(page);
  await page.getByRole('button', { name: 'More actions for AsahiPath' }).click();
  await expect(page.getByRole('menu', { name: 'AsahiPath' })).toBeVisible();
};
const charsAdd = async (page: Page) => {
  await charsShown(page);
  await page.getByRole('button', { name: 'Add character' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add character' });
  await dialog.getByLabel('IGN').fill('AsahiLumi');
  await dialog.getByRole('button', { name: 'Look up' }).click();
  await expect(dialog.getByRole('region', { name: 'NEXON lookup' })).toContainText('found');
};
/** Edit AsahiKanna with a done lookup, scrolled to the strip as the board leaves it. */
const charEdit = async (page: Page) => {
  await charsShown(page);
  await page.getByRole('button', { name: 'Edit AsahiKanna' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit AsahiKanna' });
  await dialog.locator('.estimate').getByRole('button', { name: /^Look up/ }).click();
  await expect(dialog.locator('.estimate table')).toBeVisible({ timeout: 20_000 });
  await dialog.locator('.chardata').evaluate((strip) => {
    const body = strip.closest('.modal__body')!;
    body.scrollTop = strip.getBoundingClientRect().top - body.getBoundingClientRect().top + body.scrollTop - 8;
  });
};
const linkNotice = async (page: Page) => {
  await charsShown(page);
  await page.getByRole('button', { name: 'Review and link…' }).click();
  await expect(page.locator('[data-fid="link-notice"]')).toBeVisible();
};
const myData = async (page: Page) => {
  await charsShown(page);
  await page.getByRole('button', { name: 'My data…' }).click();
  await expect(page.locator('[data-fid="mydata-entry"]').first()).toBeVisible();
};
const confirmAdd = async (page: Page) => {
  await charsShown(page);
  await page.getByRole('button', { name: 'Add character' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add character' });
  await dialog.getByLabel('IGN').fill('AsahiLumi');
  await dialog.getByRole('button', { name: 'Add character' }).click();
  await expect(page.getByRole('dialog', { name: "Confirm it's you" })).toBeVisible();
  // The board draws Confirm alone: the refused Add has closed (nothing was added).
  await expect(dialog).toBeHidden();
};
const confirmLink = async (page: Page) => {
  await linkNotice(page);
  await page.getByRole('button', { name: 'Link AsahiDK' }).click();
  await expect(page.getByRole('dialog', { name: "Confirm it's you" })).toBeVisible();
};
/** The own run's pane (or phone screen), scrolled to Party · characters. */
const runParty = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  const section = page.locator('[data-fid="week-party-chars"]');
  await expect(section.locator('.party-char__line, .party-char__none').first()).toBeVisible();
  await section.evaluate((node) => {
    for (let box = node.parentElement; box; box = box.parentElement)
      if (box.scrollHeight > box.clientHeight && getComputedStyle(box).overflowY !== 'visible') {
        box.scrollTop = node.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop - 8;
        break;
      }
  });
};
const CHARS = '/account?tab=characters';
const MEMBER_CHAR_PAIRS: Pair[] = [
  { name: 'mc-chars', board: 'M_Chars', path: CHARS, setup: boardRoster, ready: charsShown },
  { name: 'mc-chars-menu', board: 'M_CharsMenu', path: CHARS, setup: boardRosterNoRequest, ready: charsMenu },
  { name: 'mc-chars-add', board: 'M_CharsAdd', path: CHARS, setup: boardRoster, ready: charsAdd },
  { name: 'mc-char-edit', board: 'M_CharEdit', path: CHARS, setup: boardRoster, ready: charEdit },
  { name: 'mc-link-notice', board: 'M_LinkNotice', path: CHARS, setup: boardRoster, ready: linkNotice },
  { name: 'mc-mydata', board: 'M_MyData', path: CHARS, setup: boardRoster, ready: myData },
  { name: 'mc-confirm', board: 'M_Confirm', path: CHARS, setup: boardRosterUnfresh, ready: confirmAdd },
  { name: 'mc-run-party', board: 'M_RunParty', path: '/?run=r-carling', setup: boardRosterWithReadings, ready: runParty },
  { name: 'mc-phone-chars', board: 'M_PhoneChars', path: CHARS, setup: boardRoster, ready: charsShown },
  { name: 'mc-phone-chars-add', board: 'M_PhoneCharsAdd', path: CHARS, setup: boardRoster, ready: charsAdd },
  { name: 'mc-phone-char-edit', board: 'M_PhoneCharEdit', path: CHARS, setup: boardRoster, ready: charEdit },
  { name: 'mc-phone-link-notice', board: 'M_PhoneLinkNotice', path: CHARS, setup: boardRoster, ready: linkNotice },
  { name: 'mc-phone-mydata', board: 'M_PhoneMyData', path: CHARS, setup: boardRoster, ready: myData },
  { name: 'mc-phone-confirm', board: 'M_PhoneConfirm', path: CHARS, setup: boardRosterUnfresh, ready: confirmLink },
  { name: 'mc-phone-run-party', board: 'M_PhoneRunParty', path: '/?run=r-carling', setup: boardRosterWithReadings, ready: runParty },
].map((pair) => ({ ...pair, app: 'public' as const }));

// The character roster (module `roster`, on in the mock): Mika's sheet on its Characters tab.
// The Members frame around the sheet is the `members` pair's (B_Members); these pairs
// compare the sheet, the tab and the dialogs (desktop: the page line, the roster bar
// and list; phone: also the top bar, which phone-nav compares, and the roster window).
const ROSTER_FRAME = ['page-line', 'window-bar', 'members-list'];
const PHONE_ROSTER_FRAME = ['topbar', 'page-line', 'window'];
// The roster boards' "External data · Later · off" strip is replaced by the character data
// strip (chardata-* pairs): the board loses it before it is measured, the app runs with module
// `character_data` off (its dialog then ends at Retired), and a dialog the board centres stays centred.
// The board's Region and Class dropdowns gain the shared Select's `dd-trigger` tag here,
// as the boards live outside the worktree.
const editBoard = (centre?: string) => (page: Page) =>
  page.evaluate((centre) => {
    for (const fid of ['roster-external', 'roster-estimate', 'roster-external-attrib']) document.querySelector(`[data-fid="${fid}"]`)?.remove();
    for (const dd of document.querySelectorAll<HTMLElement>('[data-fid="roster-world"] .dd, [data-fid="roster-class"] .dd')) dd.dataset.fid = 'dd-trigger';
    const dialog = centre ? document.querySelector<HTMLElement>(`[data-fid="${centre}"]`) : null;
    if (dialog) dialog.style.top = `${(innerHeight - dialog.offsetHeight) / 2}px`;
  }, centre);

// Drafts and the request editor (KANADE_MOCKUPS=docs/research/2026-10-11-drafts-mockups).
// The rail is the rail-more pairs'; the shared DayStrip/TimeStepper keep their own
// move-picker tags, which these boards do not draw (they tag the strip's box).
// The phone pairs leave the top bar (phone-nav's) and the screen-reader page line out too.
const DRAFTS_OMIT = ['rail', 'topbar', 'move-days', 'move-day', 'move-time'];
const PHONE_DRAFTS_OMIT = [...DRAFTS_OMIT, 'page-line'];
/** The boards' drafts hold six changes; the mock seeds two (Weekend reshuffle) and three (Carling retime). */
const moreOps = (id: string, version: number) => async (page: Page) => {
  const headers = await csrf(page.request);
  for (const op of [
    { kind: 'party', run: 'r-bm', add: ['1004'], remove: ['1008'] },
    { kind: 'status', run: 'r-bm', status: 'confirmed' },
    { kind: 'move', run: 'r-fa', day: 6, time: '23:00' },
    { kind: 'status', run: 'r-bellona', status: 'cancelled' },
  ].slice(0, id === 'd-weekend' ? 4 : 3)) {
    const done = await page.request.post(`${ADMIN}/api/admin/drafts/${id}/ops`, { headers, data: { version, op } });
    version = (await done.json()).version;
  }
};
/**
 * B_DraftsConflict's data on Carling retime: the open draft heads the list (the board's
 * conflicted draft is its first row), and the upstream cancellation flags changes 3 and 4,
 * as the board's does, rather than the seeded 2 and 3 (change 2 is a clean one on the board).
 */
const conflictLikeBoard = async (page: Page) => {
  await moreOps('d-carling', 4)(page);
  await page.route('**/api/admin/drafts?state=open', async (route) => {
    const list = await (await route.fetch()).json();
    route.fulfill({ json: { ...list, drafts: [...list.drafts].sort((a: { id: string }, b: { id: string }) => Number(b.id === 'd-carling') - Number(a.id === 'd-carling')) } });
  });
  await page.route('**/api/admin/drafts/d-carling/preview', async (route) => {
    const preview = await (await route.fetch()).json();
    for (const c of preview.conflicts) if (c.kind === 'upstream_removed') c.ops = [2, 3];
    route.fulfill({ json: preview });
  });
};
/** The boards list three open drafts: the mock's two plus "Bellona clean-up" with one change. */
const thirdDraft = async (page: Page) => {
  const headers = await csrf(page.request);
  const made = await (await page.request.post(`${ADMIN}/api/admin/drafts`, { headers, data: { title: 'Bellona clean-up' } })).json();
  await page.request.post(`${ADMIN}/api/admin/drafts/${made.id}/ops`, { headers, data: { version: made.version, op: { kind: 'status', run: 'r-bellona', status: 'cancelled' } } });
};
const draftShown = (title: string) => async (page: Page) => {
  await expect(page.getByRole('heading', { level: 2, name: title })).toBeVisible();
  await page.waitForLoadState('networkidle');
};
const draftsList = async (page: Page) => {
  await expect(page.getByRole('listbox', { name: 'Open drafts' })).toBeVisible();
  await page.waitForLoadState('networkidle');
};
/** No open drafts; the closed tab gains the board's third, expired row. */
const noOpenDrafts = async (page: Page) => {
  await page.route('**/api/admin/drafts?state=open', (route) => route.fulfill({ json: { head: 9, drafts: [] } }));
  await page.route('**/api/admin/drafts?state=closed', async (route) => {
    const list = await (await route.fetch()).json();
    route.fulfill({ json: { ...list, drafts: [...list.drafts, { ...list.drafts[0], id: 'd-old', title: 'Bellona clean-up', status: 'expired', merged_seq: null }] } });
  });
};
const holdDrafts = (page: Page) => page.route('**/api/admin/drafts?state=open', () => {});
const failDrafts = (page: Page) =>
  page.route('**/api/admin/drafts?state=open', (route) =>
    route.fulfill({ status: 503, json: { error: 'unavailable', message: 'The server didn’t answer (timeout after 10 s).' } }),
  );
/** The drafts error boards give LoadError's Copy details an icon; the approved B_States (states-error), which the shared LoadError follows, draws none. */
const errorBoard = (page: Page) =>
  page.evaluate(() => {
    for (const icon of document.querySelectorAll('[data-fid="state-actions"] .btn svg')) icon.remove();
  });
/** The board's edit: change 1 a step earlier (Sat 22:00), change 2 dropped. */
const requestEdited = async (page: Page) => {
  await expect(page.getByRole('heading', { level: 2, name: /Weekly timing change/ })).toBeVisible();
  await page.getByRole('button', { name: /Edit before approving/ }).click();
  await page.getByRole('button', { name: /minutes earlier$/ }).click();
  await page.getByRole('button', { name: 'Drop change 2: the party change' }).click();
  await page.getByRole('radio', { name: 'Keep as it is' }).check();
  // Checking the radio scrolls the phone's thread; the board draws it from the top.
  await page.evaluate(() => {
    for (const el of document.querySelectorAll('*')) if (el.scrollTop) el.scrollTop = 0;
  });
};
/** Tsubame changes the request under the edit: Approve edited is refused. */
const requestRefused = async (page: Page) => {
  await requestEdited(page);
  await page.request.post(`${ADMIN}/__mock/inbox/kalos-v4`);
  await page.getByRole('button', { name: 'Approve edited' }).click();
  await expect(page.locator('[data-fid="inbox-edit-toast"]')).toBeVisible();
  await page.locator('.toast').evaluate((el) => Promise.all(el.getAnimations().map((a) => a.finished)));
};
/**
 * The request editor boards redraw the approved Inbox frame around the detail. Left out:
 * the list (inbox-self's and phone-inbox's subject, with their known row-height gap), the
 * phone's hidden list bar (phone-inbox's child-order gap), and the refusal toast with the
 * shared toast region it lives in (the app has one toast system; the board draws the
 * toast inside the window).
 */
const INBOX_EDIT_OMIT = [...DRAFTS_OMIT, 'inbox-list', 'inbox-edit-toast', 'toasts'];
const PHONE_INBOX_EDIT_OMIT = [...PHONE_DRAFTS_OMIT, 'inbox-list', 'window-bar', 'inbox-edit-toast', 'toasts'];
/**
 * The board's window is positioned only to hold its toast (left out above). On the laptop
 * boards the title names the boss by token on one unwrapping line ("XKalos"); the approved
 * Inbox boards and the app name it in full, and the app's title wraps whole boss tags (name
 * beside its pill: inbox-self's known flex-wrap gap). The board's title gets the full name,
 * drawn as such a tag. The phone boards already break before the boss, as the app does: left
 * as drawn; they tighten the approved bubble (`.bub2`, 12px 16px in B_PhoneInbox) to 10px 14px,
 * which the app's Inbox frame keeps.
 */
const inboxEditBoard = (page: Page) =>
  page.evaluate(() => {
    for (const el of document.querySelectorAll<HTMLElement>('[data-fid="window"]')) el.style.position = 'static';
    if (innerWidth < 600) {
      for (const el of document.querySelectorAll<HTMLElement>('[data-fid="inbox-quote"]')) el.style.padding = '';
      return;
    }
    for (const title of document.querySelectorAll<HTMLElement>('[data-fid="inbox-title"]')) {
      title.style.flexWrap = 'wrap';
      const h = title.querySelector('h2')!;
      const pill = title.querySelector('.pill')!;
      h.textContent = h.textContent!.replace(' XKalos', '');
      const tag = document.createElement('span');
      tag.style.cssText = 'display: flex; align-items: center; gap: 8px';
      const name = h.cloneNode() as HTMLElement;
      name.textContent = 'Gatekeeper Kalos';
      tag.append(name, pill);
      title.append(tag);
    }
  });
/**
 * The stage boards redraw the whole run sheet; the sheet itself is HeroSheet's and
 * HeroPhone's (the hero-sheet and hero-phone pairs), so these compare what staging
 * adds: the stage button, the picker, the strip, the staged clock line and party slot.
 */
const STAGE_OMIT = ['rail', 'topbar', 'page-line', 'window-bar', 'move-days', 'move-day', 'move-time'];
const STAGE_SCOPE = ['sheet-stage', 'sheet-stage-picker', 'sheet-stage-start', 'sheet-staging', 'sheet-stage-stop', 'sheet-staged-when', 'sheet-party-staged'];
/**
 * The stage boards redraw HeroSheet's frame their own way; before measuring what staging
 * adds, the board gets the frame as the approved B_HeroSheet draws it (hero-sheet compares
 * it): the sheet spans the 1280 px frame under its 48 px title bar (the bar outside the
 * sheet region), the actions column spreads Move, the actions and the status over the
 * hero's height (`space-between`), the party grid is 930 px wide beside the panel's side
 * column and fills the panel, and the clock column carries hero-sheet's known 8 px top gap.
 * On the phone, the picker is the shared Modal (a fixed dialog) where the board positions
 * it absolutely in its frame.
 */
const stageBoard = (page: Page) =>
  page.evaluate(() => {
    for (const el of document.querySelectorAll<HTMLElement>('[data-fid="sheet-party"]')) el.dataset.fid = 'week-party';
    if (innerWidth < 600) {
      for (const el of document.querySelectorAll<HTMLElement>('[data-fid="sheet-stage-picker"]')) el.style.position = 'fixed';
      return;
    }
    const sheet = document.querySelector<HTMLElement>('[data-fid="sheet"]')!;
    Object.assign(sheet.style, { left: '0px', top: '48px', width: '1280px', height: '752px' });
    const bar = sheet.querySelector<HTMLElement>('[data-fid="window-bar"]')!;
    Object.assign(bar.style, { position: 'absolute', top: '-48px', left: '0px', right: '0px' });
    for (const el of document.querySelectorAll<HTMLElement>('[data-fid="sheet-actions"]')) el.style.justifyContent = 'space-between';
    for (const el of document.querySelectorAll<HTMLElement>('[data-fid="week-party"]')) Object.assign(el.style, { width: '930px', height: '100%' });
    // hero-sheet's known gap: the app's clock column starts 8 px down (board 0).
    for (const el of document.querySelectorAll<HTMLElement>('[data-fid="sheet-when"]')) el.style.paddingTop = '8px';
  });
const stageApp = (page: Page) =>
  page.evaluate(() => {
    // HeroSheet's window around the tabs and panel: the stage boards draw the tabs and party straight in the sheet.
    for (const el of document.querySelectorAll<HTMLElement>('[data-fid="sheet-window"]')) delete el.dataset.fid;
    // The phone board tags the action row (Move, More actions, the stage button) and no sheet around the hero.
    if (innerWidth < 600) {
      for (const el of document.querySelectorAll<HTMLElement>('[data-fid="sheet"]')) delete el.dataset.fid;
      for (const el of document.querySelectorAll<HTMLElement>('[data-fid="week-move"]')) el.dataset.fid = 'sheet-actions';
    }
  });
/** HCarling + HStar in the full sheet (the pane's larger view on a laptop, the sheet on a phone). */
const carlingSheet = async (page: Page) => {
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const pane = page.getByRole('complementary', { name: 'HCarling + HStar' });
  if (page.viewportSize()!.width >= 900) {
    await expect(pane).toBeVisible();
    await pane.getByRole('button', { name: 'Open in a larger view' }).click();
  }
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await expect(sheet).toBeVisible();
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  return sheet;
};
const DRAFT_PAIRS: Pair[] = [
  { name: 'drafts', board: 'B_Drafts', path: '/drafts?item=d-weekend', omit: DRAFTS_OMIT, setup: moreOps('d-weekend', 3), ready: draftShown('Weekend reshuffle') },
  { name: 'drafts-conflict', board: 'B_DraftsConflict', path: '/drafts?item=d-carling', omit: DRAFTS_OMIT, setup: conflictLikeBoard, ready: draftShown('Carling retime') },
  {
    name: 'drafts-empty',
    board: 'B_DraftsEmpty',
    path: '/drafts',
    omit: DRAFTS_OMIT,
    setup: noOpenDrafts,
    ready: async (page) => {
      await expect(page.getByRole('heading', { name: 'No open drafts' })).toBeVisible();
      await expect(page.locator('[data-fid="empty-recent-row"]').first()).toBeVisible();
    },
  },
  { name: 'drafts-loading', board: 'B_DraftsLoading', path: '/drafts', omit: DRAFTS_OMIT, setup: holdDrafts, ready: (page) => expect(page.getByText('Loading drafts…')).toBeVisible() },
  { name: 'drafts-error', board: 'B_DraftsError', path: '/drafts', omit: DRAFTS_OMIT, setup: failDrafts, prepare: errorBoard, ready: (page) => expect(page.getByRole('button', { name: 'Try again' })).toBeVisible() },
  { name: 'phone-drafts', board: 'B_PhoneDrafts', path: '/drafts', omit: PHONE_DRAFTS_OMIT, setup: thirdDraft, ready: draftsList },
  { name: 'phone-draft-conflict', board: 'B_PhoneDraftConflict', path: '/drafts?item=d-carling', omit: PHONE_DRAFTS_OMIT, setup: conflictLikeBoard, ready: draftShown('Carling retime') },
  {
    name: 'phone-drafts-empty',
    board: 'B_PhoneDraftsEmpty',
    path: '/drafts',
    omit: PHONE_DRAFTS_OMIT,
    setup: noOpenDrafts,
    ready: async (page) => {
      await expect(page.getByRole('heading', { name: 'No open drafts' })).toBeVisible();
      await expect(page.locator('[data-fid="empty-recent-row"]').first()).toBeVisible();
    },
  },
  { name: 'phone-drafts-loading', board: 'B_PhoneDraftsLoading', path: '/drafts', omit: PHONE_DRAFTS_OMIT, setup: holdDrafts, ready: (page) => expect(page.getByText('Loading drafts…')).toBeVisible() },
  { name: 'phone-drafts-error', board: 'B_PhoneDraftsError', path: '/drafts', omit: PHONE_DRAFTS_OMIT, setup: failDrafts, prepare: errorBoard, ready: (page) => expect(page.getByRole('button', { name: 'Try again' })).toBeVisible() },
  { name: 'inbox-edit', board: 'B_InboxEdit', path: '/inbox?tab=self_service&item=p-kalos-fixed', omit: INBOX_EDIT_OMIT, prepare: inboxEditBoard, ready: requestEdited },
  { name: 'inbox-edit-conflict', board: 'B_InboxEditConflict', path: '/inbox?tab=self_service&item=p-kalos-fixed', omit: INBOX_EDIT_OMIT, prepare: inboxEditBoard, ready: requestRefused },
  { name: 'phone-inbox-edit', board: 'B_PhoneInboxEdit', path: '/inbox?tab=self_service&item=p-kalos-fixed', omit: PHONE_INBOX_EDIT_OMIT, prepare: inboxEditBoard, ready: requestEdited },
  { name: 'phone-inbox-edit-conflict', board: 'B_PhoneInboxEditConflict', path: '/inbox?tab=self_service&item=p-kalos-fixed', omit: PHONE_INBOX_EDIT_OMIT, prepare: inboxEditBoard, ready: requestRefused },
  {
    name: 'run-stage',
    board: 'B_RunSheetStage',
    path: '/',
    omit: STAGE_OMIT,
    scope: STAGE_SCOPE,
    prepare: stageBoard,
    // The board's picker lists three drafts; the mock seeds two.
    setup: withChardataOff(thirdDraft),
    ready: async (page) => {
      const sheet = await carlingSheet(page);
      await sheet.getByRole('button', { name: 'Stage in draft…' }).click();
      await expect(sheet.getByRole('dialog', { name: 'Stage in draft' }).getByRole('radio', { name: /Weekend reshuffle/ })).toBeChecked();
      await stageApp(page);
    },
  },
  {
    name: 'run-stage-active',
    board: 'B_RunSheetStageActive',
    path: '/',
    omit: STAGE_OMIT,
    scope: STAGE_SCOPE,
    prepare: stageBoard,
    // The board's draft already moves this run and adds Hinata to it; its party has six
    // (the mock adds a second Ren, #1013, this week).
    setup: async (page) => {
      await chardataOff(page);
      const headers = await csrf(page.request);
      let version = 3;
      for (const op of [
        { kind: 'move', run: 'r-carling', day: 6, time: '21:30' },
        { kind: 'party', run: 'r-carling', add: ['1006'] },
      ]) {
        const done = await page.request.post(`${ADMIN}/api/admin/drafts/d-weekend/ops`, { headers, data: { version, op } });
        version = (await done.json()).version;
      }
      await page.route('**/api/admin/week*', async (route) => {
        const response = await route.fetch();
        if (response.status() !== 200) return route.fulfill({ response });
        const week = await response.json();
        for (const run of week.runs) if (run.id === 'r-carling') run.participants = run.participants.filter((p: { id: string }) => p.id !== '1013');
        return route.fulfill({ json: week });
      });
    },
    ready: async (page) => {
      const sheet = await carlingSheet(page);
      await sheet.getByRole('button', { name: 'Stage in draft…' }).click();
      await sheet.getByRole('button', { name: 'Start staging' }).click();
      await expect(sheet.getByText('Staging into Weekend reshuffle')).toBeVisible();
      await expect(sheet.locator('[data-fid="sheet-party-staged"]')).toBeVisible();
      await stageApp(page);
    },
  },
  {
    name: 'run-stage-phone',
    board: 'B_RunSheetStagePhone',
    path: '/',
    omit: STAGE_OMIT,
    scope: STAGE_SCOPE,
    prepare: stageBoard,
    setup: withChardataOff(thirdDraft),
    ready: async (page) => {
      const sheet = await carlingSheet(page);
      await sheet.getByRole('button', { name: 'Stage in draft…' }).click();
      const picker = page.getByRole('dialog', { name: 'Stage in draft' });
      await expect(picker).toBeVisible();
      await picker.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
      await stageApp(page);
    },
  },
];

// Presence (KANADE_MOCKUPS=docs/research/2026-10-12-graph-presence-mockups). `POST
// /__mock/presence/board` seeds the boards' other admins: Ren editing p-kalos-fixed with
// Kaito viewing it 40 s ago, and Ren editing HCarling + HStar. The Inbox boards redraw
// B_InboxEdit (the inbox-edit pairs compare it) and the run sheet boards B_RunSheetStage
// (the stage pairs' frame): these compare what presence adds. B_PresenceDraft and
// B_PhonePresenceDraft have no pair: drafts are their author's only, so Draft detail
// shows no presence (the boards draw another admin's view of the draft).
const PRESENCE_SCOPE = ['presence', 'presence-pop', 'presence-note'];
const presenceBoard = async (page: Page) => {
  await page.request.post(`${ADMIN}/__mock/presence/board`);
};
/**
 * B_PresenceInbox draws the popover as a top-level overlay placed for its own window,
 * which inboxEditBoard then moves (the window goes static): the board's popover goes
 * under its presence button, 10 px below as drawn, where the app anchors it.
 */
const presenceInboxBoard = async (page: Page) => {
  await inboxEditBoard(page);
  await page.evaluate(() => {
    const pop = document.querySelector<HTMLElement>('[data-fid="presence-pop"]');
    const host = document.querySelector<HTMLElement>('[data-fid="presence"]')?.parentElement;
    if (!pop || !host) return;
    host.style.position = 'relative';
    host.append(pop);
    Object.assign(pop.style, { left: '0px', top: 'calc(100% + 10px)' });
  });
};
/** The laptop sheet's title bar: the board tags it `window-bar`; the app's is the Modal head. */
const SHEET_BAR_OMIT = STAGE_OMIT.filter((fid) => fid !== 'window-bar');
const sheetBar = (page: Page) =>
  page.evaluate(() => {
    // The Week window's own bar (behind the sheet) stays out, as STAGE_OMIT leaves it.
    for (const el of document.querySelectorAll<HTMLElement>('[data-fid="window-bar"]')) delete el.dataset.fid;
    for (const el of document.querySelectorAll<HTMLElement>('dialog.runsheet .modal__head')) el.dataset.fid = 'window-bar';
  });
const PRESENCE_PAIRS: Pair[] = [
  {
    name: 'presence-inbox',
    board: 'B_PresenceInbox',
    path: '/inbox?tab=self_service&item=p-kalos-fixed',
    omit: INBOX_EDIT_OMIT,
    scope: PRESENCE_SCOPE,
    prepare: presenceInboxBoard,
    setup: presenceBoard,
    ready: async (page) => {
      await requestEdited(page);
      await page.getByRole('button', { name: /^Other admins here/ }).click();
      await expect(page.getByRole('dialog', { name: 'Admins here' })).toBeVisible();
    },
  },
  { name: 'phone-presence-inbox', board: 'B_PhonePresenceInbox', path: '/inbox?tab=self_service&item=p-kalos-fixed', omit: PHONE_INBOX_EDIT_OMIT, scope: PRESENCE_SCOPE, prepare: inboxEditBoard, setup: presenceBoard, ready: requestEdited },
  {
    name: 'presence-run-sheet',
    board: 'B_PresenceRunSheet',
    path: '/',
    omit: SHEET_BAR_OMIT,
    scope: [...PRESENCE_SCOPE, 'window-bar'],
    prepare: stageBoard,
    setup: withChardataOff(presenceBoard),
    ready: async (page) => {
      const sheet = await carlingSheet(page);
      await expect(sheet.locator('.presence-note')).toBeVisible();
      await stageApp(page);
      await sheetBar(page);
    },
  },
  {
    name: 'phone-presence-run-sheet',
    board: 'B_PhonePresenceRunSheet',
    path: '/',
    omit: STAGE_OMIT,
    scope: PRESENCE_SCOPE,
    prepare: stageBoard,
    setup: withChardataOff(presenceBoard),
    ready: async (page) => {
      const sheet = await carlingSheet(page);
      await expect(sheet.locator('.presence__btn')).toBeVisible();
      await stageApp(page);
    },
  },
];

// History graph (KANADE_MOCKUPS=docs/research/2026-10-12-graph-presence-mockups, with the
// shared render/ beside the boards). `POST /__mock/graph/board` seeds the boards' History
// (head #39) and drafts. The rail is the rail-more pairs'; the phone top bar phone-nav's.
const GRAPH_OMIT = ['rail', 'topbar'];
const PHONE_GRAPH_OMIT = [...GRAPH_OMIT, 'page-line'];
const graphBoard = async (page: Page) => {
  await page.request.post(`${ADMIN}/__mock/graph/board`);
};
const graphShown = async (page: Page) => {
  await expect(page.getByRole('grid', { name: /^History graph/ })).toBeVisible();
  await page.waitForLoadState('networkidle');
};
const graphList = async (page: Page) => {
  await graphShown(page);
  if (page.viewportSize()!.width < 600) await page.keyboard.press('Escape');
  await page.getByRole('group', { name: 'Show as' }).getByRole('button', { name: 'List' }).click();
  await page.locator('.graph__scroll').evaluate((el) => (el.scrollTop = 0));
};
const graphLong = async (page: Page) => {
  await graphShown(page);
  await page.getByRole('button', { name: 'Go to #…' }).click();
  await page.getByRole('textbox', { name: 'Change number' }).fill('18');
  await page.getByRole('button', { name: 'Go', exact: true }).click();
  await expect(page.getByText('Showing #18 – #28')).toBeVisible();
  await page.locator('.graph__scroll').evaluate((el) => (el.scrollTop = 0));
};
const graphEmpty = (page: Page) => page.route('**/api/admin/drafts/graph?*', (route) => route.fulfill({ json: { head: 9, drafts: [] } }));
const GRAPH_PAIRS: Pair[] = [
  {
    name: 'history-graph',
    board: 'B_HistoryGraph',
    path: '/history?tab=graph&seq=38',
    omit: GRAPH_OMIT,
    setup: graphBoard,
    ready: async (page) => {
      await graphShown(page);
      await expect(page.getByRole('complementary', { name: 'Change details' })).toBeVisible();
      await page.locator('tr[data-row="r38"]').focus();
    },
  },
  {
    name: 'history-graph-draft',
    board: 'B_HistoryGraphDraft',
    path: '/history?tab=graph&draft=d-weekend',
    omit: GRAPH_OMIT,
    setup: graphBoard,
    ready: async (page) => {
      await graphShown(page);
      await expect(page.getByText('Conflicts · 2')).toBeVisible();
    },
  },
  { name: 'history-graph-list', board: 'B_HistoryGraphList', path: '/history?tab=graph&seq=38', omit: GRAPH_OMIT, setup: graphBoard, ready: graphList },
  { name: 'history-graph-long', board: 'B_HistoryGraphLong', path: '/history?tab=graph', omit: GRAPH_OMIT, setup: graphBoard, ready: graphLong },
  {
    name: 'history-graph-empty',
    board: 'B_HistoryGraphEmpty',
    path: '/history?tab=graph',
    omit: GRAPH_OMIT,
    setup: graphEmpty,
    ready: (page) => expect(page.getByRole('heading', { name: 'No drafts to draw yet' })).toBeVisible(),
  },
  { name: 'phone-history-graph', board: 'B_PhoneHistoryGraph', path: '/history?tab=graph', omit: PHONE_GRAPH_OMIT, setup: graphBoard, ready: graphShown },
  {
    name: 'phone-history-graph-draft',
    board: 'B_PhoneHistoryGraphDraft',
    path: '/history?tab=graph&draft=d-weekend',
    omit: PHONE_GRAPH_OMIT,
    setup: graphBoard,
    ready: async (page) => {
      await expect(page.getByRole('dialog', { name: 'Weekend reshuffle' })).toContainText('Conflicts · 2');
      await settle(page);
    },
  },
  { name: 'phone-history-graph-list', board: 'B_PhoneHistoryGraphList', path: '/history?tab=graph', omit: PHONE_GRAPH_OMIT, setup: graphBoard, ready: graphList },
  { name: 'phone-history-graph-long', board: 'B_PhoneHistoryGraphLong', path: '/history?tab=graph', omit: PHONE_GRAPH_OMIT, setup: graphBoard, ready: graphLong },
  {
    name: 'phone-history-graph-empty',
    board: 'B_PhoneHistoryGraphEmpty',
    path: '/history?tab=graph',
    omit: PHONE_GRAPH_OMIT,
    setup: graphEmpty,
    ready: (page) => expect(page.getByRole('heading', { name: 'No drafts to draw yet' })).toBeVisible(),
  },
];

const rosterOff = async (page: Page) => {
  await page.request.post(`${ADMIN}/__mock/modules`, { data: { roster: false } });
};
const rosterTab = async (page: Page, role: 'complementary' | 'dialog', name: string) => {
  await page.getByRole('button', { name: /^Mika/ }).click();
  const sheet = page.getByRole(role, { name, exact: true });
  await sheet.getByRole('tab', { name: /^Characters/ }).click();
  await expect(sheet.getByRole('list', { name: "Mika's characters" })).toBeVisible();
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  return sheet;
};
const rosterDelete = async (page: Page, role: 'complementary' | 'dialog', name: string) => {
  const sheet = await rosterTab(page, role, name);
  await sheet.getByRole('button', { name: 'More actions for MikaBish' }).click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Delete MikaBish?' });
  await expect(confirm.getByRole('button', { name: 'Cancel' })).toBeFocused();
};
/**
 * The run-pick boards redraw the run sheet; these pairs compare what the roster adds to its
 * Party tab. The boards' "Later slice" tags and Mikadere's disabled level reason are not
 * built (user, 2026-10-12: any active character, no level filter): the board loses the tags
 * and the reason before it is measured, and its picker is the shared Modal on the phone. The
 * rules box keeps its tag as the header line the app's "Characters" caption stands in.
 */
const RUN_PICK_SCOPE = ['party-slot', 'party-char', 'party-pick', 'party-warn', 'party-rules', 'roster-picker', 'roster-picker-option'];
const runPickBoard = (page: Page) =>
  page.evaluate(() => {
    document.querySelector('[data-fid="party-later"]')?.remove();
    for (const tag of document.querySelectorAll('.ltag')) if (!tag.closest('[data-fid="party-rules"]')) tag.remove();
    for (const off of document.querySelectorAll<HTMLElement>('[data-fid="roster-picker"] .opt.no')) {
      off.classList.remove('no');
      off.removeAttribute('aria-disabled');
      off.querySelector<HTMLElement>('.cmeta')!.textContent = 'Lara';
    }
    if (innerWidth < 600) for (const el of document.querySelectorAll<HTMLElement>('[data-fid="roster-picker"]')) el.style.position = 'fixed';
  });
/** The boards' Ren (1) picked RenShade (no "inferred"), as on the other run it clashes with. */
const renPicked = async (page: Page) => {
  const headers = await csrf(page.request);
  for (const run of ['r-carling', 'r-nstar'])
    await page.request.put(`${ADMIN}/api/admin/runs/${run}/characters/1002`, { headers, data: { character_id: 'ch-s7', version: 0 } });
};

// Character data (KANADE_MOCKUPS=docs/research/2026-10-12-chardata-mockups). The strip boards
// splice their dialog into B_RosterEdit's Members window (the roster pairs' frame, left out the
// same way); the run-sheet boards redraw B_RosterRunPick's sheet, so those pairs compare what
// the Estimate panel adds; the Config boards are the whole Settings window. The States boards
// (B_CharStates*, B_EstStates*, B_EstRunStates*) are grids of cells over several screens, not
// one screen: e2e/chardata.spec.ts covers each of their states instead.
const scouterOff = async (page: Page) => {
  await page.request.post(`${ADMIN}/__mock/estimates`, { data: { scenario: 'off' } });
};
const chardataHealth = (health: string) => async (page: Page) => {
  await page.request.post(`${ADMIN}/__mock/character-data`, { data: { health } });
};
/** A finished MapleScouter lookup (the mock steps a running job once per read). */
const lookedUp = (target: { character_ids: string[] } | { run_id: string }) => async (page: Page) => {
  const headers = await csrf(page.request);
  const { job } = await (await page.request.post(`${ADMIN}/api/admin/estimates`, { headers, data: target })).json();
  for (let read = 0; read < 10; read++) if ((await (await page.request.get(`${ADMIN}/api/admin/estimates/${job.id}`)).json()).state !== 'running') break;
};
/** The boards' MikaBish unlinked first (B_CharLink asks to link it). */
const unlinked = async (page: Page) => {
  await page.request.delete(`${ADMIN}/api/admin/characters/ch-s3/link`, { headers: await csrf(page.request) });
};
/** As the roster pairs: the board's dropdowns gain the Select's tag, and a desktop dialog is centred as the app centres it. */
const stripBoard = (centre: boolean) => (page: Page) =>
  page.evaluate((centre) => {
    for (const dd of document.querySelectorAll<HTMLElement>('[data-fid="roster-world"] .dd, [data-fid="roster-class"] .dd')) dd.dataset.fid = 'dd-trigger';
    const dialog = centre ? document.querySelector<HTMLElement>('[data-fid="roster-dialog"]') : null;
    if (dialog) dialog.style.top = `${(innerHeight - dialog.offsetHeight) / 2}px`;
  }, centre);
/**
 * MikaBish's Edit dialog, `act` done on its strip, scrolled as the board scrolls it: the board
 * shifts its IGN field up by `scroll` px from where it sits under its head (66 px into the
 * desktop dialog, 72 px into the phone sheet), so the app's body scrolls until its IGN field
 * sits at that same offset in the dialog (its own head is taller).
 */
const stripOpen =
  (role: 'complementary' | 'dialog', name: string, scroll: number, act?: (strip: Locator) => Promise<void>) =>
  async (page: Page) => {
    const sheet = await rosterTab(page, role, name);
    await sheet.getByRole('button', { name: 'Edit MikaBish' }).click();
    const dialog = page.getByRole('dialog', { name: 'Edit MikaBish' });
    const strip = dialog.locator('[data-fid="roster-external"]');
    await expect(strip.locator('[data-fid="chardata-state"]')).toBeVisible();
    await act?.(strip);
    await dialog.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
    const base = (page.viewportSize()!.width < 600 ? 72 : 66) - scroll;
    await dialog.locator('[data-fid="roster-ign"]').evaluate((el, want) => {
      let box = el.parentElement;
      while (box && box.scrollHeight <= box.clientHeight) box = box.parentElement;
      const top = el.getBoundingClientRect().top - el.closest('[data-fid="roster-dialog"]')!.getBoundingClientRect().top;
      if (box) box.scrollTop += top - want;
    }, base);
  };
// As RUN_PICK_SCOPE: the party list's own box (a scrolling list on the boards' phones, the
// sheet's panel scrolls in the app) is the hero and roster-run-pick pairs'.
const ESTIMATE_RUN_SCOPE = [
  'party-slot',
  'sheet-aside',
  'estimate-panel',
  'estimate-label',
  'estimate-disclaimer',
  'estimate-run',
  'estimate-combined',
  'estimate-members',
  'estimate-missing',
  'estimate-issues',
];
const estimateRun = async (page: Page) => {
  const sheet = await carlingSheet(page);
  const panel = sheet.locator('[data-fid="estimate-panel"]');
  await expect(panel.locator('[data-fid="estimate-members"]')).toBeVisible();
  // The phone board shows the Party list scrolled to the panel at its end.
  if (page.viewportSize()!.width < 600) await panel.evaluate((el) => el.scrollIntoView({ block: 'end' }));
};
const CHARDATA_HEALTH = [
  ['', 'ok'],
  ['Alert', 'alert'],
  ['Setup', 'setup'],
  ['Refused', 'refused'],
] as const;
const healthShown = (page: Page) => expect(page.locator('[data-fid="chardata-nexon-health"]')).toBeVisible();
const PHONE_CFG_SCOPE = ['cfg-title', 'cfg-savebar', 'cfg-save'];
const READY_SCOPE = [
  'party-slot',
  'party-char',
  'party-pick',
  'party-warn',
  'sheet-aside',
  'party-ready',
  'party-ready-note',
  'party-ready-attrib',
  'party-ready-rules',
];
const readyBoard = async (page: Page) => {
  await scouterOff(page);
  await renPicked(page);
  const headers = await csrf(page.request);
  await page.request.put(`${ADMIN}/api/admin/runs/r-carling/characters/1003`, { headers, data: { character_id: 'ch-s3', version: 0 } });
};
const readyShownOn = async (page: Page) => {
  const sheet = await carlingSheet(page);
  await expect(sheet.locator('[data-fid="party-ready"]').first()).toBeVisible();
  // The phone board shows the Party list scrolled to its end (the rules line).
  if (page.viewportSize()!.width < 600) await sheet.locator('.ready__hint').evaluate((el) => el.scrollIntoView({ block: 'end' }));
};

const PAIRS: Pair[] = [
  {
    name: 'inbox-self',
    board: 'B_InboxSelf',
    path: '/inbox?tab=self_service&item=p-fa-request',
    ready: (page) => expect(page.getByText('Changed since the member asked')).toBeVisible(),
  },
  {
    name: 'history',
    board: 'B_History',
    path: '/history',
    ready: async (page) => {
      await page.locator('[data-history]').first().click();
      await expect(page.getByRole('complementary', { name: 'Change details' })).toBeVisible();
    },
  },
  {
    name: 'history-ck',
    board: 'B_HistoryCk',
    path: '/history',
    ready: async (page) => {
      await page.getByRole('tab', { name: 'Checkpoints' }).click();
      await expect(page.getByRole('table', { name: /Backups/ })).toBeVisible();
    },
  },
  {
    name: 'fixed',
    board: 'B_Fixed',
    path: '/fixed',
    ready: async (page) => {
      await page.getByRole('button', { name: /^Edit / }).first().click();
      await expect(page.getByRole('complementary', { name: 'Weekly timing details' })).toBeVisible();
    },
  },
  {
    name: 'bosses',
    board: 'B_Bosses',
    path: '/bosses/MaleficStar/knowledge',
    ready: (page) => expect(page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' })).toBeVisible(),
  },
  {
    name: 'phone-inbox',
    board: 'B_PhoneInbox',
    path: '/inbox?tab=extractor&item=p-bm-move',
    ready: (page) => expect(page.getByRole('heading', { level: 2, name: /Move — Black Mage/ })).toBeVisible(),
  },
  {
    name: 'members',
    board: 'B_Members',
    path: '/members',
    // B_Members predates the roster module; B_RosterOff: with it off the sheet is exactly this one.
    setup: rosterOff,
    ready: async (page) => {
      await page.getByRole('button', { name: /^Mika/ }).click();
      await expect(page.getByRole('complementary', { name: 'Member details' })).toBeVisible();
    },
  },
  // The character roster (KANADE_MOCKUPS=docs/research/2026-10-11-roster-mockups):
  // Mika's Characters tab with MikaBish's menu, the Edit dialog with the IGN
  // taken and the level out of range, the phone's Add sheet, and Delete. The
  // States and Off boards are notes over several screens (Off is B_Members, above).
  {
    name: 'roster-chars',
    board: 'B_RosterChars',
    path: '/members',
    omit: ROSTER_FRAME,
    ready: async (page) => {
      const pane = await rosterTab(page, 'complementary', 'Member details');
      await pane.getByRole('button', { name: 'More actions for MikaBish' }).click();
      await expect(pane.getByRole('menu', { name: 'MikaBish' })).toBeVisible();
    },
  },
  {
    name: 'phone-roster-chars',
    board: 'B_PhoneRosterChars',
    path: '/members',
    omit: PHONE_ROSTER_FRAME,
    ready: async (page) => {
      await rosterTab(page, 'dialog', 'Mika');
    },
  },
  {
    name: 'roster-edit',
    board: 'B_RosterEdit',
    path: '/members',
    omit: ROSTER_FRAME,
    setup: async (page) => {
      await chardataOff(page);
    },
    prepare: editBoard('roster-dialog'),
    ready: async (page) => {
      const pane = await rosterTab(page, 'complementary', 'Member details');
      await pane.getByRole('button', { name: 'Edit MikaBish' }).click();
      const dialog = page.getByRole('dialog', { name: 'Edit MikaBish' });
      await dialog.getByRole('textbox', { name: 'IGN' }).fill('hotaru7');
      await dialog.getByRole('button', { name: 'Save' }).click();
      await expect(dialog.getByText('already on the roster in MapleSEA')).toBeVisible();
      await dialog.getByRole('textbox', { name: 'Level' }).fill('312');
      await dialog.getByRole('button', { name: 'Save' }).click();
      await expect(dialog.getByText('Fix 2 fields to save:')).toBeVisible();
    },
  },
  {
    name: 'phone-roster-edit',
    board: 'B_PhoneRosterEdit',
    path: '/members',
    omit: PHONE_ROSTER_FRAME,
    // The board draws the manual Class and Level (no NEXON lookup): module `character_data` off.
    setup: async (page) => {
      await chardataOff(page);
    },
    prepare: editBoard(),
    ready: async (page) => {
      const sheet = await rosterTab(page, 'dialog', 'Mika');
      await sheet.getByRole('button', { name: 'Add character' }).click();
      const add = page.getByRole('dialog', { name: 'Add character' });
      await add.getByRole('textbox', { name: 'IGN' }).fill('MikaKain');
      await choose(add.getByRole('combobox', { name: 'Class' }), { label: 'Kain' });
      await add.getByRole('textbox', { name: 'Level' }).fill('0');
      await add.getByRole('button', { name: 'Add character' }).click();
      await expect(add.getByText('Level is 1 to 300.')).toBeVisible();
    },
  },
  {
    name: 'roster-delete',
    board: 'B_RosterDelete',
    path: '/members',
    omit: ROSTER_FRAME,
    ready: async (page) => {
      await rosterDelete(page, 'complementary', 'Member details');
    },
  },
  {
    name: 'phone-roster-delete',
    board: 'B_PhoneRosterDelete',
    path: '/members',
    omit: PHONE_ROSTER_FRAME,
    ready: async (page) => {
      await rosterDelete(page, 'dialog', 'Mika');
    },
  },
  // The run-sheet character picker (B_RosterRunPick, B_PhoneRosterRunPick): HCarling + HStar
  // with Mika's picker open. The sheet around the party is hero-sheet's and hero-phone's.
  {
    name: 'roster-run-pick',
    board: 'B_RosterRunPick',
    path: '/',
    omit: STAGE_OMIT,
    scope: RUN_PICK_SCOPE,
    setup: withChardataOff(renPicked),
    prepare: runPickBoard,
    ready: async (page) => {
      const sheet = await carlingSheet(page);
      await sheet.getByRole('button', { name: /^Pick Mika's character/ }).click();
      await expect(sheet.getByRole('dialog', { name: "Mika's character for HCarling + HStar" })).toBeVisible();
    },
  },
  {
    name: 'phone-roster-run-pick',
    board: 'B_PhoneRosterRunPick',
    path: '/',
    omit: STAGE_OMIT,
    scope: RUN_PICK_SCOPE,
    setup: withChardataOff(renPicked),
    prepare: runPickBoard,
    ready: async (page) => {
      const sheet = await carlingSheet(page);
      await sheet.getByRole('button', { name: /^Pick Mika's character/ }).click();
      const picker = page.getByRole('dialog', { name: "Mika's character" });
      await expect(picker).toBeVisible();
      await picker.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
    },
  },
  // Character data (KANADE_MOCKUPS=docs/research/2026-10-12-chardata-mockups): the strip,
  // linked, asking to link, confirming an unlink, and with the estimate row.
  {
    name: 'chardata-linked',
    board: 'B_CharLinked',
    path: '/members',
    omit: ROSTER_FRAME,
    setup: scouterOff,
    prepare: stripBoard(true),
    ready: stripOpen('complementary', 'Member details', 0),
  },
  {
    name: 'phone-chardata-linked',
    board: 'B_PhoneCharLinked',
    path: '/members',
    omit: PHONE_ROSTER_FRAME,
    setup: scouterOff,
    prepare: stripBoard(false),
    ready: stripOpen('dialog', 'Mika', 150),
  },
  {
    name: 'chardata-link',
    board: 'B_CharLink',
    path: '/members',
    omit: ROSTER_FRAME,
    setup: async (page) => {
      await scouterOff(page);
      await unlinked(page);
    },
    prepare: stripBoard(true),
    ready: stripOpen('complementary', 'Member details', 150, (strip) => strip.getByRole('button', { name: 'Link…' }).click()),
  },
  {
    name: 'phone-chardata-link',
    board: 'B_PhoneCharLink',
    path: '/members',
    omit: PHONE_ROSTER_FRAME,
    setup: async (page) => {
      await scouterOff(page);
      await unlinked(page);
    },
    prepare: stripBoard(false),
    ready: stripOpen('dialog', 'Mika', 150, (strip) => strip.getByRole('button', { name: 'Link…' }).click()),
  },
  {
    name: 'chardata-unlink',
    board: 'B_CharUnlink',
    path: '/members',
    omit: ROSTER_FRAME,
    setup: scouterOff,
    prepare: stripBoard(true),
    ready: stripOpen('complementary', 'Member details', 230, (strip) => strip.getByRole('button', { name: 'Unlink…' }).click()),
  },
  {
    name: 'phone-chardata-unlink',
    board: 'B_PhoneCharUnlink',
    path: '/members',
    omit: PHONE_ROSTER_FRAME,
    setup: scouterOff,
    prepare: stripBoard(false),
    ready: stripOpen('dialog', 'Mika', 260, (strip) => strip.getByRole('button', { name: 'Unlink…' }).click()),
  },
  {
    name: 'chardata-estimate',
    board: 'B_CharEstimate',
    path: '/members',
    omit: ROSTER_FRAME,
    setup: lookedUp({ character_ids: ['ch-s3'] }),
    prepare: stripBoard(true),
    ready: stripOpen('complementary', 'Member details', 300, (strip) => expect(strip.locator('[data-fid="estimate-result"]')).toBeVisible()),
  },
  {
    name: 'phone-chardata-estimate',
    board: 'B_PhoneCharEstimate',
    path: '/members',
    omit: PHONE_ROSTER_FRAME,
    setup: lookedUp({ character_ids: ['ch-s3'] }),
    prepare: stripBoard(false),
    ready: stripOpen('dialog', 'Mika', 520, (strip) => expect(strip.locator('[data-fid="estimate-result"]')).toBeVisible()),
  },
  // The run sheet's Estimate panel over HCarling + HStar (B_RosterRunPick's sheet, Ren (1) picked).
  {
    name: 'chardata-estimate-run',
    board: 'B_CharEstimateRun',
    path: '/',
    omit: STAGE_OMIT,
    scope: ESTIMATE_RUN_SCOPE,
    setup: async (page) => {
      await renPicked(page);
      await lookedUp({ run_id: 'r-carling' })(page);
      // The board draws the panel without readiness lines (B_CharReady has them).
      await page.route('**/api/admin/runs/*/readiness', (route) => route.abort());
    },
    prepare: runPickBoard,
    ready: estimateRun,
  },
  {
    name: 'phone-chardata-estimate-run',
    board: 'B_PhoneCharEstimateRun',
    path: '/',
    omit: STAGE_OMIT,
    scope: ESTIMATE_RUN_SCOPE,
    setup: async (page) => {
      await renPicked(page);
      await lookedUp({ run_id: 'r-carling' })(page);
      await page.route('**/api/admin/runs/*/readiness', (route) => route.abort());
    },
    prepare: runPickBoard,
    ready: estimateRun,
  },
  // Readiness on the Party tab (B_CharReady): Mika brings MikaBish as the board draws; no scouter (no Estimate panel).
  {
    name: 'chardata-ready',
    board: 'B_CharReady',
    path: '/',
    omit: STAGE_OMIT,
    scope: READY_SCOPE,
    setup: readyBoard,
    prepare: runPickBoard,
    ready: readyShownOn,
  },
  {
    name: 'phone-chardata-ready',
    board: 'B_PhoneCharReady',
    path: '/',
    omit: STAGE_OMIT,
    scope: READY_SCOPE,
    setup: readyBoard,
    prepare: runPickBoard,
    ready: readyShownOn,
  },
  // Config › Character data: all ok, problems, no key, key refused. The phone boards draw their
  // own Config frame (no phone Config board was approved before them): those pairs compare the
  // section's contents (PHONE_CFG_SCOPE), not the shared frame.
  ...CHARDATA_HEALTH.flatMap(([variant, health]) => [
    { name: `cfg-chardata${variant ? `-${variant.toLowerCase()}` : ''}`, board: `B_CfgCharHealth${variant}`, path: '/config?section=character-data', setup: chardataHealth(health), ready: healthShown },
    {
      name: `phone-cfg-chardata${variant ? `-${variant.toLowerCase()}` : ''}`,
      board: `B_PhoneCfgCharHealth${variant}`,
      path: '/config?section=character-data',
      setup: chardataHealth(health),
      scope: [...PHONE_CFG_SCOPE, 'chardata-nexon-health', 'chardata-scouter-health', 'chardata-sweeper-health', 'chardata-env', 'chardata-post'],
      ready: healthShown,
    },
  ]),
  {
    name: 'inbox-extractor',
    board: 'VarRail2',
    path: '/inbox?tab=extractor&item=p-bm-move',
    ready: (page) => expect(page.getByRole('heading', { level: 2, name: /Move — Black Mage/ })).toBeVisible(),
  },
  {
    name: 'cfg',
    board: 'B_Config',
    path: '/config?section=chatbot',
    // The board shows one unsaved change in the save bar.
    ready: async (page) => {
      await page.getByRole('spinbutton', { name: 'Answers per person' }).fill('2');
      await expect(page.getByText('1 unsaved change')).toBeVisible();
    },
  },
  {
    name: 'cfg-persona',
    board: 'B_CfgPersona',
    path: '/config?section=persona',
    // The board shows two reply profiles selected for a bulk change.
    ready: async (page) => {
      const picks = page.getByRole('table', { name: 'Reply profiles' }).locator('tbody').getByRole('checkbox');
      await picks.nth(0).check();
      await picks.nth(1).check();
      await expect(page.getByText('2 profiles selected.')).toBeVisible();
    },
  },
  {
    name: 'cfg-models',
    board: 'B_CfgModels',
    path: '/config?section=models',
    // The board shows one unsaved change: Chat's reasoning level.
    ready: async (page) => {
      const chat = page.getByRole('group', { name: 'Chat' }).getByRole('combobox', { name: 'Reasoning' });
      const current = await chat.getAttribute('data-value');
      const values = await (await openList(chat)).getByRole('option').evaluateAll((options) => options.map((o) => (o as HTMLElement).dataset.value ?? ''));
      await choose(chat, values.find((v) => v !== current && v !== '') ?? values[0]!);
      await expect(page.getByText('1 unsaved change')).toBeVisible();
    },
  },
  {
    name: 'cfg-roles',
    board: 'B_CfgRoles',
    path: '/config?section=persona',
    ready: async (page) => {
      await page.getByRole('tab', { name: /^Role overrides/ }).click();
      await expect(page.getByText('Current guild roles are loaded.', { exact: false })).toBeVisible();
      // The board shows a reordered draft.
      await page.getByRole('list', { name: 'Role assignments in precedence order' }).getByRole('button', { name: 'Move down' }).first().click();
      await expect(page.getByText('1 unsaved change')).toBeVisible();
    },
  },
  {
    name: 'week-sel',
    board: 'B_WeekSel',
    path: '/',
    // The board shows today's first run selected in the side pane.
    ready: async (page) => {
      await page.locator('[data-run="r-carling"] .plan-card__open').click();
      await expect(page.getByRole('complementary', { name: 'HCarling + HStar' })).toBeVisible();
    },
  },
  {
    name: 'week-runs',
    board: 'B_WeekRuns',
    path: '/?week=next',
    ready: async (page) => {
      await page.getByRole('tab', { name: /^Runs/ }).click();
      await expect(page.getByRole('table', { name: /Every run/ })).toBeVisible();
    },
  },
  {
    name: 'week-answers',
    board: 'B_WeekAnswers',
    path: '/?week=next',
    ready: async (page) => {
      await page.getByRole('tab', { name: /^Answers/ }).click();
      await expect(page.getByRole('heading', { name: 'Still waiting' })).toBeVisible();
    },
  },
  {
    name: 'reminders',
    board: 'B_Reminders',
    path: '/reminders',
    // The "In" column and "today" read the browser clock: pin it to the mock's.
    ready: async (page) => {
      await page.clock.setFixedTime(PINNED_NOW);
      await page.reload();
      await expect(page.getByRole('table', { name: /^Queued reminders/ }).locator('tbody tr').first()).toBeVisible();
    },
  },
  {
    name: 'limits',
    board: 'B_Limits',
    path: '/limits',
    // The board shows the route unmounted: answer as the Rust server does, then reload.
    ready: async (page) => {
      await page.route('**/api/admin/limits', (route) =>
        route.fulfill({ status: 404, json: { error: 'not_found', message: 'No such endpoint on this origin.' } }),
      );
      await page.reload();
      await expect(page.getByRole('link', { name: /Config → Models/ })).toBeVisible();
    },
  },
  // Live Limits: the boards show the mock's three seeded groups (cloud full and
  // queueing, local half-open, legacy open), or its default one gateway group.
  ...(
    [
      ['limits-live', 'B_LimitsLive', 'three', 'Backends'],
      ['limits-live-one', 'B_LimitsLiveOne', 'default', 'Backends'],
      ['limits-queue', 'B_LimitsQueue', 'three', 'Queue'],
      ['limits-admission', 'B_LimitsAdmission', 'three', 'Admission'],
      ['limits-allowances', 'B_LimitsAllowances', 'three', 'Allowances'],
      ['phone-limits', 'B_PhoneLimits', 'three', 'Backends'],
      ['phone-limits-admission', 'B_PhoneLimitsAdmission', 'three', 'Admission'],
    ] as const
  ).map(
    ([name, board, groups, tab]): Pair => ({
      name,
      board,
      path: '/limits',
      ready: async (page) => {
        await page.request.post(`${ADMIN}/__mock/limits`, { data: { groups } });
        await page.reload();
        const chosen = page.getByRole('tab', { name: new RegExp(`^${tab}`) });
        await chosen.click();
        await expect(chosen).toHaveAttribute('aria-selected', 'true');
        await expect(page.getByRole('tabpanel')).toBeVisible();
      },
    }),
  ),
  {
    name: 'extract',
    board: 'B_Extract',
    // The board shows the newest call open on Chat read (the default tab).
    path: '/extractions',
    ready: (page) => expect(page.getByRole('list', { name: 'Messages read' }).getByRole('listitem').first()).toBeVisible(),
  },
  {
    name: 'extract-prompt',
    board: 'B_ExtractPrompt',
    path: '/extractions',
    ready: async (page) => {
      await page.getByRole('tab', { name: 'Prompt' }).click();
      await expect(page.getByRole('tabpanel', { name: 'Prompt' }).locator('pre')).toContainText('Messages:');
    },
  },
  {
    name: 'phone-week',
    board: 'B_PhoneWeek',
    path: '/',
    ready: (page) => expect(page.locator('[data-run="r-carling"]')).toBeVisible(),
  },
  ...(
    [
      ['cfg-pings', 'B_CfgPings', 'pings', 'Pings'],
      ['cfg-watch', 'B_CfgWatch', 'watching', 'Chat watching'],
      ['cfg-self', 'B_CfgSelf', 'self-service', 'Self-service'],
      ['cfg-notify', 'B_CfgNotify', 'notifications', 'Notifications'],
      ['cfg-digest', 'B_CfgDigest', 'digest', 'Weekly digest'],
      ['cfg-reread', 'B_CfgReread', 'rescan', 'Re-read the party channels'],
      ['cfg-access', 'B_CfgAccess', 'access', 'Channel access'],
      ['cfg-theme', 'B_CfgTheme', 'theme', 'Theme'],
      ['cfg-env', 'B_CfgEnv', 'env', 'Set in the environment'],
    ] as const
  ).map(([name, board, section, heading]) => ({
    name,
    board,
    path: `/config?section=${section}`,
    ready: (page: Page) => expect(page.getByRole('heading', { level: 3, name: heading })).toBeVisible(),
  })),
  // c-when is the mock's masked turn: two rounds, one tool call and a stored Model view, as the boards show.
  {
    name: 'chat',
    board: 'B_Chat',
    path: '/chat/c-when',
    ready: (page) => expect(page.getByRole('button', { name: 'Model view (masked)' })).toBeVisible(),
  },
  {
    name: 'chat-trace',
    board: 'B_ChatTrace',
    path: '/chat/c-when',
    ready: async (page) => {
      await page.getByRole('tab', { name: /^Model trace/ }).click();
      await expect(page.getByText('Round 2', { exact: true })).toBeVisible();
    },
  },
  {
    name: 'inbox-empty',
    board: 'B_Empty',
    path: '/inbox',
    // The board shows nothing waiting: answer with an empty inbox, then reload.
    ready: async (page) => {
      await page.route('**/api/admin/inbox', (route) => route.fulfill({ json: [] }));
      await page.reload();
      await expect(page.getByRole('heading', { name: 'Nothing waiting' })).toBeVisible();
    },
  },
  {
    name: 'states-error',
    board: 'B_States',
    path: '/members',
    // The board's "Error and retry" quadrant: a pane whose read failed.
    ready: async (page) => {
      await page.route('**/api/admin/members', (route) =>
        route.fulfill({ status: 503, json: { error: 'unavailable', message: "The server didn't answer in time: the request timed out after 10 seconds." } }),
      );
      await page.reload();
      await expect(page.getByRole('button', { name: 'Try again' })).toBeVisible();
    },
  },
  {
    name: 'phone-nav',
    board: 'B_PhoneNav',
    path: '/',
    ready: async (page) => {
      await page.getByRole('button', { name: 'Open the navigation' }).click();
      const drawer = page.getByRole('dialog', { name: 'Navigation' });
      await expect(drawer).toBeVisible();
      // Measure the drawer at rest, not mid-slide.
      await drawer.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
    },
  },
  // Rail option B (KANADE_MOCKUPS=docs/research/2026-10-11-drafts-mockups): the boards
  // draw the rail over the Drafts page, which has not landed, so only the rail is compared.
  {
    name: 'rail-more-800',
    board: 'B_RailMore800',
    path: '/history',
    scope: ['rail', 'rail-more', 'rail-more-menu'],
    ready: async (page) => {
      await page.locator('.navrail').getByRole('button', { name: 'More', exact: true }).click();
      await expect(page.getByRole('menu', { name: 'More destinations' })).toBeVisible();
    },
  },
  {
    name: 'rail-more-720',
    board: 'B_RailMore720',
    path: '/history',
    scope: ['rail', 'rail-more'],
    ready: (page) => expect(page.locator('.navrail').getByRole('button', { name: 'More', exact: true })).toBeVisible(),
  },
  {
    name: 'rail-more-drawer',
    board: 'B_RailMoreDrawer',
    path: '/history',
    scope: ['drawer', 'drawer-head', 'drawer-close', 'drawer-more', 'drawer-foot'],
    ready: async (page) => {
      await page.getByRole('button', { name: 'Open the navigation' }).click();
      const drawer = page.getByRole('dialog', { name: 'Navigation' });
      await expect(drawer.getByRole('button', { name: /^More/ })).toBeVisible();
      await drawer.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
    },
  },
  {
    name: 'hero-login',
    board: 'HeroLogin',
    path: '/login',
    ready: async (page) => {
      await page.request.post(`${ADMIN}/__mock/session`, { data: { method: 'none' } });
      await page.goto(`${ADMIN}/login?sw=off`);
      await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
      await expect(page.getByText('Carling + Radiant Malefic Star')).toBeVisible();
    },
  },
  // The run sheet: the phone's full sheet, and the pane's "larger view" on a laptop.
  {
    name: 'hero-phone',
    board: 'HeroPhone',
    path: '/',
    setup: chardataOff,
    ready: async (page) => {
      await page.locator('[data-run="r-carling"] .plan-card__open').click();
      await expect(page.getByRole('dialog', { name: 'HCarling + HStar' })).toBeVisible();
    },
  },
  {
    name: 'hero-sheet',
    board: 'HeroSheet',
    path: '/',
    setup: chardataOff,
    ready: async (page) => {
      await page.locator('[data-run="r-carling"] .plan-card__open').click();
      await page.getByRole('complementary', { name: 'HCarling + HStar' }).getByRole('button', { name: 'Open in a larger view' }).click();
      await expect(page.getByRole('dialog', { name: 'HCarling + HStar' })).toBeVisible();
    },
  },
  // The date picker (picker boards, KANADE_MOCKUPS=…/2026-10-04-picker-mockups): Chat's
  // range popover with an applied range reopened, History's "Since", and the phone sheet.
  {
    name: 'dates-range',
    board: 'P_Dates',
    path: '/chat',
    ready: async (page) => {
      await page.getByRole('button', { name: /^Filters/ }).click();
      const trigger = page.getByRole('button', { name: /^Dates/ });
      await trigger.click();
      await page.getByRole('dialog', { name: 'Date range' }).getByRole('button', { name: 'Last 7 days' }).click();
      await page.getByRole('dialog', { name: 'Date range' }).getByRole('button', { name: 'Apply' }).click();
      await trigger.click();
      await expect(page.getByRole('dialog', { name: 'Date range' }).getByRole('grid')).toBeVisible();
    },
  },
  {
    name: 'dates-since',
    board: 'P_DatesSpec',
    path: '/history',
    ready: async (page) => {
      const trigger = page.getByRole('complementary', { name: 'Change details' }).getByRole('button', { name: /^Since/ });
      await trigger.click();
      await page.getByRole('dialog', { name: 'Since' }).getByRole('button', { name: /^Last reset/ }).click();
      await trigger.click();
      await expect(page.getByRole('dialog', { name: 'Since' }).getByRole('grid')).toBeVisible();
    },
  },
  {
    name: 'dates-phone',
    board: 'P_DatesPhone',
    path: '/chat',
    ready: async (page) => {
      await page.getByRole('button', { name: /^Filters/ }).click();
      await page.getByRole('button', { name: /^Dates/ }).click();
      const sheet = page.getByRole('dialog', { name: 'Dates' });
      await sheet.getByRole('button', { name: 'Last boss week' }).click();
      await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
    },
  },
  // The Move picker (KANADE_MOCKUPS=docs/research/2026-10-04-picker-mockups). Next week
  // has a clash to show: HCarling + HStar on Monday at HFA's 20:00.
  {
    name: 'move-pane',
    board: 'Main',
    path: '/?week=next',
    ready: async (page) => {
      await page.locator('[data-run="n-carling"] .plan-card__open').click();
      const pane = page.getByRole('complementary', { name: 'HCarling + HStar' });
      await pane.getByRole('radio', { name: /^Mon 05/ }).click();
      await pane.getByRole('textbox', { name: 'Type a day and time' }).fill('20:00');
      await expect(pane.getByRole('status').filter({ hasText: 'Clash:' })).toBeVisible();
    },
  },
  {
    name: 'move-widths',
    board: 'P_MoveWidths',
    path: '/',
    // The board's first pane: a typed day that has passed.
    ready: async (page) => {
      await page.locator('[data-run="r-bm"] .plan-card__open').click();
      const typed = page.getByRole('complementary', { name: 'XBM' }).getByRole('textbox', { name: 'Type a day and time' });
      await typed.fill('sat 21:30');
      await typed.press('Enter');
      await expect(page.getByText('Sat 26 has passed.', { exact: false })).toBeVisible();
    },
  },
  {
    name: 'move-phone',
    board: 'P_MovePhone',
    path: '/?week=next',
    ready: async (page) => {
      await page.locator('[data-run="n-carling"] .plan-card__open').click();
      await page.getByRole('dialog', { name: 'HCarling + HStar' }).getByRole('button', { name: 'Move HCarling + HStar…' }).click();
      const view = page.getByRole('dialog', { name: 'Move HCarling + HStar' });
      await view.getByRole('radio', { name: /^Mon 05/ }).click();
      await view.getByRole('textbox', { name: 'Or type a day and time' }).fill('20:00');
      await expect(view.getByRole('status').filter({ hasText: 'Clash:' })).toBeVisible();
    },
  },
  // The dropdown (picker boards P_Select, P_SelectSpec, P_SelectPhone): Reminders'
  // filters with a run chosen and the list reopened, History's searchable "Who",
  // the Re-read multi-select, and the phone's pills over native selects.
  {
    name: 'select-bar',
    board: 'P_Select',
    path: '/reminders',
    ready: async (page) => {
      await page.getByRole('button', { name: /^Filters/ }).click();
      const run = page.getByRole('combobox', { name: 'Run' });
      await choose(run, { index: 4 });
      await run.press('ArrowDown');
      await page.keyboard.press('ArrowDown');
      await expect(page.getByRole('listbox', { name: 'Run' })).toBeVisible();
    },
  },
  {
    name: 'select-search',
    board: 'P_SelectSpec',
    path: '/history',
    ready: async (page) => {
      await page.getByRole('combobox', { name: 'Who' }).click();
      await page.getByRole('combobox', { name: 'Filter people' }).fill('yu');
      await expect(page.getByRole('listbox', { name: 'Who' })).toBeVisible();
    },
  },
  {
    name: 'select-multi',
    board: 'P_SelectSpec',
    path: '/config?section=rescan',
    ready: async (page) => {
      const channels = page.getByRole('combobox', { name: 'Channels to re-read' });
      const list = await openList(channels);
      for (const i of [0, 1, 3]) await list.getByRole('option').nth(i).click();
      await channels.press('ArrowDown');
    },
  },
  {
    name: 'select-phone',
    board: 'P_SelectPhone',
    path: '/reminders',
    ready: async (page) => {
      await page.getByRole('button', { name: /^Filters/ }).click();
      await choose(page.getByRole('combobox', { name: 'Run' }), { index: 4 });
    },
  },
];

test.describe('layout fidelity', () => {
  test.skip(!ENABLED, 'run with `bun run fidelity` (needs a KANADE_FIDELITY=1 build)');

  for (const pair of [...PAIRS, ...DRAFT_PAIRS, ...GRAPH_PAIRS, ...PRESENCE_PAIRS, ...PUBLIC_PAIRS, ...MEMBER_CHAR_PAIRS].filter((p) => !ONLY.length || ONLY.includes(p.name))) {
    test(pair.name, async ({ page, browser }) => {
      test.skip(!boardExists(pair.board), `no board ${pair.board}.dc.html under ${MOCKUPS}`);
      const dir = join(OUT, pair.name);
      mkdirSync(dir, { recursive: true });

      const board = await renderBoard(browser, pair.board);
      await pair.prepare?.(board.page);
      // A scoped pair compares only its subject's regions, on both sides.
      const scope = new Set(pair.scope ?? []);
      const boardRegions = (await skeleton(board.page, pair.omit)).filter((r) => !scope.size || scope.has(r.fid));
      const boardPng = await board.page.screenshot({ animations: 'disabled' });
      await board.page.close();

      await page.setViewportSize(pair.viewport ?? { width: board.width, height: board.height });
      // The face the boards are drawn in: the admin boards in Nazuna, the portal's in Otonose.
      await page.addInitScript((colorway) => {
        localStorage.setItem('colorway', colorway);
        localStorage.setItem('theme', 'light');
      }, pair.app === 'public' ? 'marigold' : 'blossom');
      await pair.setup?.(page);
      await page.goto(`${pair.app === 'public' ? PUBLIC : ADMIN}${pair.path}${pair.path.includes('?') ? '&' : '?'}sw=off`);
      await pair.ready(page);
      await settle(page);
      // Boards older than rail option B and staging leave those regions untagged: on those,
      // the app's are no finding (they stay compared where a board tags them).
      const drawn = new Set(boardRegions.map((r) => r.fid));
      const untagged = LATER_FIDS.filter((fid) => !drawn.has(fid));
      let appRegions = (await skeleton(page, pair.omit))
        .filter((r) => (!scope.size || scope.has(r.fid)) && !untagged.includes(r.fid))
        .map((r) => ({ ...r, children: r.children.filter((c) => !untagged.includes(c)) }));
      // A strip of the frame (Mast): top-level regions keep their place against the board's frame, not the larger page.
      if (pair.viewport)
        appRegions = appRegions.map((r) =>
          r.parent ? r : { ...r, box: { ...r.box, right: board.width - r.abs.x - r.abs.w, bottom: board.height - r.abs.y - r.abs.h, pw: board.width, ph: board.height } },
        );
      const appPng = await page.screenshot({ animations: 'disabled', clip: { x: 0, y: 0, width: board.width, height: board.height } });

      const notes: string[] = [];
      if (board.errors.length) notes.push(`Board script errors: ${board.errors.join(' | ')}`);
      if (!boardRegions.length) notes.push(`The board has no data-fid tags: tag ${pair.board}.dc.html first.`);
      if (!appRegions.length) notes.push('The app has no data-fid tags: untagged, or built without KANADE_FIDELITY=1.');
      const findings = boardRegions.length && appRegions.length ? diff(boardRegions, appRegions) : [];

      writeFileSync(join(dir, 'board.json'), JSON.stringify(boardRegions, null, 2));
      writeFileSync(join(dir, 'app.json'), JSON.stringify(appRegions, null, 2));
      writeFileSync(join(dir, 'report.json'), JSON.stringify({ pair, notes, findings }, null, 2));
      writeFileSync(join(dir, 'report.md'), markdown(pair.name, pair.board, pair.path, findings, notes));
      await composite(
        browser,
        join(dir, 'composite.png'),
        { png: boardPng, regions: boardRegions, width: board.width, height: board.height },
        { png: appPng, regions: appRegions, width: board.width, height: board.height },
        findings,
      );
      const counts = [1, 2, 3, 4].map((s) => findings.filter((f) => f.severity === s).length);
      console.log(`${pair.name}: ${findings.length} findings (structure ${counts[0]}, layout ${counts[1]}, detail ${counts[2]}, app-only ${counts[3]}) → ${dir}`);
    });
  }
});
