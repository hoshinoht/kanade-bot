import type { Page } from '@playwright/test';
import { SCREENS, SIZES, screenUrl } from './frames';
import { ADMIN, PUBLIC, expect, settle, signInPublic, test } from './support';
import { auditText, type Finding } from './text-audit';

// General text-clipping check (docs/notes/design/verification.md "Measuring text
// in the app"): every admin and public screen, plus the main states behind a
// tab or a pick, at the five layout frames. Any clipped, cut, spilled or
// off-screen text fails unless ALLOW names it; an ellipsis needs an ALLOW entry too.

type Size = `${number}×${number}`;

interface Allow {
  /** Matches the text element or any ancestor of it. */
  selector: string;
  /** Screen names (see STATES / SCREENS paths) or '*'. */
  screens: string[] | '*';
  sizes: Size[] | '*';
  kinds: Finding['kind'][];
  reason: string;
}

const CHAT = ['admin/chat', 'admin/chat/c-move', 'admin/chat/c-when', 'admin/chat/c-safe-line'];
const INBOX = ['admin/inbox', 'admin/inbox?tab=self_service', 'admin/inbox (item open)'];
const EXTRACTIONS = ['admin/extractions', 'admin/extractions/x-kalos'];
const REWRITES = ['admin/rewrites', 'admin/rewrites?attempt=rw-over'];
const SECTIONS = ['pings', 'run-lengths', 'watching', 'chatbot', 'profanity', 'persona', 'models', 'self-service', 'notifications', 'digest', 'channels', 'rescan', 'access', 'theme', 'env'];
const CONFIG = ['admin/config', ...SECTIONS.map((key) => `admin/config?section=${key}`)];

/** The one allow-list. Keep every entry specific and give it a reason. */
const ALLOW: Allow[] = [
  {
    selector: '.skip',
    screens: '*',
    sizes: '*',
    kinds: ['hidden'],
    reason: 'the skip link waits above the frame until it takes focus (_base.scss .skip)',
  },
  {
    selector: '.knowledge-hero__meta',
    screens: '*',
    sizes: '*',
    kinds: ['ellipsis'],
    reason: "the knowledge header is one line (DR 2026-10-02): level · researched · source path; the path gives way, the title holds it whole",
  },
  {
    selector: '.knowledge-aside__others',
    screens: '*',
    sizes: '*',
    kinds: ['ellipsis'],
    reason: "a weekly timing's other bosses give way to its time and pill; titled, and the timing opens in Fixed",
  },
  {
    selector: '.modelstats__chips',
    screens: CHAT,
    sizes: ['1000×670'],
    kinds: ['hidden'],
    reason: 'ModelStats drops a chip that does not fit rather than cutting it; "+n models" lists every model (web/AGENTS.md)',
  },
  {
    selector: '.row-content__compact',
    screens: [...INBOX, 'admin/history'],
    sizes: '*',
    kinds: ['ellipsis', 'ellipsis-bare'],
    reason: "a list row's one-line summary; opening the row shows it in full",
  },
  {
    selector: '.extract-row__facts',
    screens: EXTRACTIONS,
    sizes: '*',
    kinds: ['ellipsis', 'ellipsis-bare'],
    reason: "a call row's one-line facts; the opened call shows each in full",
  },
  {
    selector: '.extract-row__facts',
    screens: REWRITES,
    sizes: '*',
    kinds: ['ellipsis', 'ellipsis-bare'],
    reason: "a rewrite row's one-line facts (stage, code, latency, tokens); the opened attempt shows each in full",
  },
  {
    selector: '.chat-row__q',
    screens: CHAT,
    sizes: '*',
    kinds: ['ellipsis'],
    reason: "a turn row's question (titled); the opened turn shows it in full",
  },
  {
    selector: '.pageline__context',
    screens: CONFIG,
    sizes: ['1000×670'],
    kinds: ['ellipsis-bare'],
    reason: "_page-line.scss: the page line's context gives way first",
  },
  {
    selector: '.settings__problem',
    screens: CONFIG,
    sizes: ['1000×670'],
    kinds: ['ellipsis-bare'],
    reason: 'the problem chip is itself the link to Channel access; its "· fix in Channel access" hint gives way (phones drop it)',
  },
  {
    selector: '.profile__open',
    screens: ['admin/config?section=persona'],
    sizes: '*',
    kinds: ['ellipsis-bare'],
    reason: "a profile's prompt preview is the button that opens the full prompt",
  },
  {
    selector: '.boss-stack__names, .chips .name',
    screens: ['admin/fixed'],
    sizes: '*',
    kinds: ['ellipsis'],
    reason: 'weekly-timing boss names and member chips ellipsise with their full name in a title',
  },
  {
    selector: '.proposal__threadhead .proposal__threadfact',
    screens: INBOX,
    sizes: '*',
    kinds: ['ellipsis', 'ellipsis-bare'],
    reason: "the thread head's channel and time span give way to the Used / All switch; each message shows its own time",
  },
  {
    selector: '.member-card__party',
    screens: ['public/ signed in (Week)'],
    sizes: '*',
    kinds: ['ellipsis'],
    reason: "a member card's one-line party gives way to the card's width (boards Main, PhoneWeek); titled with the whole party",
  },
];

