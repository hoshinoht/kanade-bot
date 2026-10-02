import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Page } from '@playwright/test';
import { boardExists, composite, diff, markdown, MOCKUPS, renderBoard, settle, skeleton } from './fidelity-kit';
import { ADMIN, expect, test } from './support';

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
