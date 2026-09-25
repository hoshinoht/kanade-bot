import { defineConfig } from '@playwright/test';

// Uses the installed Google Chrome (channel 'chrome'); no browser downloads.
// One worker: both origins share the mock server's in-memory week.
//
// Boss art: tests always use the synthetic fixtures under e2e/fixtures/boss.
// `KANADE_REAL_ART=1 bunx playwright test capture` instead serves the local,
// git-ignored repo-root boss/ art on separate ports for visual review only;
// its screenshots go to e2e/.captures/real/ (also git-ignored).
const real = process.env.KANADE_REAL_ART === '1';
export const MOCK_NOW = '2026-09-29T04:00:00Z';
// e2e owns its ports; dev servers use 4173/4174 (or anything else), never these.
const adminPort = real ? '4383' : '4373';
const publicPort = real ? '4384' : '4374';
process.env.KANADE_E2E_ADMIN = `http://127.0.0.1:${adminPort}`;
process.env.KANADE_E2E_PUBLIC = `http://127.0.0.1:${publicPort}`;

export default defineConfig({
  testDir: './e2e',
  outputDir: './e2e/.results',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [['list']],
  use: {
    channel: 'chrome',
    trace: 'retain-on-failure',
    viewport: { width: 1280, height: 800 },
  },
  webServer: {
    command: 'cargo run --quiet --release --manifest-path ../tools/pwa-mock/Cargo.toml',
    url: `http://127.0.0.1:${adminPort}/`,
    // Never drive a stray server: start our own, and the fixture checks it
    // is a pinned-clock mock (`/__mock/whoami`) before every test.
    reuseExistingServer: false,
    timeout: 180_000,
    env: {
      KANADE_WEB_DIR: '.',
      // Tuesday 29 Sep 2026, 12:00 in the guild's timezone: every date, countdown
      // and reminder state in the suite is fixed, whatever day it runs.
      KANADE_MOCK_NOW: MOCK_NOW,
      KANADE_BOSS_DIR: real ? '../boss' : 'e2e/fixtures/boss',
      ADMIN_PORT: adminPort,
      PUBLIC_PORT: publicPort,
    },
  },
});
