import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Account › This browser › "Reduce motion": on, the admin app moves exactly as
// under the device's `prefers-reduced-motion: reduce`; off, it follows the
// device (a device that reduces is never overridden). Kept in this browser
// (`kanade.motion`), applied before first paint as `<html data-motion="reduce">`,
// and live without a reload. Each state is checked against the same
// signature: CSS transitions (the press shape-morph and the blanket duration
// rule), scripted motion (Web Animations on a pane) and the wavy permit bars.

const KEY = 'kanade.motion';
const WIDE = { width: 1280, height: 800 };
const sw = (page: Page) => page.getByRole('tabpanel').getByRole('switch', { name: 'Reduce motion' });

/** Counts Web Animations started on the page (the scripted motion helpers use them). */
async function countAnimations(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as { __animated: number };
    w.__animated = 0;
    const original = Element.prototype.animate;
    Element.prototype.animate = function (this: Element, frames, options) {
      w.__animated += 1;
      return original.call(this, frames, options);
    };
  });
}
const animated = (page: Page) => page.evaluate(() => (window as unknown as { __animated: number }).__animated);
const resetAnimated = (page: Page) => page.evaluate(() => ((window as unknown as { __animated: number }).__animated = 0));

interface Signature {
  pressTransition: string;
  transitionDuration: string;
  paneAnimations: number;
  flatPermits: boolean;
}

const ys = (d: string) => new Set([...d.matchAll(/[ML][\d.]+ ([\d.]+)/g)].map((m) => m[1]!)).size;

/** How the app moves right now, reached by in-app navigation only (no reload). */
async function signature(page: Page): Promise<Signature> {
  const nav = page.getByRole('navigation', { name: 'Sections' });
  await nav.getByRole('link', { name: /^Fixed/ }).click();
  const key = page.locator('[data-fixed-add]');
  await expect(key).toBeVisible();
  const [pressTransition, transitionDuration] = await key.evaluate((el) => {
    const s = getComputedStyle(el);
    return [s.transitionProperty, s.transitionDuration];
  });

  await nav.getByRole('link', { name: /^Members/ }).click();
  await page.getByRole('button', { name: /^Tsubame/ }).first().click();
  await resetAnimated(page);
  await page.locator('.memberlist__row').nth(1).click();
  await expect(page.getByRole('complementary', { name: 'Member details' })).toBeVisible();
  await page.waitForTimeout(150); // "nothing starts" needs real time to pass
  const paneAnimations = await animated(page);

  await nav.getByRole('link', { name: /^Limits/ }).click();
  const bars = page.getByRole('progressbar', { name: / permits in use$/ });
  await expect(bars.first()).toBeVisible();
  await page.waitForTimeout(300); // let a drifting wave move if it is going to
  let flatPermits = true;
  for (const bar of await bars.all()) {
    const d = (await bar.locator('.wavy__wave').getAttribute('d')) ?? '';
    if (d && ys(d) > 1) flatPermits = false;
  }
  return { pressTransition, transitionDuration, paneAnimations, flatPermits };
}

async function openBrowserTab(page: Page) {
  await page.setViewportSize(WIDE);
  await page.goto(`${ADMIN}/account?tab=browser&sw=off`);
  await expect(sw(page)).toBeVisible();
}

const REDUCED = { transitionDuration: '1e-06s', paneAnimations: 0, flatPermits: true };

test.describe('Reduce motion switch', () => {
  test.beforeEach(async ({ page }) => countAnimations(page));

  test('off with the device at no preference: full motion', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await openBrowserTab(page);
    await expect(sw(page)).toHaveAttribute('aria-checked', 'false');
    const s = await signature(page);
    expect(s.pressTransition).toContain('border-radius');
    expect(s.transitionDuration).not.toBe('1e-06s');
    expect(s.paneAnimations).toBeGreaterThan(0);
    expect(s.flatPermits).toBe(false);
    expect(await page.evaluate(() => document.documentElement.dataset.motion)).toBeUndefined();
  });

  test('off with the device reducing: follows the device', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await openBrowserTab(page);
    await expect(sw(page)).toHaveAttribute('aria-checked', 'false');
    const s = await signature(page);
    expect(s).toMatchObject(REDUCED);
    expect(s.pressTransition).not.toContain('border-radius');
  });

  test('on with the device at no preference: exactly as the device reducing, live, then back when off', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await openBrowserTab(page);
    const device = await signature(page);

    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.goto(`${ADMIN}/account?tab=browser&sw=off`);
    await sw(page).click();
    await expect(sw(page)).toHaveAttribute('aria-checked', 'true');
    expect(await page.evaluate((k) => [document.documentElement.dataset.motion, localStorage.getItem(k)], KEY)).toEqual(['reduce', 'reduce']);
    // The switch's own knob already stops sliding.
    expect(await sw(page).locator('.switch__knob').evaluate((el) => getComputedStyle(el).transitionProperty)).toBe('none');
    const own = await signature(page);
    expect(own).toEqual(device);

    // Off again, still without a reload: full motion returns.
    await page.getByRole('button', { name: /^Account: Asahi/ }).click();
    await page.getByRole('menu', { name: 'Account' }).getByRole('menuitem', { name: 'Your account' }).click();
    await page.getByRole('tab', { name: /^This browser/ }).click();
    await sw(page).click();
    await expect(sw(page)).toHaveAttribute('aria-checked', 'false');
    expect(await page.evaluate((k) => [document.documentElement.dataset.motion ?? null, localStorage.getItem(k)], KEY)).toEqual([null, null]);
    const back = await signature(page);
    expect(back.pressTransition).toContain('border-radius');
    expect(back.paneAnimations).toBeGreaterThan(0);
    expect(back.flatPermits).toBe(false);
  });

  test('a stored choice applies before the app runs (no flash of motion) and survives a reload', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.addInitScript(() => {
      const w = window as unknown as { __early: string | null };
      // 'interactive' comes after the head's classic scripts and before any module script.
      document.addEventListener('readystatechange', () => {
        if (document.readyState === 'interactive') w.__early = document.documentElement.dataset.motion ?? null;
      });
    });
    await openBrowserTab(page);
    await sw(page).click();
    await page.reload();
    await expect(sw(page)).toHaveAttribute('aria-checked', 'true');
    expect(await page.evaluate(() => (window as unknown as { __early: string | null }).__early)).toBe('reduce');
    expect(await signature(page)).toMatchObject(REDUCED);
  });
});
