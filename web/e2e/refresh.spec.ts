import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// A poll cycle must update in place: no page content remounted, no scroll
// reset, the Answers chart's canvas kept (it was destroyed and redrawn on
// every 15 s poll — the "random refresh with a flash").

/** Each poll's week reads as newer, as on a live server. */
async function livelyWeek(page: Page) {
  let n = 0;
  await page.route(/\/api\/admin\/(week|stats)(\?|$)/, async (route) => {
    const res = await route.fetch();
    const body = await res.json();
    n++;
    if (body.version !== undefined) {
      body.version += n;
      body.generated_at = `2026-09-29T04:${String(n).padStart(2, '0')}:00Z`;
    }
    if (body.per_day) body.per_day[0].answered += n;
    await route.fulfill({ response: res, json: body });
  });
}

async function refreshTimes(page: Page, times: number) {
  for (let i = 0; i < times; i++) {
    const polled = page.waitForResponse((r) => r.url().includes('/api/admin/week'));
    await page.keyboard.press('ControlOrMeta+k');
    await page.getByRole('dialog', { name: 'Command palette' }).getByRole('combobox').fill('Refresh now');
    await page.keyboard.press('Enter');
    await polled;
    await page.waitForTimeout(200);
  }
}

/** Marks what is on screen now and counts every element later removed from the page. */
async function watch(page: Page, selector: string) {
  await page.evaluate((sel) => {
    const w = window as unknown as { __removed: number; __kept: Element[] };
    w.__removed = 0;
    w.__kept = [...document.querySelectorAll(sel)];
    new MutationObserver((list) => {
      for (const m of list) for (const n of m.removedNodes) if (n instanceof Element) w.__removed++;
    }).observe(document.querySelector('main')!, { childList: true, subtree: true });
  }, selector);
}

async function survived(page: Page) {
  return page.evaluate(() => {
    const w = window as unknown as { __removed: number; __kept: Element[] };
    return { removed: w.__removed, kept: w.__kept.length > 0 && w.__kept.every((e) => e.isConnected) };
  });
}

for (const [name, path, selector, setup] of [
  ['week planner', '/', '[data-run]', null],
  ['week runs', '/', 'main table tbody tr', 'Runs'],
  ['week answers', '/', '.chart__canvas canvas', 'Answers'],
  ['chat log', '/chat', 'main table tbody tr', null],
  ['reminders', '/reminders', 'main table tbody tr', null],
  ['config', '/config', '.settings__panel', null],
] as const) {
  test(`poll cycles update ${name} in place, with no remount or scroll reset`, async ({ page }) => {
    await livelyWeek(page);
    await page.goto(`${ADMIN}${path}?sw=off`);
    await page.waitForLoadState('networkidle');
    if (setup) await page.getByRole('tab', { name: setup }).click();
    await page.locator(selector).first().waitFor();
    const scroller = page.locator('.shell');
    await scroller.evaluate((el) => el.scrollTo(0, 40));
    const before = await scroller.evaluate((el) => el.scrollTop);
    await watch(page, selector);
    await refreshTimes(page, 3);
    expect(await survived(page)).toEqual({ removed: 0, kept: true });
    expect(await scroller.evaluate((el) => el.scrollTop)).toBe(before);
  });
}