interface Screen {
  name: string;
  url: string;
  /** Opens the state after the page has loaded (a tab, a row). */
  open?: (page: Page) => Promise<void>;
}

const STATES: Screen[] = [
  ...SCREENS.map(([app, origin, path]) => ({ name: `${app}${path}`, url: screenUrl(origin, path) })),
  ...SECTIONS.filter((key) => !SCREENS.some(([, , path]) => path === `/config?section=${key}`)).map((key) => ({
    name: `admin/config?section=${key}`,
    url: screenUrl(ADMIN, `/config?section=${key}`),
  })),
  { name: 'admin/inbox?tab=self_service', url: screenUrl(ADMIN, '/inbox?tab=self_service') },
  { name: 'admin/account?tab=sessions', url: screenUrl(ADMIN, '/account?tab=sessions') },
  { name: 'admin/account?tab=browser', url: screenUrl(ADMIN, '/account?tab=browser') },
  { name: 'public/?login_error=not_eligible (Denied)', url: screenUrl(PUBLIC, '/?login_error=not_eligible') },
  {
    name: 'public/ signed in (Week)',
    url: screenUrl(PUBLIC, '/'),
    open: async (page) => {
      await signInPublic(page);
      await page.reload();
      await expect(page.locator('main [data-run]').first()).toBeVisible();
    },
  },
  {
    name: 'public/account signed in (Account)',
    url: screenUrl(PUBLIC, '/account?tab=devices'),
    open: async (page) => {
      await signInPublic(page);
      await page.reload();
      await expect(page.getByText('This device')).toBeVisible();
    },
  },
  {
    name: 'admin/account (reply style picker)',
    url: screenUrl(ADMIN, '/account'),
    open: async (page) => {
      await page.locator('.account-window__panel button[aria-haspopup="dialog"]').click();
      await expect(page.getByRole('dialog', { name: 'Reply style' }).getByRole('radio').first()).toBeVisible();
    },
  },
  {
    name: 'admin/inbox (item open)',
    url: screenUrl(ADMIN, '/inbox'),
    open: async (page) => {
      await page.locator('[data-item]').first().click();
    },
  },
  {
    name: 'admin/ Runs tab',
    url: screenUrl(ADMIN, '/'),
    open: async (page) => {
      await page.getByRole('tab', { name: /^Runs/ }).click();
    },
  },
  {
    name: 'admin/ Answers tab',
    url: screenUrl(ADMIN, '/'),
    open: async (page) => {
      await page.getByRole('tab', { name: /^Answers/ }).click();
    },
  },
  // The boss guide on real content (tracked boss/knowledge): other bosses, a mission, the other tabs.
  ...['BM', 'Bellona', 'Kalos'].map((key) => ({ name: `admin/bosses/${key}/knowledge`, url: screenUrl(ADMIN, `/bosses/${key}/knowledge`) })),
  { name: 'admin/bosses/Carling/knowledge?difficulty=Destiny', url: screenUrl(ADMIN, '/bosses/Carling/knowledge?difficulty=Destiny') },
  { name: 'admin/bosses/Lotus/knowledge?difficulty=Champion', url: screenUrl(ADMIN, '/bosses/Lotus/knowledge?difficulty=Champion') },
  {
    name: 'admin/bosses/Carling/knowledge (scrolled, compact hero, HP closed)',
    url: screenUrl(ADMIN, '/bosses/Carling/knowledge'),
    open: async (page) => {
      await page.locator('.knowledge-detail__body').evaluate((body) => body.scrollTo(0, 400));
      await expect(page.locator('.knowledge-hero--compact')).toHaveCount(1);
    },
  },
  // HP is closed by default: these two open it so the breakdown is audited.
  { name: 'admin/bosses/Limbo/knowledge (HP open)', url: screenUrl(ADMIN, '/bosses/Limbo/knowledge'), open: async (page) => { await page.locator('.guide-hp__toggle').first().click(); await expect(page.locator('.guide-hp__phase').first()).toBeVisible(); } },
  { name: 'admin/bosses/Carling/knowledge?difficulty=Extreme (HP open)', url: screenUrl(ADMIN, '/bosses/Carling/knowledge?difficulty=Extreme'), open: async (page) => { await page.locator('.guide-hp__toggle').first().click(); await expect(page.locator('.guide-hp__phase').first()).toBeVisible(); } },
  { name: 'admin/bosses/Seren/knowledge?tab=phases', url: screenUrl(ADMIN, '/bosses/Seren/knowledge?tab=phases') },
  { name: 'admin/bosses/BM/knowledge?tab=phases', url: screenUrl(ADMIN, '/bosses/BM/knowledge?tab=phases') },
  { name: 'admin/bosses/Meilin/knowledge?tab=notes', url: screenUrl(ADMIN, '/bosses/Meilin/knowledge?tab=notes') },
  { name: 'admin/bosses/Carling/knowledge?tab=sources', url: screenUrl(ADMIN, '/bosses/Carling/knowledge?tab=sources') },
  {
    name: 'admin/bosses/BM/knowledge?tab=strategies (steps open)',
    url: screenUrl(ADMIN, '/bosses/BM/knowledge?tab=strategies'),
    open: async (page) => {
      await page.getByRole('button', { name: /^Show \d+ steps?$/ }).first().click();
    },
  },
  {
    name: 'admin/ run open',
    url: screenUrl(ADMIN, '/'),
    open: async (page) => {
      const card = page.locator('[data-run="r-carling"]').first();
      await card.scrollIntoViewIfNeeded();
      await card.click();
      await expect(page.locator('.week-pane, dialog[open]').first()).toBeVisible();
    },
  },
];

