// Fidelity pairs for reminder teasing (KANADE_MOCKUPS=docs/research/2026-10-17-header-tease):
// the portal's Account › Profile row (on, off, off for everyone), Mika's Members sheet with the
// note saved and refused, and Config › Notifications' two header switches; desktop and phone.
// W_Wording is a sheet of sentences across several screens, not a screen: no pair (its words are
// pinned by roster.spec, public-characters.spec and public-privacy.spec).
import type { Locator, Page } from '@playwright/test';
import { ADMIN, ASAHI_SNOWFLAKE, csrf, expect, signInPublic } from './support';

/** fidelity.spec's `Pair`, which that spec keeps to itself; these pairs leave the last four out. */
interface TeasePair {
  name: string;
  board: string;
  path: string;
  app?: 'public';
  setup?: (page: Page) => Promise<void>;
  ready: (page: Page) => Promise<void>;
  scope?: string[];
  viewport?: { width: number; height: number };
  theme?: 'dark';
  omit?: string[];
  prepare?: (board: Page) => Promise<void>;
}

/** Mika's snowflake (the seed's member ids are Discord ids). */
const MIKA = '100000000000001003';
const JOKE = 'still owes the party a Pitched box from August';
const BAD_JOKE = 'MikaBish forgets her familiar every single week';

async function headers(page: Page, on: boolean) {
  const notifications = on ? { header_estimates: true, header_teasing: true } : { header_estimates: false };
  const reply = await page.request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(page.request), data: { notifications } });
  expect(reply.status(), await reply.text()).toBe(200);
}

async function tease(page: Page, member: string, on: boolean) {
  const reply = await page.request.patch(`${ADMIN}/api/admin/members/${member}`, { headers: await csrf(page.request), data: { tease_ok: on, running_joke: JOKE } });
  expect(reply.status(), await reply.text()).toBe(200);
}

/** The boards' "scrolled here": every scrolling ancestor brings the part to its top, 8 px below (the board runtime's rule). */
async function scrollHere(part: Locator) {
  await part.evaluate((el) => {
    for (let box = el.parentElement; box; box = box.parentElement) {
      const oy = getComputedStyle(box).overflowY;
      if ((oy === 'auto' || oy === 'scroll') && box.scrollHeight > box.clientHeight) box.scrollTop += el.getBoundingClientRect().top - box.getBoundingClientRect().top - 8;
    }
  });
}

/**
 * The boards tag only what they changed on the captured screens; the app also tags the parts around
 * them. Give the board the app's tags for the parents these pairs compare (`null` takes a tag off:
 * the phone Account panel the app leaves untagged).
 */
const retag = (tags: [selector: string, fid: string | null][]) => async (board: Page) => {
  await board.evaluate((list) => {
    for (const [selector, fid] of list)
      for (const el of document.querySelectorAll(selector)) {
        if (fid === null) el.removeAttribute('data-fid');
        else el.setAttribute('data-fid', fid);
      }
  }, tags);
};

// ── Portal Account › Profile ──
const PROFILE_STATES = [
  // [suffix, teasing on for everyone, Asahi opted in]
  ['', true, true],
  ['Off', true, false],
  ['GlobalOff', false, true],
] as const;

const profileShown = async (page: Page) => {
  await page.waitForLoadState('networkidle');
  const row = page.locator('main [data-fid="account-tease"]');
  await expect(row.getByRole('switch', { name: 'OK to be teased' })).toBeVisible();
  await expect(page.locator('main [data-fid="account-tease-note"]')).toBeVisible();
  await scrollHere(page.locator('main [data-fid="account-reach"]'));
};
// The app's own parts the boards leave untagged: the Characters tab, the allowance row, the note, the phone's Chat section.
const ACCOUNT_OMIT = ['account-tab-characters', 'account-allowance', 'account-note', 'account-chat'];

