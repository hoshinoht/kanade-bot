import type { Page } from '@playwright/test';
import { ADMIN, PUBLIC, expect, settle, test } from './support';

// docs/v5/pwa-design-guidelines.md "Area follows importance" (user rule): the
// one scrolling area keeps ≥ 55% of the viewport height and never less than
// 360 px where the viewport can hold that (below 655 px tall the 55% share
// is the floor), and the document itself never scrolls.
export const SIZES = [
  { width: 1280, height: 800 },
  { width: 1000, height: 670 },
  { width: 1280, height: 600 },
  { width: 390, height: 844 },
  { width: 844, height: 390 },
] as const;

/** The primary surface each page gives its area to, in priority order. */
const SURFACE = [
  '.week-surface',
  '.inbox__detail:not([hidden])',
  '.inbox__list:not([hidden])',
  '.settings__detail',
  '.tabs__panel:not([hidden])',
  '.pane__body',
  '.window-fill',
].join(', ');

export async function area(page: Page) {
  return page.evaluate((selector) => {
    const surface = [...document.querySelectorAll<HTMLElement>(selector)].find((el) => el.getClientRects().length > 0);
    const before = document.scrollingElement!.scrollTop;
    window.scrollTo(0, 400);
    const scrolled = document.scrollingElement!.scrollTop;
    window.scrollTo(0, before);
    return { height: surface ? surface.getBoundingClientRect().height : 0, name: surface?.className ?? 'none', scrolled };
  }, SURFACE);
}

export function budget(height: number): number {
  return Math.max(0.55 * height, height >= 655 ? 360 : 0);
}

const SCREENS: [string, string, string][] = [
  ['admin', ADMIN, '/'],
  ['admin', ADMIN, '/fixed'],
  ['admin', ADMIN, '/bosses'],
  ['admin', ADMIN, '/bosses/Carling/knowledge'],
  ['admin', ADMIN, '/members'],
  ['admin', ADMIN, '/reminders'],
  ['admin', ADMIN, '/inbox'],
  ['admin', ADMIN, '/extractions'],
  ['admin', ADMIN, '/extractions/x-kalos'],
  ['admin', ADMIN, '/chat'],
  ['admin', ADMIN, '/chat/c-move'],
  ['admin', ADMIN, '/chat/c-when'],
  ['admin', ADMIN, '/chat/c-safe-line'],
  ['admin', ADMIN, '/limits'],
  ['admin', ADMIN, '/history'],
  ['admin', ADMIN, '/config'],
  ['admin', ADMIN, '/config?section=models'],
  ['admin', ADMIN, '/config?section=persona'],
  ['admin', ADMIN, '/config?section=profanity'],
  ['public', PUBLIC, '/'],
];

for (const size of SIZES) {
  test(`layout ${size.width}×${size.height}: every screen keeps its area and never scrolls the document`, async ({ page }) => {
    test.setTimeout(120_000);
    await page.setViewportSize(size);
    const failures: string[] = [];
    for (const [app, origin, path] of SCREENS) {
      await page.goto(`${origin}${path}${path.includes('?') ? '&' : '?'}sw=off`);
      await expect(page.getByRole('heading').first()).toBeVisible();
      await settle(page);
      const got = await area(page);
      const need = budget(size.height);
      if (got.scrolled !== 0) failures.push(`${app}${path}: the document scrolled`);
      if (got.height < need) failures.push(`${app}${path}: ${Math.round(got.height)} px < ${Math.round(need)} (${got.name})`);
    }
    expect(failures).toEqual([]);
  });
}

// v4's board at 1280×800 starts at y≈378 (v4-live/v4-week-wide.png), so it
// shows ≈422 px; the same stack at 1000×670 leaves it ≈292 px.
test('week: the board sits under the window title bar and beats v4 at 1280×800 and 1000×670', async ({ page }) => {
  for (const [size, v4] of [
    [{ width: 1280, height: 800 }, 422],
    [{ width: 1000, height: 670 }, 292],
  ] as const) {
    await page.setViewportSize(size);
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    const board = (await page.locator('.week-surface').boundingBox())!;
    expect(board.height).toBeGreaterThanOrEqual(Math.max(v4, budget(size.height)));
    // B_WeekSel (gate G3): the board is the Week window's body, straight under its 48 px title bar.
    const bar = (await page.locator('.week-window__bar').boundingBox())!;
    expect(await page.locator('.week-surface').evaluate((el) => el.closest('.card')?.classList.contains('week-window'))).toBe(true);
    expect(bar.height).toBeLessThanOrEqual(48.5);
    expect(Math.abs(board.y - (bar.y + bar.height))).toBeLessThanOrEqual(2);
    // The title bar is one row: every visible control shares a line (nothing wraps below).
    const rows = await page.locator('.week-window__bar').evaluate((head) => {
      const items = [...head.children].flatMap((c) => (getComputedStyle(c).display === 'contents' ? [...c.children] : [c]));
      const rects = items.map((c) => c.getBoundingClientRect()).filter((r) => r.height > 0);
      return { lowestTop: Math.max(...rects.map((r) => r.top)), highestBottom: Math.min(...rects.map((r) => r.bottom)) };
    });
    expect(rows.lowestTop, `week header wraps at ${size.width}×${size.height}`).toBeLessThan(rows.highestBottom);
  }
});
