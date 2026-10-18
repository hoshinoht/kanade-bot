import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { EstimateJob, EstimateStarted, Limits } from '@kanade/api-types';
import type { Locator, Page } from '@playwright/test';
import { boardExists, composite, diff, markdown, MOCKUPS, renderBoard, settle, skeleton } from './fidelity-kit';
import { TEASE_PAIRS } from './fidelity-tease';
import {
  ADMIN,
  expect,
  PINNED_NOW,
  PUBLIC,
  test,
  choose,
  csrf,
  dropSessions,
  endMemberSessions,
  openList,
  setCharacterData,
  setClock,
  setEstimates,
  setLimitGroups,
  setMemberCharacters,
  setModules,
  setPortal,
  signInPublic,
  unconditional,
  useLook,
} from './support';

// Layout fidelity against the M3E boards: `bun run fidelity [pair ...] [--keep]`
// (scripts/fidelity.ts builds with KANADE_FIDELITY=1 so the data-fid tags
// survive, runs this spec, then rebuilds clean). Writes, per pair, under the
// git-ignored e2e/.captures/fidelity/: report.md, report.json, composite.png
// (board left, app right, findings outlined), board.json and app.json.
// It reports; it never fails on findings. It drives the real-API e2e server:
// the seed keeps the retired dev mock's ids, names and times, and the boards'
// states come from its `/__e2e/` controls.
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
  /** The board's face, where it is drawn dark. */
  theme?: 'dark';
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

/** A harness control (`examples/e2e_server/controls*.rs`) that must succeed. */
const control = async (page: Page, path: string, data?: unknown) => {
  const done = await page.request.post(`${ADMIN}/__e2e/${path}`, data === undefined ? {} : { data });
  expect(done.ok(), `/__e2e/${path}: ${await done.text()}`).toBe(true);
  return done;
};
/** The real member ids (`1` + the mock's id, zero-padded: examples/e2e_server/seed/ids.rs). */
const member = (mockId: number) => `1${String(mockId).padStart(17, '0')}`;
/**
 * The server clock at Fri 25 Sep 12:00 guild time, as drafts.spec's request edits: XKalos (amended
 * to 22:00) is still ahead, so the weekly-timing edit offers Keep/Update for it, as the boards draw.
 */
const beforeKalos = () => setClock('2026-09-25T04:00:00Z');

/**
 * Module `character_data` off (it starts on): boards drawn before it
 * (the run sheet's Estimate panel and readiness lines) compare against their own screen.
 */
const chardataOff = () => setModules({ character_data: false });
const withChardataOff = (setup?: (page: Page) => Promise<void>) => async (page: Page) => {
  await chardataOff();
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
  await endMemberSessions();
  // Reopening the tab re-reads the devices: the 401 ends the screen.
  await page.getByRole('tab', { name: 'Profile' }).click();
  await page.getByRole('tab', { name: /^Devices/ }).click();
  await expect(page.getByRole('heading', { level: 1, name: "You've been signed out" })).toBeVisible();
};
/** A Discord link's slot: Wed 30 Sep 21:30 guild time, the last day of the seeded boss week. */
const MOVE_TO = '2026-09-30T13:30:00Z';
/** Last week's Kalos (the seed's `p-kalos`) at its old Fri 22:00. */
const PAST_MOVE_TO = '2026-09-18T14:00:00Z';
const moveShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="move-result"]')).toBeVisible();
};
const moveNotice = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await expect(page.locator('main [data-fid="move-notice"]')).toBeVisible();
};
/** Ren took Asahi off HCarling + HMaleficStar after the link was sent. */
const removedFromCarling = async (page: Page) => {
  await signInPublic(page);
  await control(page, 'public/remove', { run: 'r-carling' });
};
/** Signed in longer ago than the fresh window: the next write asks "Confirm it's you". */
const unfresh = (page: Page) => signInPublic(page, { stale: true });
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
/**
 * Signed in with the live stream cut: the real stream's hints would tell the page about
 * the requests `requestLimit` sends elsewhere before its press (precedent: public-requests.spec).
 */
const signedInQuiet = async (page: Page) => {
  await signInPublic(page);
  await page.route('**/api/public/events*', (route) => route.abort());
};
/** Sends a member request straight to the API, as another tab would; answers the request. */
const sendElsewhere = async (page: Page, body: Record<string, unknown>, key: string): Promise<{ id: string }> => {
  const session = await page.request.get(`${PUBLIC}/api/public/session`);
  const sent = await page.request.post(`${PUBLIC}/api/public/requests`, {
    headers: { 'X-Kanade-CSRF': session.headers()['x-kanade-csrf'] ?? '', 'Idempotency-Key': key, Origin: PUBLIC },
    data: body,
  });
  expect(sent.status(), await sent.text()).toBe(201);
  return sent.json();
};
/** XBM is the one open run Asahi is not on; Kaito is not on HCarling + HMaleficStar. */
const SWAP_CARLING = { kind: 'swap', run_id: 'r-carling', with: member(1009) };
/** Two more requests sent elsewhere fill the three open: the press is refused, the toast says so and the key is off. */
const requestLimit = async (page: Page) => {
  await requestForm(page);
  await sendElsewhere(page, { kind: 'join', run_id: 'r-bm' }, 'fid-limit-join');
  await sendElsewhere(page, SWAP_CARLING, 'fid-limit-swap');
  await page.getByRole('button', { name: 'Send request' }).click();
  await expect(page.locator('main [data-fid="request-limit"]')).toBeVisible();
  await expect(page.getByRole('group', { name: 'Notification' })).toBeVisible();
};
/**
 * Requests-Expired: a swap on HCarling + HMaleficStar nobody decided before its boss week ended.
 * A real request expires only once its week is over (the seed cannot hold one: the real
 * scheduler refuses the mock's expired swap, and moving the clock past the week would change
 * every other row), so Asahi sends the swap and the page reads it as expired.
 */
const expiredSwap = async (page: Page) => {
  await signInPublic(page);
  const { id } = await sendElsewhere(page, SWAP_CARLING, 'fid-expired-swap');
  await page.route('**/api/public/requests/mine*', async (route) => {
    const mine = await (await route.fetch()).json();
    for (const request of mine.requests) if (request.id === id) request.state = 'expired';
    route.fulfill({ json: { ...mine, open: mine.open - 1 } });
  });
};
const expiredShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  await page.locator('main [data-fid="requests-row"]').filter({ hasText: 'expired' }).first().click();
  await expect(page.locator('main .req-detail')).toContainText('expired');
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
  // Bosses and guides (the admin Bosses window, shared): the list, Carling's guide (Hard, Phases, Destiny), Kai (event). The real catalog names Carling "Karling".
  { name: 'pub-bosses', board: 'Bosses', path: '/bosses', setup: signInPublic, ready: bossesShown },
  { name: 'pub-guide', board: 'Guide', path: '/bosses/Carling', setup: signInPublic, ready: guideShown('Karling') },
  { name: 'pub-guide-phases', board: 'Guide-Phases', path: '/bosses/Carling?tab=phases', setup: signInPublic, ready: guideShown('Karling') },
  { name: 'pub-guide-destiny', board: 'Guide-Destiny', path: '/bosses/Carling?difficulty=Destiny', setup: signInPublic, ready: guideShown('Karling') },
  { name: 'pub-guide-event', board: 'Guide-Event', path: '/bosses/Kai', setup: signInPublic, ready: guideShown('Kai') },
  { name: 'pub-phone-bosses', board: 'PhoneBosses', path: '/bosses', setup: signInPublic, ready: bossesShown },
  { name: 'pub-phone-guide', board: 'PhoneGuide', path: '/bosses/Carling', setup: signInPublic, ready: guideShown('Karling') },
  { name: 'pub-phone-guide-phases', board: 'PhoneGuide-Phases', path: '/bosses/Carling?tab=phases', setup: signInPublic, ready: guideShown('Karling') },
  { name: 'pub-phone-guide-destiny', board: 'PhoneGuide-Destiny', path: '/bosses/Carling?difficulty=Destiny', setup: signInPublic, ready: guideShown('Karling') },
  // Member writes: the run's Move page (a Discord link), its stale shapes, the answer's fresh sign-in, the request form and My requests.
  { name: 'pub-move', board: 'Move', path: `/runs/r-carling?move_to=${MOVE_TO}`, setup: signInPublic, ready: moveShown },
  { name: 'pub-move-notin', board: 'Move-NotIn', path: `/runs/r-carling?move_to=${MOVE_TO}`, setup: removedFromCarling, ready: moveNotice },
  { name: 'pub-move-weekover', board: 'Move-WeekOver', path: `/runs/p-kalos?move_to=${PAST_MOVE_TO}`, setup: signInPublic, ready: moveNotice },
  { name: 'pub-confirm-answer', board: 'ConfirmAnswer', path: '/?run=r-carling', setup: unfresh, ready: confirmAnswer },
  { name: 'pub-request-form', board: 'RequestForm', path: '/requests/new?run=r-carling', setup: signInPublic, ready: requestForm },
  { name: 'pub-request-form-confirm', board: 'RequestForm-Confirm', path: '/requests/new?run=r-carling', setup: unfresh, ready: requestConfirm },
  { name: 'pub-request-form-limit', board: 'RequestForm-Limit', path: '/requests/new?run=r-carling', setup: signedInQuiet, ready: requestLimit },
  { name: 'pub-requests', board: 'Requests', path: '/requests', setup: signInPublic, ready: requestsShown },
  { name: 'pub-requests-expired', board: 'Requests-Expired', path: '/requests', setup: expiredSwap, ready: expiredShown },
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
// (`setMemberCharacters('board')`). The states boards (M_CharStates*,
// M_LinkOutcomes, M_EstStates*, M_RunStates, M_PhoneCharStates, M_PhoneEstStates*) are grids of
// cells over several screens: e2e/public-characters.spec.ts covers each of their states instead.
const boardRoster = async (page: Page) => {
  await signInPublic(page);
  await setMemberCharacters('board');
};
/** M_CharsMenu draws no waiting request: an admin withdraws Ren's first. */
const boardRosterNoRequest = async (page: Page) => {
  await boardRoster(page);
  expect((await page.request.delete(`${ADMIN}/api/admin/characters/ch-b2/link`, { headers: await csrf(page.request) })).ok()).toBe(true);
};
const boardRosterUnfresh = async (page: Page) => {
  await setMemberCharacters('board');
  await unfresh(page);
};
/** The board draws the party's cached estimates: an admin looks the run's party up first. */
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

