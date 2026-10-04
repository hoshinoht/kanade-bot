import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Page } from '@playwright/test';
import { boardExists, composite, diff, markdown, MOCKUPS, renderBoard, settle, skeleton } from './fidelity-kit';
import { ADMIN, expect, PINNED_NOW, test } from './support';

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
  /** Opens the state the board shows and waits for it. */
  ready: (page: Page) => Promise<void>;
}

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
    ready: async (page) => {
      await page.getByRole('button', { name: /^Mika/ }).click();
      await expect(page.getByRole('complementary', { name: 'Member details' })).toBeVisible();
    },
  },
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
      const current = await chat.inputValue();
      const values = await chat.locator('option').evaluateAll((options) => options.map((o) => (o as HTMLOptionElement).value));
      await chat.selectOption(values.find((v) => v !== current && v !== '') ?? values[0]!);
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
    ready: async (page) => {
      await page.locator('[data-run="r-carling"] .plan-card__open').click();
      await expect(page.getByRole('dialog', { name: 'HCarling + HStar' })).toBeVisible();
    },
  },
  {
    name: 'hero-sheet',
    board: 'HeroSheet',
    path: '/',
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
      const trigger = page.getByRole('group', { name: 'Filters' }).getByRole('button', { name: /^Dates/ });
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
      await page.getByRole('group', { name: 'Filters' }).getByRole('button', { name: /^Dates/ }).click();
      const sheet = page.getByRole('dialog', { name: 'Dates' });
      await sheet.getByRole('button', { name: 'Last boss week' }).click();
      await sheet.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
    },
  },
];

test.describe('layout fidelity', () => {
  test.skip(!ENABLED, 'run with `bun run fidelity` (needs a KANADE_FIDELITY=1 build)');

  for (const pair of PAIRS.filter((p) => !ONLY.length || ONLY.includes(p.name))) {
    test(pair.name, async ({ page, browser }) => {
      test.skip(!boardExists(pair.board), `no board ${pair.board}.dc.html under ${MOCKUPS}`);
      const dir = join(OUT, pair.name);
      mkdirSync(dir, { recursive: true });

      const board = await renderBoard(browser, pair.board);
      const boardRegions = await skeleton(board.page);
      const boardPng = await board.page.screenshot({ animations: 'disabled' });
      await board.page.close();

      await page.setViewportSize({ width: board.width, height: board.height });
      // The face the boards are drawn in.
      await page.addInitScript(() => {
        localStorage.setItem('colorway', 'blossom');
        localStorage.setItem('theme', 'light');
      });
      await page.goto(`${ADMIN}${pair.path}${pair.path.includes('?') ? '&' : '?'}sw=off`);
      await pair.ready(page);
      await settle(page);
      const appRegions = await skeleton(page);
      const appPng = await page.screenshot({ animations: 'disabled' });

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
