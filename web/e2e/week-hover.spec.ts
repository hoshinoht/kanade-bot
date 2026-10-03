import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Week mini cards grow on hover (fine pointers) and keyboard focus, as an
// overlay that moves nothing; a click only opens the run. The run sheet
// (below 840 px) closes on a backdrop click unless it holds unsaved input.

// Layout boxes (offset*), so the hover lift's 1 px transform does not count.
const box = (page: Page, sel: string) => page.locator(sel).evaluate((el: HTMLElement) => ({ y: el.offsetTop, h: el.offsetHeight }));

async function openWeek(page: Page) {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
}

test('hover grows a card over its neighbours without moving them; a click opens the pane, not the grow', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  // HCarling + HStar has XBM under it in the Tue column.
  const card = page.locator('[data-run="r-carling"]');
  const face = card.locator('.plan-card__open');
  const rest = await box(page, '[data-run="r-carling"]');
  const next = await box(page, '[data-run="r-bm"]');
  await face.hover();
  await expect(card).toHaveClass(/plan-card--grown/);
  await expect(card.locator('.row-content__full .portrait').first()).toBeVisible();
  await expect.poll(async () => (await face.boundingBox())!.height).toBeGreaterThan(rest.h + 10);
  await page.waitForTimeout(300);
  // The slot and the card below stay exactly where they were.
  expect(await box(page, '[data-run="r-carling"]')).toEqual(rest);
  expect(await box(page, '[data-run="r-bm"]')).toEqual(next);

  await face.click();
  await expect(page.getByRole('complementary', { name: 'HCarling + HStar' })).toBeVisible();
  await expect(card).not.toHaveClass(/plan-card--grown/);
  await expect(card).toHaveClass(/plan-card--selected/);
  await expect(card.locator('.row-content__compact')).toBeVisible();
  // Leaving and coming back grows it again.
  await page.mouse.move(5, 400);
  await face.hover();
  await expect(card).toHaveClass(/plan-card--grown/);
});

test('a click opening the modal sheet leaves the card at rest behind it', async ({ page }) => {
  await page.setViewportSize({ width: 820, height: 800 });
  await page.goto(`${ADMIN}/?sw=off`);
  const card = page.locator('[data-run="r-carling"]');
  await card.locator('.plan-card__open').click();
  await expect(page.getByRole('dialog', { name: 'HCarling + HStar' })).toBeVisible();
  await expect(card).not.toHaveClass(/plan-card--grown/);
});

test('keyboard focus grows a card; M lifts it back to rest', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  const card = page.locator('[data-run="r-bm"]');
  await page.locator('[data-run="r-carling"] .plan-card__open').focus();
  await page.keyboard.press('Tab');
  await expect(card.locator('.plan-card__open')).toBeFocused();
  await expect(card).toHaveClass(/plan-card--grown/);
  await page.keyboard.press('m');
  await expect(card).toHaveClass(/plan-card--lifted/);
  await expect(card).not.toHaveClass(/plan-card--grown/);
  await page.keyboard.press('Escape');
});

test('reduced motion grows at once', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  const card = page.locator('[data-run="r-carling"]');
  await card.locator('.plan-card__open').hover();
  await expect(card).toHaveClass(/plan-card--grown/);
  expect(await card.locator('.row-content__reveal').evaluate((el) => getComputedStyle(el).transitionDuration)).toBe('0s');
});

test.describe('touch', () => {
  test.use({ hasTouch: true, isMobile: true, viewport: { width: 1280, height: 800 } });

  test('a tap opens the run and never grows the card', async ({ page }) => {
    await openWeek(page);
    const card = page.locator('[data-run="r-carling"]');
    await card.locator('.plan-card__open').tap();
    await expect(page.getByRole('complementary', { name: 'HCarling + HStar' }).or(page.getByRole('dialog', { name: 'HCarling + HStar' }))).toBeVisible();
    await expect(card).not.toHaveClass(/plan-card--grown/);
  });
});

test.describe('run sheet backdrop', () => {
  test.use({ viewport: { width: 800, height: 800 } });

  async function backdrop(page: Page) {
    const panel = (await page.locator('dialog[open] .modal__panel').boundingBox())!;
    // A point on the backdrop: beside the panel, or above it.
    return panel.x > 12 ? { x: panel.x / 2, y: 400 } : { x: 400, y: Math.max(2, panel.y / 2) };
  }

  test('closes on a backdrop click when clean, stays open with unsaved input or a drag from the panel', async ({ page }) => {
    await page.goto(`${ADMIN}/?sw=off`);
    const open = page.locator('[data-run="r-limbo"] .plan-card__open');
    const sheet = page.getByRole('dialog', { name: 'HLimbo' });

    await open.click();
    await expect(sheet).toBeVisible();
    let at = await backdrop(page);
    await page.mouse.click(at.x, at.y);
    await expect(sheet).toBeHidden();
    await expect(open).toBeFocused();

    // A typed Move target is unsaved: the backdrop leaves it open.
    await open.click();
    await expect(sheet).toBeVisible();
    at = await backdrop(page);
    const field = sheet.getByRole('textbox', { name: /Move HLimbo/ });
    await field.fill('sat 20:30');
    await page.mouse.click(at.x, at.y);
    await expect(sheet).toBeVisible();
    await expect(field).toHaveValue('sat 20:30');
    await field.fill('');

    // So is an open swap picker.
    await sheet.getByRole('button', { name: 'Swap timing with…' }).click();
    await page.mouse.click(at.x, at.y);
    await expect(sheet).toBeVisible();
    await sheet.getByRole('group', { name: /Swap HLimbo's timing/ }).getByRole('button', { name: 'Cancel' }).click();

    // A press in the panel released on the backdrop (a text selection) is not a dismiss.
    const title = (await sheet.locator('.modal__title').boundingBox())!;
    await page.mouse.move(title.x + 4, title.y + title.height / 2);
    await page.mouse.down();
    await page.mouse.move(at.x, at.y, { steps: 5 });
    await page.mouse.up();
    await expect(sheet).toBeVisible();

    // Escape still closes it.
    await page.keyboard.press('Escape');
    await expect(sheet).toBeHidden();
  });
});