const sizeName = (s: { width: number; height: number }): Size => `${s.width}×${s.height}`;
const hits = (list: string[] | '*', value: string) => list === '*' || list.includes(value);

test.describe.configure({ mode: 'parallel' });

for (const screen of STATES) {
  test(`text clipping: ${screen.name}`, async ({ page }) => {
    test.setTimeout(90_000);
    const failures: string[] = [];
    for (const size of SIZES) {
      const at = sizeName(size);
      await page.setViewportSize(size);
      await page.goto(screen.url);
      await expect(page.getByRole('heading').first()).toBeVisible();
      await screen.open?.(page);
      await page.evaluate(() => document.fonts.ready);
      await settle(page);
      const allow = ALLOW.filter((a) => hits(a.screens, screen.name) && hits(a.sizes, at));
      const found = await auditText(page, allow.map((a) => a.selector));
      for (const f of found) {
        if (f.allowed.some((i) => allow[i]!.kinds.includes(f.kind))) continue;
        failures.push(`${at} ${f.kind.padEnd(13)} ${f.path} "${f.text}" (${f.detail})`);
      }
    }
    expect(failures, `clipped text on ${screen.name}`).toEqual([]);
  });
}

// Positive control: the audit must catch each kind it claims to (built with CSSOM, as CSP requires).
test('text clipping: the audit catches a clip, a cut, a spill, an ellipsis and off-screen text', async ({ page }) => {
  await page.goto(screenUrl(PUBLIC, '/'));
  await expect(page.getByRole('heading').first()).toBeVisible();
  await page.evaluate(() => {
    const add = (parent: HTMLElement, text: string, css: Record<string, string>) => {
      const el = document.createElement('div');
      el.textContent = text;
      for (const [key, value] of Object.entries(css)) el.style.setProperty(key, value);
      parent.append(el);
      return el;
    };
    const host = add(document.body, '', { position: 'fixed', top: '0', left: '0', 'z-index': '9' });
    const long = 'A long line of words that cannot fit';
    add(host, long, { width: '60px', overflow: 'hidden', 'white-space': 'nowrap' }).className = 'probe-clip';
    const clipper = add(host, '', { width: '60px', overflow: 'clip' });
    const inner = document.createElement('span');
    inner.className = 'probe-cut';
    inner.textContent = long;
    inner.style.setProperty('white-space', 'nowrap');
    clipper.append(inner);
    add(host, long, { width: '60px', 'white-space': 'nowrap' }).className = 'probe-spill';
    add(host, long, { width: '60px', overflow: 'hidden', 'white-space': 'nowrap', 'text-overflow': 'ellipsis' }).className = 'probe-ellipsis';
    add(document.body, 'Past the right edge', { position: 'fixed', top: '200px', left: 'calc(100vw - 20px)', 'white-space': 'nowrap' }).className = 'probe-off';
  });
  const kinds = (await auditText(page, [])).filter((f) => f.path.includes('probe-')).map((f) => `${f.path.split('.').pop()} ${f.kind}`);
  expect(kinds).toEqual(expect.arrayContaining(['probe-clip clip-x', 'probe-cut cut', 'probe-spill spill', 'probe-ellipsis ellipsis-bare', 'probe-off off-screen']));
});

