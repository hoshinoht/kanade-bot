import { defineConfig } from '@playwright/test';

// Uses the installed Google Chrome (channel 'chrome'); no browser downloads.
// Workers run in parallel, each against its own pwa-mock: the mock holds the
// week in memory and both origins share it, so a worker never shares a mock.
//
// Boss art: tests always use the synthetic fixtures under e2e/fixtures/boss.
// `KANADE_REAL_ART=1 bunx playwright test capture` instead serves the local,
// git-ignored repo-root boss/ art on separate ports for visual review only;
// its screenshots go to e2e/.captures/real/ (also git-ignored).
const real = process.env.KANADE_REAL_ART === '1';
export const MOCK_NOW = '2026-09-29T04:00:00Z';
// e2e owns its ports; dev servers use 4173/4174 (or anything else), never these.
// Port scheme (support.ts `origins()` reads the same base): worker i serves
// admin on base + 2i and public on base + 2i + 1, base = KANADE_E2E_PORT_BASE
// (default 4373), + 10 for real art. At most five workers, so a run stays in
// base..base+9 (real art base+10..base+19) and never reaches the next lane's
// base 100 above.
const MAX_WORKERS = 5;
const workers = Number(process.env.KANADE_E2E_WORKERS ?? '4');
if (!Number.isInteger(workers) || workers < 1 || workers > MAX_WORKERS)
  throw new Error(`KANADE_E2E_WORKERS must be 1..${MAX_WORKERS} (one mock port pair per worker), got ${process.env.KANADE_E2E_WORKERS}`);
const base = Number(process.env.KANADE_E2E_PORT_BASE ?? '4373') + (real ? 10 : 0);
process.env.KANADE_E2E_ORIGIN_BASE = String(base);
// Exactly motion.spec.ts (not reduce-motion.spec.ts).
const MOTION = /(^|[\\/])motion\.spec\.ts$/;

export default defineConfig({
  testDir: './e2e',
  // The Rust contract suite has its own live server, serial store and config.
  testIgnore: '**/rust/**',
  outputDir: './e2e/.results',
  fullyParallel: false,
  workers,
  retries: 0,
  reporter: [['list']],
  use: {
    channel: 'chrome',
    trace: 'retain-on-failure',
    viewport: { width: 1280, height: 800 },
  },
  // motion.spec.ts measures frame rates, so it runs one test at a time instead
  // of competing with itself for CPU. CI runs the two projects as separate
  // steps (ci.yml), so there motion also has the machine to itself.
  projects: [
    { name: 'main', testIgnore: [MOTION, '**/rust/**'] },
    { name: 'motion', testMatch: MOTION, workers: 1 },
  ],
  // Servers start one after another (Playwright awaits each before the next),
  // so the first `cargo run` builds the release mock and the rest only start it.
  webServer: Array.from({ length: workers }, (_, i) => {
    const adminPort = String(base + 2 * i);
    const publicPort = String(base + 2 * i + 1);
    return {
      command: 'cargo run --quiet --release --manifest-path ../devtools/pwa-mock/Cargo.toml',
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
    };
  }),
});
