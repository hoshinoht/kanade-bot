import type { Page, Route } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Design experiments (pwa-design-guidelines "Experiments"): A, the morphing
// loading indicator on short waits; B, the wavy rescan progress. Off
// (`?experiments=off`) must look and behave exactly as before. Before/after
// captures go to the git-ignored e2e/.captures/experiments/.
const OUT = 'e2e/.captures/experiments';

async function go(page: Page, path: string, on = true) {
  const sep = path.includes('?') ? '&' : '?';
  await page.goto(`${ADMIN}${path}${sep}sw=off&experiments=${on ? 'on' : 'off'}`);
}

/** Holds every config PATCH until released, so a pending state can be seen. */
async function holdSaves(page: Page) {
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  await page.route(`${ADMIN}/api/admin/config`, async (route: Route) => {
    if (route.request().method() === 'PATCH') await gate;
    await route.continue();
  });
  return release;
}

async function startRescan(page: Page, on: boolean) {
  await go(page, '/extractions', on);
  await page.getByText('Re-read the party channels').click();
  const boxes = page.getByRole('group', { name: 'Channels' }).getByRole('checkbox');
  for (const box of await boxes.all()) await box.check();
  await page.getByRole('button', { name: 'Re-read' }).click();
}

const pings = (page: Page) => page.locator('.settings__panel:not([hidden])');
/** The save bar's key is enabled only with a change to save. */
async function change(page: Page) {
  await pings(page).getByRole('textbox', { name: 'Morning ping' }).fill('08:45');
}

test('A: a Config save shows the loading indicator in its button, then returns', async ({ page }) => {
  const release = await holdSaves(page);
  await go(page, '/config?section=pings');
  await expect(page.locator('html')).toHaveAttribute('data-experiments', 'on');
  const panel = pings(page);
  await change(page);
  await panel.getByRole('button', { name: 'Save pings', exact: true }).click();
  const pending = panel.getByRole('button', { name: 'Saving…' });
  await expect(pending).toBeVisible();
  await expect(pending.locator('.xp-loading[role="status"][aria-label="Saving…"]')).toBeVisible();
  // The hidden label keeps the button's size.
  const box = await pending.boundingBox();
  await pending.locator('.xp-loading').screenshot({ path: `${OUT}/A-indicator-closeup.png` });
  await panel.locator('form').first().screenshot({ path: `${OUT}/A-after-config-save-pending.png` });
  release();
  const idle = panel.getByRole('button', { name: 'Save pings', exact: true });
  await expect(idle).toBeVisible();
  await expect(idle.locator('.xp-loading')).toHaveCount(0);
  expect((await idle.boundingBox())?.width).toBeCloseTo(box!.width, 0);
});

test('A off: a pending Config save keeps today\'s plain button', async ({ page }) => {
  const release = await holdSaves(page);
  await go(page, '/config?section=pings', false);
  await expect(page.locator('html')).toHaveAttribute('data-experiments', 'off');
  const panel = pings(page);
  await change(page);
  await panel.getByRole('button', { name: 'Save pings', exact: true }).click();
  await page.waitForTimeout(300);
  await expect(panel.getByRole('button', { name: 'Save pings', exact: true })).toBeVisible();
  await expect(page.locator('.xp-loading')).toHaveCount(0);
  await panel.locator('form').first().screenshot({ path: `${OUT}/A-before-config-save-pending.png` });
  release();
});

test('A: reduced motion shows a still shape', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const release = await holdSaves(page);
  await go(page, '/config?section=pings');
  await change(page);
  await pings(page).getByRole('button', { name: 'Save pings', exact: true }).click();
  const shape = page.locator('.xp-loading__shape');
  await expect(shape).toBeVisible();
  const names = await page.evaluate(() => {
    const s = document.querySelector('.xp-loading__shape')!;
    return [getComputedStyle(s).animationName, getComputedStyle(s.parentElement!).animationName];
  });
  expect(names).toEqual(['none', 'none']);
  release();
});

