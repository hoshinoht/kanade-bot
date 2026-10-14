import { ADMIN, CAPTURE_VIEWPORTS, LOOKS, expect, settle, test, useLook } from './support';

// The core primitives showcase (devtools/pwa-mock/showcase/): every core
// primitive once, captured per colourway into the git-ignored
// e2e/.captures/showcase/ for visual checks, under the admin origin's CSP
// (the fixture fails on any report). Dev-only: `bun run build && bun run
// showcase`, then `KANADE_SHOWCASE=1 bunx playwright test e2e/showcase.spec.ts`.
const ENABLED = process.env.KANADE_SHOWCASE === '1';
const OUT = 'e2e/.captures/showcase';

test.describe('core primitives showcase', () => {
  test.skip(!ENABLED, 'run with KANADE_SHOWCASE=1 after `bun run showcase`');
  test.describe.configure({ mode: 'parallel' });

  for (const vp of CAPTURE_VIEWPORTS) {
    for (const look of LOOKS) {
      test(`showcase ${vp.name} ${look.name}`, async ({ page }) => {
        await page.setViewportSize({ width: vp.width, height: vp.height });
        await useLook(page, look.colorway, look.theme);
        await page.goto(`${ADMIN}/__showcase/`);
        await expect(page.getByRole('heading', { level: 1, name: 'Core primitives' })).toBeVisible();
        await page.evaluate(() => document.fonts.ready);
        await settle(page);
        const tag = `${vp.name}-${look.name}`;
        await page.screenshot({ path: `${OUT}/showcase-${tag}.png`, fullPage: true, animations: 'disabled' });

        await page.getByRole('button', { name: 'Open the modal' }).click();
        await expect(page.getByRole('dialog', { name: 'Revert #2?' })).toBeVisible();
        await settle(page);
        await page.screenshot({ path: `${OUT}/showcase-modal-${tag}.png`, animations: 'disabled' });
      });
    }
  }
});