// The character roster (module `roster`, on by default): Mika's sheet on its Characters tab.
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
/** Stages `ops` in draft `id`, each a real drafts-service write that must succeed. */
const stage = async (page: Page, id: string, ops: unknown[]) => {
  const headers = await csrf(page.request);
  let version: number = (await (await page.request.get(`${ADMIN}/api/admin/drafts/${id}`)).json()).version;
  for (const op of ops) {
    const done = await page.request.post(`${ADMIN}/api/admin/drafts/${id}/ops`, { headers, data: { version, op } });
    expect(done.ok(), `staging ${JSON.stringify(op)} in ${id}: ${await done.text()}`).toBe(true);
    version = (await done.json()).version;
  }
};
/**
 * The boards' drafts hold six changes; the seed stages two (Weekend reshuffle) and the drafts
 * board three (Carling retime). The mock's extra changes moved HFA and cancelled Bellona, which
 * have ended by the pinned Tuesday (the real scheduler refuses them): next week's runs stand in,
 * so the merge preview again shows a slot change and a cancellation of their own.
 */
const moreOps = (id: string) => (page: Page) =>
  stage(
    page,
    id,
    [
      { kind: 'party', run: 'r-bm', add: [member(1004)], remove: [member(1008)] },
      { kind: 'status', run: 'r-bm', status: 'confirmed' },
      { kind: 'move', run: 'n-carling', day: 6, time: '23:00' },
      { kind: 'status', run: 'n-kalos', status: 'cancelled' },
    ].slice(0, id === 'd-weekend' ? 4 : 3),
  );
/**
 * B_DraftsConflict's data on Carling retime: the open draft heads the list (the board's
 * conflicted draft is its first row), and the upstream cancellation flags changes 3 and 4,
 * as the board's does, rather than the seeded 2 and 3 (change 2 is a clean one on the board).
 */
