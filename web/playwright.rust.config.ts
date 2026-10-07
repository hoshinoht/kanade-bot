import { defineConfig } from '@playwright/test';

// Rust E2E owns one live store. Keep its ports above the mock range (base..+19):
// base 5073 therefore serves the admin and public apps on 5123 and 5124.
const base = Number(process.env.KANADE_E2E_PORT_BASE ?? '4373');
if (!Number.isInteger(base) || base < 1 || base > 65_484) throw new Error(`KANADE_E2E_PORT_BASE must be a valid port base, got ${process.env.KANADE_E2E_PORT_BASE}`);
const adminPort = base + 50;
const publicPort = base + 51;
if (publicPort > 65_535) throw new Error(`KANADE_E2E_PORT_BASE + 51 must be a valid port, got ${base}`);

export default defineConfig({
  testDir: './e2e/rust',
  outputDir: './e2e/.results-rust',
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
    command: 'bun e2e/rust/serve.ts',
    url: `http://127.0.0.1:${adminPort}/healthz`,
    // A shared or stray process would violate the temporary-store isolation.
    reuseExistingServer: false,
    timeout: 180_000,
    gracefulShutdown: { signal: 'SIGTERM', timeout: 30_000 },
    env: {
      KANADE_RUST_E2E_ADMIN_PORT: String(adminPort),
      KANADE_RUST_E2E_PUBLIC_PORT: String(publicPort),
    },
  },
});
