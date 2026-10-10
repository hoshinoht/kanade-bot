import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, HEADING, PUBLIC, REAL_ART, expect, settle, signInPublic, test } from './support';

// The member portal's Bosses (catalog C7/C8): the admin Bosses window, shared
// through @kanade/ui, over `/api/public/bosses*`. List → guide → every tab, on
// desktop and in the phone frame; reads only member routes and never the
// bot-only `detail` wording. Every test ends with zero CSP/TT reports (`csp`).

test.describe.configure({ mode: 'parallel' });
test.skip(REAL_ART, 'fixture-specific assertions');

const TABS = ['Overview', 'Phases', 'Strategies', 'Notes', 'Sources'] as const;

async function open(page: Page, path: string) {
  await signInPublic(page);
  await page.goto(`${PUBLIC}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

/** Every API path the page asked for. */
function apiPaths(page: Page): string[] {
  const seen: string[] = [];
  page.on('request', (request) => {
    const { pathname } = new URL(request.url());
    if (pathname.startsWith('/api/')) seen.push(pathname);
  });
  return seen;
}

async function serious(page: Page) {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
  return result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => `${v.id}: ${v.nodes.length}`);
}

async function everyTab(page: Page) {
  const strip = page.getByRole('tablist', { name: 'Guide sections' });
  // The strip sits below the fold: bring it up first and let the hero finish collapsing (a moving target never clicks).
  await strip.scrollIntoViewIfNeeded();
  await settle(page);
  for (const name of TABS) {
    await strip.getByRole('tab', { name: new RegExp(`^${name}`) }).click();
    await expect(strip.getByRole('tab', { name: new RegExp(`^${name}`) })).toHaveAttribute('aria-selected', 'true');
    const panel = page.locator(`.guide-panel[data-tab="${name.toLowerCase()}"]`);
    await expect(panel).toBeVisible();
    await expect(panel).not.toBeEmpty();
    if (name !== 'Overview') await expect(page).toHaveURL(new RegExp(`tab=${name.toLowerCase()}`));
  }
}

test('desktop: the masthead opens Bosses, a row opens its guide, every tab shows', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const paths = apiPaths(page);
  await open(page, '/');
  await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: 'Bosses' }).click();
  await expect(page).toHaveURL(`${PUBLIC}/bosses`);
  await expect(page.getByRole('heading', { level: 1, name: 'Bosses' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Bosses' })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('heading', { name: 'Pick a boss to read its guide' })).toBeVisible();
  // Event bosses are listed and marked by season (Q9).
  const events = page.getByRole('list', { name: 'Event bosses' });
  await expect(events.getByRole('listitem').filter({ hasText: 'Kai' }).locator('.status-chip')).toHaveText('Seasonal boss · CW3');

  await page.getByRole('link', { name: 'Carling', exact: true }).click();
  await expect(page).toHaveURL(`${PUBLIC}/bosses/Carling`);
  await expect(page.getByRole('heading', { level: 2, name: /^Carling/ })).toBeVisible();
  await expect(page.getByText('Boss guide', { exact: true })).toBeVisible();
  // No repository path and no weekly-timings aside: the aside is "On this page" only.
  await expect(page.locator('.knowledge-hero__meta')).not.toContainText('boss/knowledge');
  const aside = page.getByRole('complementary', { name: 'On this page' });
  await expect(aside.getByRole('navigation', { name: 'On this page' })).toBeVisible();
  await expect(aside).not.toContainText('Weekly timings');
  await expect(page.getByRole('group', { name: 'Difficulty' }).getByRole('button', { name: /^Destiny/ })).toBeVisible();
  await everyTab(page);

  // Destiny: the mission card; Back returns to the catalog.
  await page.getByRole('group', { name: 'Difficulty' }).getByRole('button', { name: /^Destiny/ }).click();
  await expect(page.locator('#guide-mission-heading')).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(`${PUBLIC}/bosses`);
  expect(await serious(page)).toEqual([]);

  // Only member routes: never an admin one.
  expect(paths.filter((path) => path.startsWith('/api/admin'))).toEqual([]);
  expect(paths).toContain('/api/public/bosses');
  expect(paths).toContain('/api/public/bosses/Carling/knowledge');
});

test('phone: the drawer opens Bosses, a guide fills the frame with a back step, every tab shows', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await open(page, '/');
  await page.getByRole('button', { name: 'Open the navigation' }).click();
  await page.getByRole('dialog', { name: 'Navigation' }).getByRole('link', { name: 'Bosses' }).click();
  await expect(page).toHaveURL(`${PUBLIC}/bosses`);
  await expect(page.locator('.topbar__title')).toHaveText('Bosses');
  // One pane: the catalog alone.
  await expect(page.getByRole('navigation', { name: 'Boss catalog' })).toBeVisible();
  await expect(page.locator('.knowledge-detail')).toBeHidden();

  await page.getByRole('link', { name: 'Carling', exact: true }).click();
  await expect(page).toHaveURL(`${PUBLIC}/bosses/Carling`);
  await expect(page.getByRole('navigation', { name: 'Boss catalog' })).toBeHidden();
  const back = page.getByRole('button', { name: 'Back to the catalog (Bosses)' });
  await expect(back).toHaveText('Carling');
  await expect(page.getByRole('heading', { level: 2, name: 'Boss guide' })).toBeVisible();
  await everyTab(page);
  expect(await serious(page)).toEqual([]);

  await back.click();
  await expect(page).toHaveURL(`${PUBLIC}/bosses`);
  await expect(page.getByRole('navigation', { name: 'Boss catalog' })).toBeVisible();
  await expect(page.locator('.topbar__title')).toHaveText('Bosses');
});

test('a deep link opens the guide, its tab and difficulty; an unknown boss says so', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page, '/bosses/Carling?tab=phases&difficulty=Destiny');
  await expect(page.getByRole('tablist', { name: 'Guide sections' }).getByRole('tab', { name: /^Phases/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('group', { name: 'Difficulty' }).getByRole('button', { name: /^Destiny/ })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#guide-mission-heading')).toBeVisible();

  await page.goto(`${PUBLIC}/bosses/Nobody?sw=off`);
  await expect(page.getByRole('alert')).toContainText('No knowledge for “Nobody”.');
});

test('the window sits in the shell as the admin one does: panes flush to the frame, the same difficulty buttons', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const look = async () => {
    const pressed = page.getByRole('group', { name: 'Difficulty' }).getByRole('button', { pressed: true });
    await expect(pressed).toBeVisible();
    return {
      window: await page.locator('.bosses-window').evaluate((el) => {
        const style = getComputedStyle(el);
        return { padding: style.padding, parent: el.parentElement?.classList.contains('shell') ?? false };
      }),
      button: await pressed.evaluate((el) => {
        const style = getComputedStyle(el);
        return { family: style.fontFamily, size: style.fontSize, weight: style.fontWeight, padding: style.padding };
      }),
    };
  };
  await page.goto(`${ADMIN}/bosses/Carling/knowledge?sw=off`);
  const admin = await look();
  await open(page, '/bosses/Carling');
  const member = await look();
  expect(member).toEqual(admin);
  expect(member.window).toEqual({ padding: '0px', parent: true });
});

test('the guide never carries the bot-only detail, and the admin page still does', async ({ page }) => {
  await signInPublic(page);
  const member = await (await page.request.get(`${PUBLIC}/api/public/bosses/Carling/knowledge`)).text();
  expect(member).not.toContain('"detail"');
  expect(member).not.toContain('"path"');
  const admin = await (await page.request.get(`${ADMIN}/api/admin/bosses/Carling/knowledge`)).text();
  expect(admin).toContain('"detail"');
});

test('signed out, Bosses is the sign-in page and reads no boss data', async ({ page }) => {
  const paths = apiPaths(page);
  await page.goto(`${PUBLIC}/bosses?sw=off`);
  await expect(page.getByRole('heading', { level: 1, name: HEADING.public })).toBeVisible();
  expect(paths.filter((path) => path.startsWith('/api/public/bosses'))).toEqual([]);
});