// Negative control: spaces that hang past a `pre-wrap` line draw nothing, so
// they are not a spill; a visible word that runs past the same box still is.
test('text clipping: hanging pre-wrap spaces are not a spill, visible overflow still is', async ({ page }) => {
  await page.goto(screenUrl(PUBLIC, '/'));
  await expect(page.getByRole('heading').first()).toBeVisible();
  const hang = await page.evaluate(() => {
    const host = document.createElement('div');
    for (const [key, value] of Object.entries({ position: 'fixed', top: '0', left: '0', 'z-index': '9' })) host.style.setProperty(key, value);
    document.body.append(host);
    const add = (className: string, text: string) => {
      const el = document.createElement('div');
      el.className = className;
      el.textContent = text;
      for (const [key, value] of Object.entries({ width: '80px', 'white-space': 'pre-wrap', 'font-size': '16px' })) el.style.setProperty(key, value);
      host.append(el);
      return el;
    };
    const hanging = add('probe-hang', `Calm${' '.repeat(40)}words`);
    add('probe-word', `Calm${' '.repeat(40)}Supercalifragilistic`);
    // The premise: the whole text node's extent (spaces included) does run past the box.
    const range = document.createRange();
    range.selectNodeContents(hanging.firstChild!);
    return Math.max(...[...range.getClientRects()].map((r) => r.right)) - hanging.getBoundingClientRect().right;
  });
  expect(hang, 'the hanging spaces overflow the box').toBeGreaterThan(1);
  const kinds = (await auditText(page, [])).filter((f) => f.path.includes('probe-h') || f.path.includes('probe-w')).map((f) => `${f.path.split('.').pop()} ${f.kind}`);
  expect(kinds).toEqual(['probe-word spill']);
});