test('A: Reload profiles and the inbox reject show it too', async ({ page }) => {
  let releaseReload!: () => void;
  const reloadGate = new Promise<void>((r) => (releaseReload = r));
  await page.route(`${ADMIN}/api/admin/config/profiles/reload`, async (route) => {
    await reloadGate;
    await route.continue();
  });
  await go(page, '/config?section=persona');
  await page.getByRole('button', { name: 'Reload profiles' }).click();
  await expect(page.getByRole('button', { name: 'Reloading…' }).locator('.xp-loading')).toBeVisible();
  releaseReload();
  await expect(page.getByRole('button', { name: 'Reload profiles' })).toBeVisible();

  let releaseReject!: () => void;
  const rejectGate = new Promise<void>((r) => (releaseReject = r));
  await page.route(/\/api\/admin\/inbox\/[^/]+\/reject$/, async (route) => {
    await rejectGate;
    await route.continue();
  });
  await go(page, '/inbox');
  await page.getByRole('button', { name: 'Reject…' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Reject change' }).click();
  await expect(page.getByRole('dialog').getByRole('button', { name: 'Rejecting…' }).locator('.xp-loading')).toBeVisible();
  releaseReject();
  await expect(page.getByRole('dialog')).toBeHidden();
});

test('B: a rescan shows the wavy progress bar, which settles flat when done', async ({ page }) => {
  await startRescan(page, true);
  const bar = page.getByRole('progressbar', { name: 'Rescan progress' });
  await expect(bar).toBeVisible();
  await expect(bar).toHaveAttribute('aria-valuemin', '0');
  await expect(bar).toHaveAttribute('aria-valuenow', '1', { timeout: 5_000 });
  const max = Number(await bar.getAttribute('aria-valuemax'));
  expect(max).toBeGreaterThan(1);
  await expect(bar).toHaveAttribute('aria-valuetext', `1 of ${max} channels read`);
  // Mid-run the fill is a moving wave, and the flat track remains.
  const wave = bar.locator('.xp-wavy__wave');
  const first = await wave.getAttribute('d');
  await page.waitForTimeout(200);
  expect(await wave.getAttribute('d')).not.toBe(first);
  expect(new Set(ys(first!)).size).toBeGreaterThan(1);
  expect(await bar.locator('.xp-wavy__track').getAttribute('d')).toBeTruthy();
  await page.locator('.rescan').screenshot({ path: `${OUT}/B-after-rescan-running.png` });
  await expect(page.locator('.rescan__status')).toHaveText(/Done: \d+ channels read/, { timeout: 15_000 });
  await expect(bar).toHaveAttribute('aria-valuenow', String(max));
  await expect.poll(async () => new Set(ys((await wave.getAttribute('d'))!)).size, { timeout: 5_000 }).toBe(1);
  await page.locator('.rescan').screenshot({ path: `${OUT}/B-after-rescan-done.png` });
});

test('B off: the rescan keeps its text-only progress', async ({ page }) => {
  await startRescan(page, false);
  await expect(page.locator('.rescan__status')).toHaveText(/Reading 1 of \d+ channels/, { timeout: 5_000 });
  await expect(page.getByRole('progressbar')).toHaveCount(0);
  await page.locator('.rescan').screenshot({ path: `${OUT}/B-before-rescan-running.png` });
});

test('B: reduced motion draws a flat, still bar', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await startRescan(page, true);
  const bar = page.getByRole('progressbar', { name: 'Rescan progress' });
  await expect(bar).toHaveAttribute('aria-valuenow', '1', { timeout: 5_000 });
  const wave = bar.locator('.xp-wavy__wave');
  await expect.poll(async () => (await wave.getAttribute('d')) ?? '').not.toBe('');
  const d = (await wave.getAttribute('d'))!;
  expect(new Set(ys(d)).size).toBe(1);
  await page.waitForTimeout(200);
  expect(await wave.getAttribute('d')).toBe(d);
});

test('the palette turns the experiments off and on', async ({ page }) => {
  await go(page, '/');
  await page.keyboard.press('ControlOrMeta+k');
  await page.getByRole('option', { name: /Turn design experiments off/ }).click();
  await expect(page.locator('html')).toHaveAttribute('data-experiments', 'off');
  expect(await page.evaluate(() => localStorage.getItem('kanade.experiments'))).toBe('off');
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('html')).toHaveAttribute('data-experiments', 'off');
  await page.keyboard.press('ControlOrMeta+k');
  await page.getByRole('option', { name: /Turn design experiments on/ }).click();
  await expect(page.locator('html')).toHaveAttribute('data-experiments', 'on');
});

function ys(d: string): string[] {
  return [...d.matchAll(/[ML][\d.]+ ([\d.]+)/g)].map((m) => m[1]!);
}
