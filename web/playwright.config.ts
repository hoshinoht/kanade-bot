import { realpathSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { defineConfig } from '@playwright/test';

// Uses the installed Google Chrome (channel 'chrome'); no browser downloads.
// Every spec runs against the real kanade API: `examples/e2e_server` (built
// with `--features test-support`), the real routers composed as `serve`
// composes them over a temporary store, with harness controls under `/__e2e/`.
// Each worker drives its own server: worker i serves admin on base + 2i and
// public on base + 2i + 1 (support.ts reads the same scheme), base =
// KANADE_E2E_PORT_BASE (default 4373). At most five workers, so a run stays in
// base..base+9, clear of the Rust smoke config (base+50..) and the next lane's
// base 100 above. Servers start one after another, so the first `cargo run`
// builds the release example and the rest only start it.
// The suite's pinned server clock (support.ts `PINNED_NOW` must match).
const PINNED_NOW = '2026-09-29T04:00:00Z';
// `KANADE_REAL_ART=1` serves the local, git-ignored repo-root boss/ art
// (visual review and fidelity only), else the fixtures under e2e/fixtures/boss.
// The server serves no file through a symlink out of its art root, and a
// worktree links boss/portraits in from the main checkout: point at the real dir.
const BOSS_DIR = process.env.KANADE_REAL_ART === '1' ? ` --boss-dir ${dirname(realpathSync(join(import.meta.dirname, '..', 'boss', 'portraits')))}` : '';
const MAX_WORKERS = 5;
const workers = Number(process.env.KANADE_E2E_WORKERS ?? '4');
if (!Number.isInteger(workers) || workers < 1 || workers > MAX_WORKERS)
  throw new Error(`KANADE_E2E_WORKERS must be 1..${MAX_WORKERS} (one server per worker), got ${process.env.KANADE_E2E_WORKERS}`);
const base = Number(process.env.KANADE_E2E_PORT_BASE ?? '4373');
if (!Number.isInteger(base) || base < 1 || base > 65_526) throw new Error(`KANADE_E2E_PORT_BASE must be a valid port base, got ${process.env.KANADE_E2E_PORT_BASE}`);
process.env.KANADE_E2E_ORIGIN_BASE = String(base);
// Exactly motion.spec.ts (not reduce-motion.spec.ts): it measures frame rates,
// so it runs one test at a time instead of competing with itself for CPU. CI
// runs the two projects as separate steps (ci.yml), so there motion also has
// the machine to itself.
const MOTION = /(^|[\\/])motion\.spec\.ts$/;
// The Rust smoke suite has its own server and config (playwright.rust.config.ts).
const RUST = '**/rust/**';

export default defineConfig({
  testDir: './e2e',
  testIgnore: RUST,
  outputDir: './e2e/.results',
  fullyParallel: false,
  workers,
  // One retry on CI only: a test that passes on retry is reported "flaky" in
  // the list output instead of failing the job; locally a failure is final.
  retries: process.env.CI ? 1 : 0,
  reporter: [['list']],
  use: {
    channel: 'chrome',
    trace: 'retain-on-failure',
    viewport: { width: 1280, height: 800 },
  },
  projects: [
    { name: 'main', testIgnore: [MOTION, RUST] },
    { name: 'motion', testMatch: MOTION, workers: 1 },
  ],
  webServer: Array.from({ length: workers }, (_, i) => {
    const adminPort = base + 2 * i;
    const publicPort = adminPort + 1;
    return {
      command: `cargo run --quiet --locked --offline --release --example e2e_server --features test-support -- --admin-port ${adminPort} --public-port ${publicPort} --web-dir web --now ${PINNED_NOW}${BOSS_DIR}`,
      cwd: '..',
      url: `http://127.0.0.1:${adminPort}/__e2e/whoami`,
      // Never drive a stray server: the fixture also checks `/__e2e/whoami`.
      reuseExistingServer: false,
      // A cold release build of the crate.
      timeout: 900_000,
      // SIGTERM lets the server close its store and remove its temporary root.
      gracefulShutdown: { signal: 'SIGTERM', timeout: 10_000 },
    };
  }),
});