const conflictLikeBoard = async (page: Page) => {
  // The drafts boards' closed drafts and Carling retime's conflicts.
  await control(page, 'drafts/board');
  await moreOps('d-carling')(page);
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
/** The boards list three open drafts: the seeded two plus "Bellona clean-up" with one change (on next week's HFA: Bellona has ended). */
const thirdDraft = async (page: Page) => {
  const headers = await csrf(page.request);
  const made = await (await page.request.post(`${ADMIN}/api/admin/drafts`, { headers, data: { title: 'Bellona clean-up' } })).json();
  await stage(page, made.id, [{ kind: 'status', run: 'n-fa', status: 'cancelled' }]);
};
const draftShown = (title: string) => async (page: Page) => {
  await expect(page.getByRole('heading', { level: 2, name: title })).toBeVisible();
  await page.waitForLoadState('networkidle');
};
const draftsList = async (page: Page) => {
  await expect(page.getByRole('listbox', { name: 'Open drafts' })).toBeVisible();
  await page.waitForLoadState('networkidle');
};
/** No open drafts; the closed tab (the drafts board's two) gains the board's third, expired row. */
const noOpenDrafts = async (page: Page) => {
  await control(page, 'drafts/board');
  await page.route('**/api/admin/drafts?state=open', async (route) => route.fulfill({ json: { ...(await (await route.fetch()).json()), drafts: [] } }));
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
/** Tsubame's request changes under the edit (another admin's edit on the real server): Approve edited is refused. */
const requestRefused = async (page: Page) => {
  await requestEdited(page);
  await control(page, 'inbox/kalos-v4');
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
/** HCarling + HMaleficStar in the full sheet (the pane's larger view on a laptop, the sheet on a phone). */
const carlingSheet = async (page: Page) => {
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const pane = page.getByRole('complementary', { name: 'HCarling + HMaleficStar' });
  if (page.viewportSize()!.width >= 900) {
    await expect(pane).toBeVisible();
    await pane.getByRole('button', { name: 'Open in a larger view' }).click();
  }
  const sheet = page.getByRole('dialog', { name: 'HCarling + HMaleficStar' });
  await expect(sheet).toBeVisible();
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  return sheet;
};
/** A laptop pair and its phone board (`phone-<name>`, `B_Phone…`), the same unless `phone` says otherwise. */
const withPhone = (pair: Pair, phone: Partial<Pair> = {}): Pair[] => [pair, { ...pair, name: `phone-${pair.name}`, board: pair.board.replace(/^B_/, 'B_Phone'), ...phone }];
const noDrafts = async (page: Page) => {
  await expect(page.getByRole('heading', { name: 'No open drafts' })).toBeVisible();
  await expect(page.locator('[data-fid="empty-recent-row"]').first()).toBeVisible();
};
const DRAFT_PAIRS: Pair[] = [
  { name: 'drafts', board: 'B_Drafts', path: '/drafts?item=d-weekend', omit: DRAFTS_OMIT, setup: moreOps('d-weekend'), ready: draftShown('Weekend reshuffle') },
  { name: 'drafts-conflict', board: 'B_DraftsConflict', path: '/drafts?item=d-carling', omit: DRAFTS_OMIT, setup: conflictLikeBoard, ready: draftShown('Carling retime') },
  ...withPhone({ name: 'drafts-empty', board: 'B_DraftsEmpty', path: '/drafts', omit: DRAFTS_OMIT, setup: noOpenDrafts, ready: noDrafts }, { omit: PHONE_DRAFTS_OMIT }),
  ...withPhone(
    { name: 'drafts-loading', board: 'B_DraftsLoading', path: '/drafts', omit: DRAFTS_OMIT, setup: holdDrafts, ready: (page) => expect(page.getByText('Loading drafts…')).toBeVisible() },
    { omit: PHONE_DRAFTS_OMIT },
  ),
  ...withPhone(
    { name: 'drafts-error', board: 'B_DraftsError', path: '/drafts', omit: DRAFTS_OMIT, setup: failDrafts, prepare: errorBoard, ready: (page) => expect(page.getByRole('button', { name: 'Try again' })).toBeVisible() },
    { omit: PHONE_DRAFTS_OMIT },
  ),
  { name: 'phone-drafts', board: 'B_PhoneDrafts', path: '/drafts', omit: PHONE_DRAFTS_OMIT, setup: thirdDraft, ready: draftsList },
  { name: 'phone-draft-conflict', board: 'B_PhoneDraftConflict', path: '/drafts?item=d-carling', omit: PHONE_DRAFTS_OMIT, setup: conflictLikeBoard, ready: draftShown('Carling retime') },
  ...withPhone(
    { name: 'inbox-edit', board: 'B_InboxEdit', path: '/inbox?tab=self_service&item=p-kalos-fixed', omit: INBOX_EDIT_OMIT, setup: beforeKalos, prepare: inboxEditBoard, ready: requestEdited },
    { omit: PHONE_INBOX_EDIT_OMIT },
  ),
  ...withPhone(
    { name: 'inbox-edit-conflict', board: 'B_InboxEditConflict', path: '/inbox?tab=self_service&item=p-kalos-fixed', omit: INBOX_EDIT_OMIT, setup: beforeKalos, prepare: inboxEditBoard, ready: requestRefused },
    { omit: PHONE_INBOX_EDIT_OMIT },
  ),
  {
    name: 'run-stage',
    board: 'B_RunSheetStage',
    path: '/',
    omit: STAGE_OMIT,
    scope: STAGE_SCOPE,
    prepare: stageBoard,
    // The board's picker lists three drafts; the seed has two.
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
    // (the seed adds a second Ren, #1013, this week).
    setup: async (page) => {
      await chardataOff();
      await stage(page, 'd-weekend', [
        { kind: 'move', run: 'r-carling', day: 6, time: '21:30' },
        { kind: 'party', run: 'r-carling', add: [member(1006)] },
      ]);
      await page.route('**/api/admin/week*', async (route) => {
        const response = await route.fetch();
        if (response.status() !== 200) return route.fulfill({ response });
        const week = await response.json();
        for (const run of week.runs) if (run.id === 'r-carling') run.participants = run.participants.filter((p: { id: string }) => p.id !== member(1013));
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
// /__e2e/presence/board` seeds the boards' other admins: Ren editing p-kalos-fixed with
// Kaito viewing it 40 s ago, and Ren editing HCarling + HMaleficStar. The Inbox boards redraw
// B_InboxEdit (the inbox-edit pairs compare it) and the run sheet boards B_RunSheetStage
// (the stage pairs' frame): these compare what presence adds. B_PresenceDraft and
// B_PhonePresenceDraft have no pair: drafts are their author's only, so Draft detail
// shows no presence (the boards draw another admin's view of the draft).
const PRESENCE_SCOPE = ['presence', 'presence-pop', 'presence-note'];
const presenceBoard = async (page: Page) => {
  await control(page, 'presence/board');
};
/** The Inbox presence boards over B_InboxEdit's state (its clock). */
const presenceInbox = async (page: Page) => {
  await beforeKalos();
  await presenceBoard(page);
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
    setup: presenceInbox,
    ready: async (page) => {
      await requestEdited(page);
      await page.getByRole('button', { name: /^Other admins here/ }).click();
      await expect(page.getByRole('dialog', { name: 'Admins here' })).toBeVisible();
    },
  },
  { name: 'phone-presence-inbox', board: 'B_PhonePresenceInbox', path: '/inbox?tab=self_service&item=p-kalos-fixed', omit: PHONE_INBOX_EDIT_OMIT, scope: PRESENCE_SCOPE, prepare: inboxEditBoard, setup: presenceInbox, ready: requestEdited },
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
// shared render/ beside the boards). `POST /__e2e/graph/board` seeds the boards' History
// (head #39) and drafts. The rail is the rail-more pairs'; the phone top bar phone-nav's.
const GRAPH_OMIT = ['rail', 'topbar'];
const PHONE_GRAPH_OMIT = [...GRAPH_OMIT, 'page-line'];
const graphBoard = async (page: Page) => {
  await control(page, 'graph/board');
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
const graphEmpty = (page: Page) =>
  page.route('**/api/admin/drafts/graph?*', async (route) => route.fulfill({ json: { ...(await (await route.fetch()).json()), drafts: [] } }));
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
  ...withPhone({ name: 'history-graph-list', board: 'B_HistoryGraphList', path: '/history?tab=graph&seq=38', omit: GRAPH_OMIT, setup: graphBoard, ready: graphList }, { path: '/history?tab=graph', omit: PHONE_GRAPH_OMIT }),
  ...withPhone({ name: 'history-graph-long', board: 'B_HistoryGraphLong', path: '/history?tab=graph', omit: GRAPH_OMIT, setup: graphBoard, ready: graphLong }, { omit: PHONE_GRAPH_OMIT }),
  ...withPhone(
    {
      name: 'history-graph-empty',
      board: 'B_HistoryGraphEmpty',
      path: '/history?tab=graph',
      omit: GRAPH_OMIT,
      setup: graphEmpty,
      ready: (page) => expect(page.getByRole('heading', { name: 'No drafts to draw yet' })).toBeVisible(),
    },
    { omit: PHONE_GRAPH_OMIT },
  ),
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
];

const rosterOff = () => setModules({ roster: false });
const rosterTab = async (page: Page, role: 'complementary' | 'dialog', name: string) => {
  // Mikan is on the roster too.
  await page.getByRole('button', { name: /^Mika\b/ }).click();
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
 * B_CharEstimateRun redraws the run sheet with the roster's Party tab. Its "Later slice" tags and
 * Mikadere's disabled level reason are not built (user, 2026-10-12: any active character, no
 * level filter): the board loses them before it is measured.
 */
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
  for (const run of ['r-carling', 'r-nstar']) {
    const picked = await page.request.put(`${ADMIN}/api/admin/runs/${run}/characters/${member(1002)}`, { headers, data: { character_id: 'ch-s7', version: 0 } });
    expect(picked.ok(), `Ren's pick on ${run}: ${await picked.text()}`).toBe(true);
  }
};

// Character data (KANADE_MOCKUPS=docs/research/2026-10-12-chardata-mockups). The strip boards
// splice their dialog into B_RosterEdit's Members window (the roster pairs' frame, left out the
// same way); the run-sheet boards redraw B_RosterRunPick's sheet, so those pairs compare what
// the Estimate panel adds; the Config boards are the whole Settings window. The States boards
// (B_CharStates*, B_EstStates*, B_EstRunStates*) are grids of cells over several screens, not
// one screen: e2e/chardata.spec.ts covers each of their states instead.
const scouterOff = () => setEstimates('off');
const chardataHealth = (health: 'ok' | 'alert' | 'setup' | 'refused') => () => setCharacterData(health);
/** A finished scouter lookup. */
const lookedUp = (target: { character_ids: string[] } | { run_id: string }) => async (page: Page) => {
  const headers = await csrf(page.request);
  const { job } = await (await page.request.post(`${ADMIN}/api/admin/estimates`, { headers, data: target })).json();
  await expect.poll(async () => (await (await page.request.get(`${ADMIN}/api/admin/estimates/${job.id}`)).json()).state, { timeout: 30_000 }).not.toBe('running');
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
  // The panel's members table is gone (the Party grid carries each member's %): wait for Who carries.
  await expect(panel.locator('[data-fid="estimate-share"]')).toBeVisible();
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
/** The Alert boards draw the Estimates card edited 7 → 3 (one unsaved change), as the `cfg` pair types into Chatbot. */
const healthEdited = async (page: Page) => {
  await healthShown(page);
  await page.getByRole('spinbutton', { name: 'Refresh estimates every' }).fill('3');
  await expect(page.getByText('1 unsaved change')).toBeVisible();
};
const PHONE_CFG_SCOPE = ['cfg-title', 'cfg-savebar', 'cfg-save'];
/**
 * History's newest change (the head), as the boards open a member's change. The page opens on
 * the newest row, which on the real server is a Config save (no member revert), and the seed's
 * import also touches next week's runs, so it heads the list under next week's group.
 */
const newestChange = async (page: Page) => {
  const { head } = await (await page.request.get(`${ADMIN}/api/admin/history`)).json();
  return page.locator(`.history-row[data-history="${head.seq}"]`).first();
};
/** Ready once what `shown` finds is visible. */
const visible = (shown: (page: Page) => Locator) => (page: Page) => expect(shown(page)).toBeVisible();
/** Clicks what `open` finds, then waits for what `shown` finds. */
const opens = (open: (page: Page) => Locator, shown: (page: Page) => Locator) => async (page: Page) => {
  await open(page).click();
  await expect(shown(page)).toBeVisible();
};

// Estimate share chart, "Who carries" (KANADE_MOCKUPS=docs/research/2026-10-15-estimate-share-chart):
// the boards redraw the as-built run sheet (r-kalos one boss, r-carling two) and the portal run
// pane around the panel after an admin's finished party lookup, so the admin pairs compare the
// Estimate panel the aside (laptop) or Party list (phone) is scrolled to, and the portal pairs
// Party · characters scrolled into view. The board's party-slot readings are patched, not built.
const SHARE_SCOPE = [
  'sheet-aside',
  'estimate-panel',
  'estimate-label',
  'estimate-disclaimer',
  'estimate-run',
  'estimate-combined',
  'estimate-share',
  'estimate-share-missing',
  'estimate-members',
  'estimate-missing',
  'estimate-assumed',
  'estimate-issues',
];
/** `run`'s full sheet with its finished lookup, the panel scrolled to the top of its box as the boards leave it. */
const sharePanel = (run: string, title: string) => async (page: Page) => {
  await page.locator(`[data-run="${run}"] .plan-card__open`).click();
  if (page.viewportSize()!.width >= 900) await page.getByRole('complementary', { name: title }).getByRole('button', { name: 'Open in a larger view' }).click();
  const sheet = page.getByRole('dialog', { name: title });
  await expect(sheet.locator('[data-fid="estimate-share"]')).toBeVisible();
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  await sheet.locator('[data-fid="estimate-panel"]').evaluate((panel) => {
    for (let box = panel.parentElement; box; box = box.parentElement)
      if (box.scrollHeight > box.clientHeight && getComputedStyle(box).overflowY !== 'visible')
        box.scrollTop = panel.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop - 8;
  });
};
/** The run's finished lookup, one member's critical rate assumed as the boards draw (Ren on Kalos, Mika on Carling + Star). */
const shareLookedUp = (run: string) => async (page: Page) => {
  await setEstimates('assumed', run === 'r-kalos' ? 'RenShade' : 'MikaBish');
  const headers = await csrf(page.request);
  const { job } = await (await page.request.post(`${ADMIN}/api/admin/estimates`, { headers, data: { run_id: run } })).json();
  await expect.poll(async () => (await (await page.request.get(`${ADMIN}/api/admin/estimates/${job.id}`)).json()).state, { timeout: 30_000 }).toBe('done');
};
/** The portal boards tag only the pane and Party · characters (the rest is the as-built week the mc-* pairs compare). */
const SHARE_PORTAL_SCOPE = [
  'week-pane',
  'week-party-chars',
  'party-chars-note',
  'estimate-combined',
  'estimate-share',
  'estimate-share-keys',
  'estimate-share-missing',
  'party-char-row',
  'party-char-line',
  'party-char-estimate',
];
const SHARE_PAIRS: Pair[] = [
  ...(
    [
      ['share', 'B_RunShareA', 'r-kalos', 'XKalos', undefined],
      ['share-dark', 'B_RunShareDark', 'r-kalos', 'XKalos', 'dark'],
      ['share-multi', 'B_RunShareMultiA', 'r-carling', 'HCarling + HMaleficStar', undefined],
      ['share-multi-dark', 'B_RunShareMultiDark', 'r-carling', 'HCarling + HMaleficStar', 'dark'],
      ['phone-share', 'B_PhoneRunShareA', 'r-kalos', 'XKalos', undefined],
      ['phone-share-dark', 'B_PhoneRunShareDark', 'r-kalos', 'XKalos', 'dark'],
      ['phone-share-multi', 'B_PhoneRunShareMulti', 'r-carling', 'HCarling + HMaleficStar', undefined],
    ] as const
  ).map(([name, board, run, title, theme]): Pair => ({ name, board, path: '/', theme, scope: SHARE_SCOPE, setup: shareLookedUp(run), ready: sharePanel(run, title) })),
  ...(
    [
      ['mc-share', 'M_RunShare', 'r-kalos', undefined],
      ['mc-share-multi', 'M_RunShareMulti', 'r-carling', 'dark'],
      ['mc-phone-share', 'M_PhoneRunShare', 'r-carling', undefined],
    ] as const
  ).map(
    ([name, board, run, theme]): Pair => ({
      name,
      board,
      app: 'public',
      path: `/?run=${run}`,
      theme,
      scope: SHARE_PORTAL_SCOPE,
      setup: async (page) => {
        await boardRoster(page);
        await shareLookedUp(run)(page);
      },
      ready: async (page) => {
        await expect(page.locator('[data-fid="estimate-share"]')).toBeVisible();
        await runParty(page);
      },
    }),
  ),
];

// Per-boss picks v2 and the Party total (KANADE_MOCKUPS=docs/research/2026-10-16-per-boss-picks, then
// 2026-10-16-party-polish): the run sheet's Party grid over r-carling with Asahi's own HMaleficStar pick
// (AsahiMech) and Mika's timing pick for it (MikaNL) as the boards draw, after a party lookup; the Fixed
// editor's per-boss list; the portal's Party total block. The States boards are notes over fragments.
const GRID_SCOPE = [
  'week-party',
  'party-slot',
  'party-grid-boss',
  'party-grid-cell',
  'party-total',
  'party-total-row',
  'party-total-asof',
  'party-total-notes',
  'party-boss-same',
  'party-warn',
  'party-ready-rules',
  'party-pick-menu',
  'sheet-aside',
  'estimate-panel',
  'estimate-share',
];
const bossPicked = async (page: Page) => {
  const headers = await csrf(page.request);
  for (const [path, character] of [
    [`runs/r-carling/characters/${member(1001)}/bosses/MaleficStar`, 'ch-s6'],
    [`fixed/f-carling/characters/${member(1003)}/bosses/MaleficStar`, 'ch-s2'],
  ]) {
    const done = await page.request.put(`${ADMIN}/api/admin/${path}`, { headers, data: { character_id: character, version: 0 } });
    expect(done.ok(), `${path}: ${await done.text()}`).toBe(true);
  }
  await shareLookedUp('r-carling')(page);
};
const gridShown = (scroll?: 'estimate' | 'asahi-pick') => async (page: Page) => {
  await carlingSheet(page);
  // The cell's popover is a dialog named after the run too: take the sheet itself.
  const sheet = page.locator('dialog.runsheet[open]');
  await expect(sheet.locator('[data-fid="party-total"]')).toBeVisible();
  await expect(sheet.locator('[data-fid="estimate-share"]')).toBeVisible();
  if (scroll === 'asahi-pick') {
    await sheet.getByRole('button', { name: /^Asahi's character for HMaleficStar/ }).click();
    await expect(sheet.locator('[data-fid="party-pick-menu"]')).toBeVisible();
  }
  if (scroll === 'estimate')
    await sheet.locator('[data-fid="estimate-panel"]').evaluate((el, block) => el.scrollIntoView({ block }), page.viewportSize()!.width < 600 ? ('end' as const) : ('start' as const));
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
};
/** A board's `data-scroll-to`: each box around `el`, outermost first, scrolled to put it 8 px under the box's top. */
const scrollLikeBoard = (el: Element) => {
  const boxes: Element[] = [];
  for (let box = el.parentElement; box && box !== document.body; box = box.parentElement) boxes.unshift(box);
  for (const box of boxes) box.scrollTop = el.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop - 8;
};
const fixedBoss = (pick: boolean) => async (page: Page) => {
  await page.getByRole('button', { name: 'Edit Tuesday 22:00 — HCarling + HMaleficStar' }).click();
  const phone = page.viewportSize()!.width < 600;
  const editor = phone ? page.getByRole('dialog', { name: 'Tuesday 22:00 — HCarling + HMaleficStar' }) : page.getByRole('complementary', { name: 'Weekly timing details' });
  await editor.getByRole('tab', { name: /^Characters/ }).click();
  // As B_FixedBoss2 and the phone boards: Asahi's Per boss opened, Mika's list open with her HMaleficStar pick.
  await editor.getByRole('button', { name: "Pick Asahi's character per boss" }).click();
  const list = editor.locator('[data-fid="fixed-boss-picks"]').last();
  await expect(list).toBeVisible();
  // The phone boards and B_FixedBoss2Pick scroll to Mika's row (`data-scroll-to=".fixedready__row:nth-child(3)"`);
  // B_FixedBoss2 shows the pane from its top.
  if (phone || pick) await editor.locator('.fixedready__row').nth(2).evaluate(scrollLikeBoard);
  if (pick) {
    await list.getByRole('button', { name: /^Mika's character for HMaleficStar/ }).click();
    await expect(page.locator('[data-fid="fixed-pick-menu"]').first()).toBeVisible();
  }
};
/** Run in a board: the app chrome its capture left untagged (the page line, the phone top bar) gets the app's tags. */
const BOARD_CHROME = () => {
  for (const [selector, fid] of [
    ['.pageline', 'page-line'],
    ['.topbar', 'topbar'],
    ['.topbar__menu', 'topbar-menu'],
    ['.topbar__title', 'topbar-title'],
    ['.topbar__fresh', 'topbar-fresh'],
    ['.topbar__inbox', 'topbar-inbox'],
  ])
    for (const el of document.querySelectorAll<HTMLElement>(selector)) el.dataset.fid ??= fid;
};
/**
 * The per-boss-picks boards leave the frame they captured untagged: the page line and its Add key,
 * the phone top bar, the window's title bar and search, the list's column heads, every row but the
 * open one (the app tags each row), on the phone boards the editor sheet's head and foot, and they
 * tag readiness only where a per-boss list shows it: the single-character rows' basis, character,
 * values and bosses, and the desktop pick menu's options, carry the app's tags here.
 */
const fixedBoard = async (board: Page) => {
  await board.evaluate(BOARD_CHROME);
  await board.evaluate(() => {
    const tag = (el: Element | null | undefined, fid: string) => el instanceof HTMLElement && !el.dataset.fid && (el.dataset.fid = fid);
    const all = (selector: string, fid: string) => document.querySelectorAll(selector).forEach((el) => tag(el, fid));
    tag(document.querySelector('.pageline .fixed-add'), 'fixed-add');
    tag(document.querySelector('.fixed-window__head'), 'window-bar');
    tag(document.querySelector('.fixed-window__search'), 'window-search');
    tag(document.querySelector('[data-fid="fixed-list"] tr[role="row"]:not(.expandable-row)'), 'fixed-head');
    all('[data-fid="fixed-list"] tr.expandable-row', 'fixed-row');
    const sheet = document.querySelector('dialog[data-fid="fixed-editor"]');
    tag(sheet?.querySelector('.modal__head'), 'fixed-editor-head');
    tag(sheet?.querySelector('.modal__foot'), 'fixed-editor-foot');
    all('.fixedready__head .fixedready__basis', 'fixed-ready-basis');
    all('.fixedready__head .slotchar__chip', 'fixed-ready-char');
    all('.fixedready__row > .ready', 'fixed-ready-values');
    all('.fixedready__row > .ready > .ready__bosses', 'fixed-ready-bosses');
    all('[data-fid="fixed-pick-menu"] .charpick__option', 'fixed-pick-option');
  });
};
/**
 * The phone boards' subject is the editor sheet: the list behind it (and its title bar) stands where
 * the board's capture had scrolled it; desktop `fixed` (B_Fixed) compares the list.
 */
const PHONE_FIXED_OMIT = ['window-bar', 'fixed-list'];
const fixedBossSetup = async (page: Page) => {
  const done = await page.request.put(`${ADMIN}/api/admin/fixed/f-carling/characters/${member(1003)}/bosses/MaleficStar`, { headers: await csrf(page.request), data: { character_id: 'ch-s2', version: 0 } });
  expect(done.ok(), await done.text()).toBe(true);
};
const BOSS_PICK_PAIRS: Pair[] = [
  { name: 'boss-grid', board: 'B_RunBossPicks', path: '/', omit: STAGE_OMIT, scope: GRID_SCOPE, setup: bossPicked, ready: gridShown() },
  { name: 'boss-grid-dark', board: 'B_RunBossPicksDark', path: '/', theme: 'dark', omit: STAGE_OMIT, scope: GRID_SCOPE, setup: bossPicked, ready: gridShown() },
  { name: 'boss-grid-pick', board: 'B_RunBossPick', path: '/', omit: STAGE_OMIT, scope: GRID_SCOPE, setup: bossPicked, ready: gridShown('asahi-pick') },
  { name: 'boss-grid-estimate', board: 'B_RunBossEstimate', path: '/', omit: STAGE_OMIT, scope: GRID_SCOPE, setup: bossPicked, ready: gridShown('estimate') },
  { name: 'phone-boss-grid', board: 'B_PhoneRunBossPicks', path: '/', omit: STAGE_OMIT, scope: GRID_SCOPE, setup: bossPicked, ready: gridShown() },
  { name: 'phone-boss-grid-estimate', board: 'B_PhoneRunBossEstimate', path: '/', omit: STAGE_OMIT, scope: GRID_SCOPE, setup: bossPicked, ready: gridShown('estimate') },
  { name: 'fixed-boss', board: 'B_FixedBoss2', path: '/fixed', setup: fixedBossSetup, prepare: fixedBoard, ready: fixedBoss(false) },
  { name: 'fixed-boss-dark', board: 'B_FixedBoss2Dark', path: '/fixed', theme: 'dark', setup: fixedBossSetup, prepare: fixedBoard, ready: fixedBoss(false) },
  { name: 'fixed-boss-pick', board: 'B_FixedBoss2Pick', path: '/fixed', setup: fixedBossSetup, prepare: fixedBoard, ready: fixedBoss(true) },
  { name: 'phone-fixed-boss', board: 'B_PhoneFixedBoss', path: '/fixed', omit: PHONE_FIXED_OMIT, setup: fixedBossSetup, prepare: fixedBoard, ready: fixedBoss(false) },
  { name: 'phone-fixed-boss-pick', board: 'B_PhoneFixedBossPick', path: '/fixed', omit: PHONE_FIXED_OMIT, setup: fixedBossSetup, prepare: fixedBoard, ready: fixedBoss(true) },
  // The Party total (2026-10-16-party-polish v2): the admin grid's row, the portal's block.
  { name: 'party-total', board: 'B_PartyTotal', path: '/', omit: STAGE_OMIT, scope: GRID_SCOPE, setup: shareLookedUp('r-carling'), ready: gridShown() },
  { name: 'party-total-crit', board: 'B_PartyTotalCrit', path: '/', theme: 'dark', omit: STAGE_OMIT, scope: GRID_SCOPE, setup: shareLookedUp('r-carling'), ready: gridShown() },
  ...(
    [
      ['mc-party-total', 'M_PartyTotal', undefined],
      ['mc-party-total-crit', 'M_PartyTotalCrit', 'dark'],
      ['mc-phone-party-total', 'M_PhonePartyTotal', undefined],
    ] as const
  ).map(
    ([name, board, theme]): Pair => ({
      name,
      board,
      app: 'public',
      path: '/?run=r-carling',
      theme,
      scope: [...SHARE_PORTAL_SCOPE, 'party-total', 'party-total-row', 'party-total-asof', 'party-total-notes'],
      setup: async (page) => {
        await boardRoster(page);
        await shareLookedUp('r-carling')(page);
      },
      ready: async (page) => {
        await expect(page.locator('[data-fid="party-total"]')).toBeVisible();
        await runParty(page);
      },
    }),
  ),
];

const PAIRS: Pair[] = [
  { name: 'inbox-self', board: 'B_InboxSelf', path: '/inbox?tab=self_service&item=p-fa-request', ready: visible((page) => page.getByText('Changed since the member asked')) },
  {
    name: 'history',
    board: 'B_History',
    path: '/history',
    ready: async (page) => {
      await (await newestChange(page)).click();
      await expect(page.getByRole('complementary', { name: 'Change details' })).toBeVisible();
    },
  },
  { name: 'history-ck', board: 'B_HistoryCk', path: '/history', ready: opens((page) => page.getByRole('tab', { name: 'Checkpoints' }), (page) => page.getByRole('table', { name: /Backups/ })) },
  {
    name: 'fixed',
    board: 'B_Fixed',
    path: '/fixed',
    ready: opens((page) => page.getByRole('button', { name: /^Edit / }).first(), (page) => page.getByRole('complementary', { name: 'Weekly timing details' })),
  },
  { name: 'bosses', board: 'B_Bosses', path: '/bosses/MaleficStar/knowledge', ready: visible((page) => page.getByRole('heading', { level: 2, name: 'Radiant Malefic Star' })) },
  { name: 'phone-inbox', board: 'B_PhoneInbox', path: '/inbox?tab=extractor&item=p-bm-move', ready: visible((page) => page.getByRole('heading', { level: 2, name: /Move — Black Mage/ })) },
  {
    name: 'members',
    board: 'B_Members',
    path: '/members',
    // B_Members predates the roster module; B_RosterOff: with it off the sheet is exactly this one.
    setup: rosterOff,
    ready: opens((page) => page.getByRole('button', { name: /^Mika\b/ }), (page) => page.getByRole('complementary', { name: 'Member details' })),
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
    setup: chardataOff,
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
    setup: chardataOff,
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
  { name: 'roster-delete', board: 'B_RosterDelete', path: '/members', omit: ROSTER_FRAME, ready: (page) => rosterDelete(page, 'complementary', 'Member details') },
  { name: 'phone-roster-delete', board: 'B_PhoneRosterDelete', path: '/members', omit: PHONE_ROSTER_FRAME, ready: (page) => rosterDelete(page, 'dialog', 'Mika') },
  // B_RosterRunPick / B_PhoneRosterRunPick and B_CharReady drew the Party list before the grid
  // (superseded by B_RunBossPicks*, BOSS_PICK_PAIRS): their screen no longer exists, so no pair.
  // Character data (KANADE_MOCKUPS=docs/research/2026-10-12-chardata-mockups): the strip,
  // linked, asking to link, confirming an unlink, and with the estimate row (B_Char<board>,
  // B_PhoneChar<board>), each scrolled by the board's laptop and phone offsets.
  ...(
    [
      ['linked', 'Linked', scouterOff, 0, 150, undefined],
      [
        'link',
        'Link',
        async (page: Page) => {
          await scouterOff();
          await unlinked(page);
        },
        150,
        150,
        (strip: Locator) => strip.getByRole('button', { name: 'Link…' }).click(),
      ],
      ['unlink', 'Unlink', scouterOff, 230, 260, (strip: Locator) => strip.getByRole('button', { name: 'Unlink…' }).click()],
      ['estimate', 'Estimate', lookedUp({ character_ids: ['ch-s3'] }), 300, 520, (strip: Locator) => expect(strip.locator('[data-fid="estimate-result"]')).toBeVisible()],
    ] as const
  ).flatMap(([variant, board, setup, scroll, phoneScroll, act]) =>
    withPhone(
      { name: `chardata-${variant}`, board: `B_Char${board}`, path: '/members', omit: ROSTER_FRAME, setup, prepare: stripBoard(true), ready: stripOpen('complementary', 'Member details', scroll, act) },
      { omit: PHONE_ROSTER_FRAME, prepare: stripBoard(false), ready: stripOpen('dialog', 'Mika', phoneScroll, act) },
    ),
  ),
  // The run sheet's Estimate panel over HCarling + HMaleficStar (B_RosterRunPick's sheet, Ren (1) picked).
  ...withPhone({
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
  }),
  // Config › Character data: all ok, problems, no key, key refused. The phone boards draw their
  // own Config frame (no phone Config board was approved before them): those pairs compare the
  // section's contents (PHONE_CFG_SCOPE), not the shared frame.
  ...CHARDATA_HEALTH.flatMap(([variant, health]) => [
    {
      name: `cfg-chardata${variant ? `-${variant.toLowerCase()}` : ''}`,
      board: `B_CfgCharHealth${variant}`,
      path: '/config?section=character-data',
      setup: chardataHealth(health),
      ready: variant === 'Alert' ? healthEdited : healthShown,
    },
    {
      name: `phone-cfg-chardata${variant ? `-${variant.toLowerCase()}` : ''}`,
      board: `B_PhoneCfgCharHealth${variant}`,
      path: '/config?section=character-data',
      setup: chardataHealth(health),
      scope: [...PHONE_CFG_SCOPE, 'chardata-nexon-health', 'chardata-scouter-health', 'chardata-estimate-days', 'chardata-sweeper-health', 'chardata-env', 'chardata-post'],
      ready: variant === 'Alert' ? healthEdited : healthShown,
    },
  ]),
  { name: 'inbox-extractor', board: 'VarRail2', path: '/inbox?tab=extractor&item=p-bm-move', ready: visible((page) => page.getByRole('heading', { level: 2, name: /Move — Black Mage/ })) },
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
    ready: opens((page) => page.locator('[data-run="r-carling"] .plan-card__open'), (page) => page.getByRole('complementary', { name: 'HCarling + HMaleficStar' })),
  },
  { name: 'week-runs', board: 'B_WeekRuns', path: '/?week=next', ready: opens((page) => page.getByRole('tab', { name: /^Runs/ }), (page) => page.getByRole('table', { name: /Every run/ })) },
  { name: 'week-answers', board: 'B_WeekAnswers', path: '/?week=next', ready: opens((page) => page.getByRole('tab', { name: /^Answers/ }), (page) => page.getByRole('heading', { name: 'Still waiting' })) },
  {
    name: 'reminders',
    board: 'B_Reminders',
    path: '/reminders',
    // The "In" column and "today" read the browser clock: pin it to the server's.
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
  // Live Limits: the boards show three governor groups (cloud full and queueing, local
  // half-open, legacy open) or the one gateway group (`setLimitGroups`, canned on the harness).
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
        await setLimitGroups(groups);
        // The real server records no admission refusals yet (`AdmissionWindow::last_hour` is
        // always empty): each read gets the board's four, as limits.spec does.
        await page.route(`${ADMIN}/api/admin/limits`, async (route) => {
          const response = await route.fetch(unconditional(route));
          const body = (await response.json()) as Limits;
          const named = (at: number) => (body.groups[at] ?? body.groups[0])?.name ?? 'gateway';
          const ago = (minutes: number) => new Date(Date.parse(body.generated_at) - minutes * 60_000).toISOString().replace('.000Z', 'Z');
          body.admission.refusals = [
            { kind: 'rate', scope: 'group', target: named(1), count: 3, last_at: ago(6) },
            { kind: 'concurrency', scope: 'group', target: named(0), count: 1, last_at: ago(41) },
            { kind: 'quota', scope: 'key', target: 'gateway key …7f2a', count: 1, last_at: ago(22) },
            { kind: 'key_rate', scope: 'key', target: 'gateway key …7f2a', count: 2, last_at: ago(9) },
          ];
          await route.fulfill({ response, json: body });
        });
        await page.reload();
        const chosen = page.getByRole('tab', { name: new RegExp(`^${tab}`) });
        await chosen.click();
        await expect(chosen).toHaveAttribute('aria-selected', 'true');
        await expect(page.getByRole('tabpanel')).toBeVisible();
      },
    }),
  ),
  // The board shows the newest call open on Chat read (the default tab).
  { name: 'extract', board: 'B_Extract', path: '/extractions', ready: visible((page) => page.getByRole('list', { name: 'Messages read' }).getByRole('listitem').first()) },
  {
    name: 'extract-prompt',
    board: 'B_ExtractPrompt',
    path: '/extractions',
    ready: async (page) => {
      await page.getByRole('tab', { name: 'Prompt' }).click();
      await expect(page.getByRole('tabpanel', { name: 'Prompt' }).locator('pre')).toContainText('Messages:');
    },
  },
  { name: 'phone-week', board: 'B_PhoneWeek', path: '/', ready: visible((page) => page.locator('[data-run="r-carling"]')) },
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
  // c-when is the seed's masked turn: two rounds, one tool call and a stored Model view, as the boards show.
  { name: 'chat', board: 'B_Chat', path: '/chat/c-when', ready: visible((page) => page.getByRole('button', { name: 'Model view (masked)' })) },
  { name: 'chat-trace', board: 'B_ChatTrace', path: '/chat/c-when', ready: opens((page) => page.getByRole('tab', { name: /^Model trace/ }), (page) => page.getByText('Round 2', { exact: true })) },
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
    ready: opens((page) => page.locator('.navrail').getByRole('button', { name: 'More', exact: true }), (page) => page.getByRole('menu', { name: 'More destinations' })),
  },
  {
    name: 'rail-more-720',
    board: 'B_RailMore720',
    path: '/history',
    scope: ['rail', 'rail-more'],
    ready: visible((page) => page.locator('.navrail').getByRole('button', { name: 'More', exact: true })),
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
      // Signed out behind the page's back: the browser forgets its session cookies.
      await dropSessions(page.context());
      await page.goto(`${ADMIN}/login?sw=off`);
      await expect(page.getByRole('link', { name: 'Sign in with Discord' })).toBeVisible();
      await expect(page.getByText('Karling + Radiant Malefic Star')).toBeVisible();
    },
  },
  // The run sheet: the phone's full sheet, and the pane's "larger view" on a laptop.
  {
    name: 'hero-phone',
    board: 'HeroPhone',
    path: '/',
    setup: chardataOff,
    ready: opens((page) => page.locator('[data-run="r-carling"] .plan-card__open'), (page) => page.getByRole('dialog', { name: 'HCarling + HMaleficStar' })),
  },
  {
    name: 'hero-sheet',
    board: 'HeroSheet',
    path: '/',
    setup: chardataOff,
    ready: async (page) => {
      await page.locator('[data-run="r-carling"] .plan-card__open').click();
      await page.getByRole('complementary', { name: 'HCarling + HMaleficStar' }).getByRole('button', { name: 'Open in a larger view' }).click();
      await expect(page.getByRole('dialog', { name: 'HCarling + HMaleficStar' })).toBeVisible();
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
      await (await newestChange(page)).click();
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
  // has a clash to show: HCarling + HMaleficStar on Monday at HFA's 20:00.
  {
    name: 'move-pane',
    board: 'Main',
    path: '/?week=next',
    ready: async (page) => {
      await page.locator('[data-run="n-carling"] .plan-card__open').click();
      const pane = page.getByRole('complementary', { name: 'HCarling + HMaleficStar' });
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
      await page.getByRole('dialog', { name: 'HCarling + HMaleficStar' }).getByRole('button', { name: 'Move HCarling + HMaleficStar…' }).click();
      const view = page.getByRole('dialog', { name: 'Move HCarling + HMaleficStar' });
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

// Guides (KANADE_MOCKUPS=docs/research/2026-10-16-guide-mockups): the 20 single-screen boards over the
// guides board control (`/__e2e/guides/board`, examples/e2e_server/README.md), which draws G_List's six
// guides: Kalos basics published, Radiant Malefic Star out of date (art changed), Black Mage prep
// blocked (no Attach Files), Carling + Star night with message 4 unconfirmed, Limbo party roles
// publishing with its next post held, Weekly bossing etiquette not published. The composites
// G_EditorStates, G_PlanStates and G_RunStates are notes over fragments, left out (D19, lesson 92).
const GUIDE = {
  kalos: '5b1e8c2d-0f3a-4c6e-9d27-4a8b1c2d3e01',
  star: '5b1e8c2d-0f3a-4c6e-9d27-4a8b1c2d3e02',
  carling: '5b1e8c2d-0f3a-4c6e-9d27-4a8b1c2d3e03',
  limbo: '3d0c5b8a-7e21-4f3a-b6d9-2a4c8e1f5a10',
} as const;
const guidesBoard = async (page: Page) => {
  await control(page, 'guides/board');
};
/** The board state with Limbo party roles' held post let go, so its run finishes. */
const guidesReleased = async (page: Page) => {
  await guidesBoard(page);
  await control(page, 'discord/guide', { script: 'release' });
};
const guidePane = (page: Page) => page.locator('.guide-pane');
/**
 * The guide boards leave the dropdown triggers they captured (the channel, Add boss) untagged; the app's
 * Select tags each. G_ModuleOff's page line and the phone boards' top bar get the app's tags too.
 */
const guideBoard = async (board: Page) => {
  await board.evaluate(BOARD_CHROME);
  await board.evaluate(() => {
    for (const dd of document.querySelectorAll<HTMLElement>('button.dd:not([data-fid])')) dd.dataset.fid = 'dd-trigger';
  });
};
/** G_History is the captured History page with only its rows and pane tagged: its bar, tabs, filters, list, week heads and page line get the app's tags. */
const guidesHistoryBoard = async (board: Page) => {
  await guideBoard(board);
  await board.evaluate(() => {
    const tag = (selector: string, fid: string) => {
      for (const el of document.querySelectorAll<HTMLElement>(selector)) el.dataset.fid ??= fid;
    };
    tag('.history-window__head', 'window-bar');
    tag('.history-window__head .tabs__tabs', 'window-tabs');
    tag('.history-window__filters', 'window-filters');
    tag('.history-list-region__scroll', 'history-list');
    tag('.history-list-region__scroll .pane__section', 'history-group');
    tag('button.datepick-trigger', 'dr-trigger');
    tag('.history-detail__raw', 'history-raw');
  });
};
/** Row order is data: the store lists guides by title, the boards attention-first (Needs attention, Blocked, Publishing, …), so rows never pair up. */
const GUIDES_OMIT = ['guides-row'];
/**
 * Phones with a guide open: the window bar is hidden on both sides (the kit counts a hidden board
 * region as missing in the app), and the board leaves the hidden list out of its DOM.
 */
const PHONE_GUIDE_OMIT = [...GUIDES_OMIT, 'window-bar', 'guides-list'];
/**
 * The preview's embeds are the server's render (the v4 aliases line, the e2e knowledge leads); the
 * boards invent both, so the list's length and the note under it are data. Its first slots still pair.
 */
const PREVIEW_OMIT = [...GUIDES_OMIT, 'guide-preview-list', 'guide-preview-note'];
/**
 * The head's width follows its meta line: the boards name the published version ("published Mon 21:10
 * (version 12)"), which the API doesn't carry (a run summary has no version), and other versions and times.
 */
const META_OMIT = ['guide-title', 'guide-meta'];
/** The First Adversary has a knowledge lead in the e2e catalog where the boards draw none: the second entry's lead and the rows under it are data. */
const ENTRY_OMIT = ['guide-entry-lead', 'guide-entry-note', 'guide-entry-image'];
const animationsDone = (page: Page) => page.evaluate(() => Promise.all(document.getAnimations().map((a) => a.finished)));
/** The list (phone: the list alone). */
const guidesShown = async (page: Page) => {
  await expect(page.getByRole('listbox', { name: 'Guides' })).toBeVisible();
  await animationsDone(page);
};
/** The open guide's editor and, on wide screens, its pane with the preview or the run drawn. */
const guideOpen = async (page: Page) => {
  await expect(page.locator('.drafts__detail [data-fid="guide-editor"]')).toBeVisible();
  if (page.viewportSize()!.width >= 600) {
    await expect(guidePane(page).locator('.drafts__panehead')).toBeVisible();
    await expect(guidePane(page).locator('.gslot, .gplan__ops').first()).toBeVisible();
  } else await expect(page.locator('.guide-phonetabs').getByRole('tab', { name: /^Preview/ })).toHaveText(/ [1-9]\d* messages?$/);
  await animationsDone(page);
};
const openGuideBlock = async (page: Page, n: number) => {
  const summary = page.getByRole('button', { name: new RegExp(`^Edit block ${n}:`) });
  if (await summary.isVisible()) await summary.click();
};
/** Kalos basics with G_Editor's two unsaved changes (block 2's text grows, a Clear times block is added), block 2 open. */
const kalosEdited = async (page: Page) => {
  await guideOpen(page);
  await openGuideBlock(page, 2);
  const text = page.getByLabel('Block 2 text, Discord Markdown');
  await text.fill(`${await text.inputValue()}\n- Forgetting to answer the reminder, so the organiser cannot plan the party.`);
  await page.locator('.gblocks__add').getByRole('button', { name: 'Text' }).click();
  await page.getByLabel('Block 6 text, Discord Markdown').fill('## Clear times\nOur best Chaos clear this season is **14:52**. Post yours in #kalos-four after each run.');
  await openGuideBlock(page, 2);
  await expect(page.getByLabel('Block 2 text, Discord Markdown')).toBeVisible();
};
/** G_EditorBosses: the same edits with the bosses block open, both columns scrolled to it. */
const kalosBosses = async (page: Page) => {
  await kalosEdited(page);
  await openGuideBlock(page, 3);
  const open = page.locator('.gblock--open');
  await expect(open.locator('[data-fid="guide-entry"]').first()).toBeVisible();
  await open.evaluate((el) => el.scrollIntoView({ block: 'start' }));
  await guidePane(page).locator('.gslot').nth(2).evaluate((el) => el.scrollIntoView({ block: 'start' }));
};
const saveGuide = async (page: Page) => {
  await page.getByRole('button', { name: 'Save draft' }).click();
  await expect(page.getByRole('button', { name: page.viewportSize()!.width < 600 ? 'Publish…' : 'Review and publish…' })).not.toHaveAttribute('aria-disabled', 'true');
  // The boards draw no "saved" toast; one under the pointer stays (hover pauses it) and covers the phone's bar.
  await page.mouse.move(1, 1);
  await expect(page.locator('.toast')).toHaveCount(0, { timeout: 15_000 });
};
/** Kalos basics saved as G_Plan's version: block 2's text and Kalos's note change, Clear times is added. */
const kalosSaved = async (page: Page) => {
  await kalosEdited(page);
  await openGuideBlock(page, 3);
  const note = page.locator('.gblock--open .gentry__note textarea').first();
  await note.fill(`${await note.inputValue()} Bring a spare buffer.`);
  await saveGuide(page);
  await reopened(page);
};
/** The saved guide reopened, as the plan boards draw it behind the dialog: no block open, the editor at its top. */
const reopened = async (page: Page) => {
  await page.reload();
  await guideOpen(page);
};
/** Phones: block 2 open with the editor at its top (G_PhonePlan's backdrop). */
const phoneBlock2 = async (page: Page) => {
  await openGuideBlock(page, 2);
  await page.locator('.guide-phone > .drafts__main').evaluate((el) => (el.scrollTop = 0));
};
/** Radiant Malefic Star saved with its entry's large image, so messages 3–5 are posted again (G_PlanRepost). */
const starImage = async (page: Page) => {
  await guideOpen(page);
  await openGuideBlock(page, 3);
  await page.locator('.gblock--open .gentry__image').getByRole('checkbox').check();
  await saveGuide(page);
  await reopened(page);
};
const planOpen = async (page: Page, tick?: string) => {
  await page.getByRole('button', { name: page.viewportSize()!.width < 600 ? 'Publish…' : 'Review and publish…' }).click();
  const plan = page.getByRole('dialog', { name: /^(Publish|Move) / });
  await expect(plan.locator('.gplan__counts')).toBeVisible();
  if (tick) await plan.getByRole('checkbox', { name: tick }).check();
  await animationsDone(page);
};
const dialogShown = async (page: Page, name: string | RegExp) => {
  await expect(page.getByRole('alertdialog', { name })).toBeVisible();
  await animationsDone(page);
};
const GUIDE_PAIRS: Pair[] = (<Pair[]>[
  {
    name: 'guides-list',
    board: 'G_List',
    path: `/guides?item=${GUIDE.kalos}`,
    omit: PREVIEW_OMIT,
    setup: guidesBoard,
    ready: async (page) => {
      await guideOpen(page);
      await page.getByRole('button', { name: 'More, current page Guides' }).click();
      await expect(page.getByRole('menu', { name: 'More destinations' })).toBeVisible();
    },
  },
  { name: 'guides-list-dark', board: 'G_ListDark', path: `/guides?item=${GUIDE.kalos}`, theme: 'dark', omit: PREVIEW_OMIT, setup: guidesBoard, ready: guideOpen },
  {
    name: 'guides-editor',
    board: 'G_Editor',
    path: `/guides?item=${GUIDE.kalos}`,
    omit: PREVIEW_OMIT,
    setup: guidesBoard,
    ready: async (page) => {
      await kalosEdited(page);
      // The board's editor sits at its top, its preview scrolled to message 2.
      await page.locator('.guide-edit > .drafts__main').evaluate((el) => (el.scrollTop = 0));
      await guidePane(page).locator('.gslot--here').evaluate((el) => el.scrollIntoView({ block: 'start' }));
    },
  },
  { name: 'guides-editor-bosses', board: 'G_EditorBosses', path: `/guides?item=${GUIDE.kalos}`, omit: [...PREVIEW_OMIT, ...ENTRY_OMIT], setup: guidesBoard, ready: kalosBosses },
  { name: 'guides-editor-dark', board: 'G_EditorDark', path: `/guides?item=${GUIDE.kalos}`, theme: 'dark', omit: [...PREVIEW_OMIT, ...ENTRY_OMIT], setup: guidesBoard, ready: kalosBosses },
  {
    name: 'guides-plan',
    board: 'G_Plan',
    path: `/guides?item=${GUIDE.kalos}`,
    // The board's consequence counts the plan ("2 messages change, 1 is added"); the pane has no plan until the dialog reads one.
    omit: [...PREVIEW_OMIT, ...META_OMIT, 'decision'],
    setup: guidesBoard,
    ready: async (page) => {
      await kalosSaved(page);
      await planOpen(page);
    },
  },
  {
    name: 'guides-plan-repost',
    board: 'G_PlanRepost',
    path: `/guides?item=${GUIDE.star}`,
    omit: [...PREVIEW_OMIT, ...META_OMIT],
    setup: guidesBoard,
    ready: async (page) => {
      await starImage(page);
      await planOpen(page);
    },
  },
  {
    name: 'guides-plan-move',
    board: 'G_PlanMove',
    path: `/guides?item=${GUIDE.limbo}`,
    omit: [...PREVIEW_OMIT, ...META_OMIT],
    setup: guidesReleased,
    ready: async (page) => {
      await guideOpen(page);
      // Released before the page loaded: the finished run shows as the plain preview.
      await expect(guidePane(page).locator('.drafts__panehead .cap')).toHaveText(/^Preview/, { timeout: 15_000 });
      await choose(page.locator('.guide-channel').getByRole('combobox', { name: 'Channel' }), { label: /#guide-raids/ });
      await saveGuide(page);
      await planOpen(page);
    },
  },
  {
    name: 'guides-progress',
    board: 'G_Progress',
    path: `/guides?item=${GUIDE.limbo}`,
    setup: guidesBoard,
    ready: async (page) => {
      await guideOpen(page);
      await expect(page.locator('.guide-cancel')).toBeVisible();
    },
  },
  {
    name: 'guides-done',
    board: 'G_Done',
    path: `/guides?item=${GUIDE.limbo}`,
    // The board keeps G_Progress's "Publishing version 1" note after the run finished (one shared
    // LIMBO_DETAIL); the app shows "Saving never posts" once it is done.
    omit: [...GUIDES_OMIT, 'guide-live-note'],
    setup: guidesBoard,
    ready: async (page) => {
      await guideOpen(page);
      // The run finishes with the page open, as the board's admin watched it.
      await control(page, 'discord/guide', { script: 'release' });
      await expect(guidePane(page).getByRole('link', { name: 'Open in Discord' })).toBeVisible({ timeout: 15_000 });
      await animationsDone(page);
    },
  },
  {
    name: 'guides-unknown',
    board: 'G_Unknown',
    path: `/guides?item=${GUIDE.carling}`,
    omit: PREVIEW_OMIT,
    setup: guidesBoard,
    ready: async (page) => {
      await guideOpen(page);
      await expect(page.locator('.guide-unknown')).toBeVisible();
    },
  },
  {
    name: 'guides-unknown-confirm',
    board: 'G_UnknownConfirm',
    path: `/guides?item=${GUIDE.carling}`,
    omit: PREVIEW_OMIT,
    setup: guidesBoard,
    ready: async (page) => {
      await guideOpen(page);
      await page.getByRole('button', { name: 'Mark as not posted…' }).click();
      await dialogShown(page, /^Mark message \d+ as not posted\?/);
    },
  },
  {
    name: 'guides-delete',
    board: 'G_Delete',
    path: `/guides?item=${GUIDE.kalos}`,
    omit: PREVIEW_OMIT,
    setup: guidesBoard,
    ready: async (page) => {
      await guideOpen(page);
      await page.getByRole('button', { name: 'Delete guide…' }).click();
      await dialogShown(page, 'Delete Kalos basics?');
    },
  },
  {
    name: 'guides-history',
    board: 'G_History',
    path: '/history',
    setup: guidesBoard,
    prepare: guidesHistoryBoard,
    // Rows and week heads are History's data: the e2e seed adds a "Boss week of Thu 1 Oct" group (#1 system
    // (store)) above the guide rows, which the board's capture left out with its long seed rows, and the
    // board orders and words its six invented guide rows its own way. The window, filters and pane pair up.
    omit: [...GUIDES_OMIT, 'history-row', 'history-group'],
    ready: async (page) => {
      await page.locator('.history-row').filter({ hasText: 'Limbo party roles' }).filter({ hasText: 'publish requested' }).first().click();
      await expect(page.locator('.side-pane--history').getByRole('heading', { name: 'Limbo party roles: publish requested' })).toBeVisible();
      await page.locator('.history-list-region__scroll').evaluate((el) => (el.scrollTop = 0));
      await animationsDone(page);
    },
  },
  {
    name: 'guides-module-off',
    board: 'G_ModuleOff',
    path: '/guides',
    setup: () => setModules({ guides: false }),
    ready: async (page) => {
      await expect(page.getByRole('heading', { level: 1 })).toHaveText("That doesn't exist");
      await page.getByRole('button', { name: 'More' }).click();
      await expect(page.getByRole('menu', { name: 'More destinations' })).toBeVisible();
    },
  },
  { name: 'guides-phone-list', board: 'G_PhoneList', path: '/guides', omit: GUIDES_OMIT, setup: guidesBoard, ready: guidesShown },
  {
    name: 'guides-phone-editor',
    board: 'G_PhoneEditor',
    path: `/guides?item=${GUIDE.kalos}`,
    omit: [...PHONE_GUIDE_OMIT, ...META_OMIT],
    setup: guidesBoard,
    ready: async (page) => {
      await kalosEdited(page);
      await page.locator('.gblock--open').evaluate((el) => el.scrollIntoView({ block: 'start' }));
    },
  },
  {
    name: 'guides-phone-preview',
    board: 'G_PhonePreview',
    path: `/guides?item=${GUIDE.kalos}`,
    omit: [...PHONE_GUIDE_OMIT, 'guide-preview-list', 'guide-preview-note'],
    setup: guidesBoard,
    ready: async (page) => {
      await kalosEdited(page);
      await page.locator('.guide-phonetabs').getByRole('tab', { name: /^Preview/ }).click();
      await expect(page.locator('.gslot').first()).toBeVisible();
    },
  },
  {
    name: 'guides-phone-plan',
    board: 'G_PhonePlan',
    path: `/guides?item=${GUIDE.kalos}`,
    // Besides META_OMIT: the board's second op names which entry's note changed ("Gatekeeper Kalos: the
    // note changes"), a third line at phone width; the plan's ops carry no entry-level change.
    omit: [...PHONE_GUIDE_OMIT, ...META_OMIT, 'guide-plan-op'],
    setup: guidesBoard,
    ready: async (page) => {
      await kalosSaved(page);
      await phoneBlock2(page);
      await planOpen(page);
    },
  },
  {
    name: 'guides-phone-plan-repost',
    board: 'G_PhonePlanRepost',
    path: `/guides?item=${GUIDE.star}`,
    // The board draws Kalos basics' editor (PHONE_EDIT_MAIN) behind Radiant Malefic Star's plan.
    omit: [...PHONE_GUIDE_OMIT, 'guide-editor'],
    setup: guidesBoard,
    ready: async (page) => {
      await starImage(page);
      await phoneBlock2(page);
      await planOpen(page, 'Delete and repost messages 3–5');
    },
  },
]).map((pair) => ({ prepare: guideBoard, omit: GUIDES_OMIT, ...pair }));

test.describe('layout fidelity', () => {
  test.skip(!ENABLED, 'run with `bun run fidelity` (needs a KANADE_FIDELITY=1 build)');

  for (const pair of [...PAIRS, ...DRAFT_PAIRS, ...GRAPH_PAIRS, ...PRESENCE_PAIRS, ...PUBLIC_PAIRS, ...MEMBER_CHAR_PAIRS, ...SHARE_PAIRS, ...BOSS_PICK_PAIRS, ...GUIDE_PAIRS, ...TEASE_PAIRS].filter((p) => !ONLY.length || ONLY.includes(p.name))) {
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
      // The face the boards are drawn in: the admin boards in Nazuna, the portal's in Otonose (dark where the board is).
      await useLook(page, pair.app === 'public' ? 'marigold' : 'blossom', pair.theme ?? 'light');
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