// ── Members › Mika › Profile ──
const sheetShown = (phone: boolean, refused: boolean) => async (page: Page) => {
  await page.getByRole('button', { name: /^Mika\b/ }).click();
  const sheet = phone ? page.getByRole('dialog', { name: 'Mika', exact: true }) : page.getByRole('complementary', { name: 'Member details' });
  const section = sheet.locator('[data-fid="members-tease"]');
  await expect(section.getByRole('switch', { name: 'OK to be teased' })).toHaveAttribute('aria-checked', 'true');
  await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  if (refused) {
    await section.getByRole('textbox', { name: 'Running-joke note' }).fill(BAD_JOKE);
    await section.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(section.getByRole('alert')).toContainText('is a roster IGN');
  }
  await scrollHere(section);
};
// The sheet's frame (members list, its head) is the Members pairs'; these compare the sheet's Profile in its pane
// (on wide screens the pane sits in the Members window, beside the list).
const SHEET_SCOPE = ['members-pane', 'members-tabs', 'members-facts', 'members-style', 'members-tease', 'members-tease-switch', 'members-tease-note', 'members-tease-error'];
// The pane's other parts, untagged on the boards.
const SHEET_OMIT = ['members-pane-head', 'members-close', 'members-mentions', 'members-aliases', 'members-runs', 'dd-trigger'];

// ── Config › Notifications ──
const cardsShown = (phone: boolean) => async (page: Page) => {
  const card = page.locator('[data-fid="cfg-estimates-headers"]');
  await expect(card).toBeVisible();
  if (phone) await scrollHere(card);
};
// Every SwitchCard's switch is `cfg-switch`: compare the two new cards whole, in their panel and window.
const CFG_SCOPE = ['page-line', 'window', 'window-bar', 'cfg-detail', 'cfg-panel', 'cfg-estimates-headers', 'cfg-tease-headers'];
// The window's other parts and the section's other cards, untagged on the boards.
const CFG_OMIT = ['window-search', 'cfg-toc', 'cfg-head', 'cfg-card'];

export const TEASE_PAIRS: TeasePair[] = [
  ...PROFILE_STATES.flatMap(([suffix, everyone, mine]) =>
    (['', 'Phone'] as const).map((phone) => ({
      name: `tease-${phone ? 'phone-' : ''}profile${suffix ? `-${suffix.toLowerCase()}` : ''}`,
      board: `M_${phone}ProfileTease${suffix}`,
      path: '/account',
      app: 'public' as const,
      setup: async (page: Page) => {
        await headers(page, everyone);
        await tease(page, ASAHI_SNOWFLAKE, mine);
        await signInPublic(page);
      },
      ready: profileShown,
      omit: ACCOUNT_OMIT,
      prepare: phone ? retag([['[data-fid="account-panel"]', null]]) : undefined,
    })),
  ),
  ...[false, true].flatMap((phone) =>
    [false, true].map((refused) => ({
      name: `tease-${phone ? 'phone-' : ''}members${refused ? '-refused' : ''}`,
      board: `B_${phone ? 'Phone' : ''}MemberTease${refused ? 'Refused' : ''}`,
      path: '/members',
      setup: (page: Page) => tease(page, MIKA, true),
      scope: phone ? SHEET_SCOPE : ['window', ...SHEET_SCOPE],
      omit: phone ? SHEET_OMIT : ['window-bar', 'members-list', ...SHEET_OMIT],
      prepare: retag(
        phone
          ? [['[data-fid="sheet"]', 'members-pane']]
          : [
              ['section.members-window', 'window'],
              ['aside.side-pane', 'members-pane'],
            ],
      ),
      ready: sheetShown(phone, refused),
    })),
  ),
  ...[false, true].flatMap((phone) =>
    [false, true].map((on) => ({
      name: `tease-${phone ? 'phone-' : ''}cfg${on ? '-on' : ''}`,
      board: `B_${phone ? 'Phone' : ''}ConfigHeaders${on ? 'On' : ''}`,
      path: '/config?section=notifications',
      setup: on ? (page: Page) => headers(page, true) : undefined,
      scope: CFG_SCOPE,
      omit: CFG_OMIT,
      prepare: retag([
        ['.settings__detail', 'cfg-detail'],
        ['.settings__scroll', 'cfg-panel'],
      ]),
      ready: cardsShown(phone),
    })),
  ),
];
