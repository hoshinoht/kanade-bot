import type { FixedRow } from '@kanade/api-types';
import type { Locator, Page } from '@playwright/test';
import { ADMIN, expect, settle, test, unconditional } from './support';

const MULTI = 'Edit Tuesday 22:00 — HCarling + HStar';
const height = (row: Locator) => row.evaluate((el) => Math.round(el.getBoundingClientRect().height));
const fixedRow = (page: Page) => page.locator('tr').filter({ has: page.getByRole('button', { name: MULTI, exact: true }) });

test('Fixed: compact multi-boss identity is one ellipsised line with overlapping portraits and small pills', async ({ page }) => {
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await page.getByRole('button', { name: 'Edit Friday 21:30 — XKalos', exact: true }).click();
  const row = fixedRow(page);
  const compact = row.locator('.row-content__compact');
  await expect(compact).toBeVisible();
  await expect(compact.locator('.portrait')).toHaveCount(2);
  await expect(compact.locator('.pill')).toHaveCount(2);
  await expect(compact.locator('.boss-stack__names')).toContainText('Carling + Radiant Malefic Star');
  const layout = await compact.evaluate((el) => {
    const name = el.querySelector<HTMLElement>('.boss-stack__names')!;
    const pictures = [...el.querySelectorAll('.portrait')].map((p) => p.getBoundingClientRect());
    const pills = [...el.querySelectorAll('.pill')].map((p) => p.getBoundingClientRect());
    return { ellipsis: getComputedStyle(name).textOverflow, nowrap: getComputedStyle(name).whiteSpace, clipped: name.scrollWidth > name.clientWidth, overlap: pictures[1]!.left < pictures[0]!.right, tops: [...pictures, ...pills].map((r) => Math.round(r.y + r.height / 2)) };
  });
  expect(layout.ellipsis).toBe('ellipsis');
  expect(layout.nowrap).toBe('nowrap');
  expect(layout.clipped).toBe(true);
  expect(layout.overlap).toBe(true);
  expect(Math.max(...layout.tops) - Math.min(...layout.tops)).toBeLessThanOrEqual(1);
});

test('Fixed: keyboard selection animates height, reveals stacked tags, and closing restores the compact row', async ({ page }) => {
  await page.goto(`${ADMIN}/fixed?sw=off`);
  const row = fixedRow(page);
  const opener = row.getByRole('button');
  const before = await height(row);
  await opener.focus();
  await expect(opener).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(opener).toHaveAttribute('aria-current', 'true');
  await expect.poll(() => height(row)).toBeGreaterThan(before + 10);
  await expect(row.locator('.row-content__full .boss')).toHaveCount(2);
  await expect(row.locator('.row-content__full')).toContainText('Radiant Malefic Star');
  expect(await row.locator('.row-content__reveal').evaluate((el) => getComputedStyle(el).transitionProperty)).toContain('grid-template-rows');
  await page.getByRole('button', { name: 'Close weekly timing' }).click();
  await expect.poll(() => height(row)).toBe(before);
  await expect(opener).toBeFocused();
});

test('selected rows have no transition under reduced motion', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${ADMIN}/fixed?sw=off`);
  const row = fixedRow(page);
  const before = await height(row);
  await row.getByRole('button').click();
  await expect.poll(() => height(row)).toBeGreaterThan(before + 10);
  for (const element of [row, row.locator('.row-content__reveal')]) {
    expect(await element.evaluate((el) => getComputedStyle(el).transitionDuration)).toBe('0s');
  }
});

test('Fixed: the reveal produces intermediate heights, not just an animated-looking endpoint', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.goto(`${ADMIN}/fixed?sw=off`);
  await expect(page.locator('[data-fixed="f-carling"]')).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
  await settle(page);
  const samples = await page.evaluate(async () => {
    const button = document.querySelector<HTMLButtonElement>('[data-fixed="f-carling"]')!;
    const row = button.closest('tr')!;
    const heights = [row.getBoundingClientRect().height];
    button.click();
    const end = performance.now() + 350;
    while (performance.now() < end) {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      heights.push(row.getBoundingClientRect().height);
    }
    return heights;
  });
  const first = samples[0]!;
  const last = samples.at(-1)!;
  expect(last).toBeGreaterThan(first + 10);
  expect(samples.some((sample) => sample > first + 1 && sample < last - 1)).toBe(true);
});

test('Week: opening a run keeps its card at rest (the ring marks it), and closing leaves it so', async ({ page }) => {
  // Cards grow on hover and keyboard focus only (e2e/week-hover.spec.ts), never on a click.
  await page.goto(`${ADMIN}/?sw=off`);
  const card = page.locator('[data-run="r-carling"]');
  const neighbor = page.locator('[data-run="r-bm"]');
  await expect(card).toBeVisible();
  const before = await height(card);
  const otherHeight = await height(neighbor);
  await card.locator('.plan-card__open').click();
  await expect(card.locator('.plan-card__open')).toHaveAttribute('aria-current', 'true');
  await expect(card).toHaveClass(/plan-card--selected/);
  await settle(page);
  expect(await height(card)).toBe(before);
  expect(await height(neighbor)).toBe(otherHeight);
  await expect(card.locator('.row-content__compact')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect.poll(() => height(card)).toBe(before);
});

test('Bosses: selection preserves the list DOM while revealing all difficulties', async ({ page }) => {
  await page.goto(`${ADMIN}/bosses/MaleficStar/knowledge?sw=off`);
  const row = page.locator('.bossrow').filter({ has: page.getByRole('link', { name: 'Carling', exact: true }) });
  await expect(row).toBeVisible();
  const before = await height(row);
  await row.evaluate((el) => el.setAttribute('data-preserved', 'yes'));
  await row.getByRole('link').focus();
  await page.keyboard.press('Enter');
  await expect(row.getByRole('link')).toHaveAttribute('aria-current', 'true');
  await expect(row).toHaveAttribute('data-preserved', 'yes');
  await expect.poll(() => height(row)).toBeGreaterThan(before + 10);
  // Catalog difficulties only; knowledge may add info-only Champion/Destiny ticks.
  await expect(row.locator('.row-content__full .boss-tick:not(.boss-tick--champion, .boss-tick--destiny)')).toHaveCount(4);
});

for (const screen of [
  { name: 'Inbox', path: '/inbox?tab=self_service', selector: '[data-item="p-kalos-expired"]', state: 'aria-selected' },
  { name: 'History', path: '/history', selector: '[data-history="2"]', state: 'aria-current' },
  { name: 'Members', path: '/members', selector: '[data-member="1003"]', state: 'aria-current' },
  { name: 'Config', path: '/config?section=pings', selector: '.settings__tab[aria-controls$="-models"]', state: 'aria-selected' },
]) {
  test(`${screen.name}: selection grows the shared content and keeps the shell fixed`, async ({ page }) => {
    await page.goto(`${ADMIN}${screen.path}${screen.path.includes('?') ? '&' : '?'}sw=off`);
    const row = page.locator(screen.selector).first();
    await expect(row).toBeVisible();
    const before = await height(row);
    const shell = await page.locator('.shell').evaluate((el) => el.getBoundingClientRect().top);
    await row.click();
    await expect(row).toHaveAttribute(screen.state, 'true');
    await expect.poll(() => height(row)).toBeGreaterThan(before + 5);
    expect(await page.locator('.shell').evaluate((el) => el.getBoundingClientRect().top)).toBe(shell);
    expect(await page.evaluate(() => document.scrollingElement!.scrollTop)).toBe(0);
  });
}

for (const width of [1920, 2560]) {
  test(`Week: every day stays a single card column at ${width}×1080`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1080 });
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
    const failures = await page.locator('.board__runs').evaluateAll((lists) => lists.flatMap((list) => {
      const cards = [...list.children].map((c) => c.getBoundingClientRect());
      return cards.slice(1).flatMap((card, index) => Math.abs(card.left - cards[0]!.left) > 1 || card.top < cards[index]!.bottom ? ['cards share a row'] : []);
    }));
    expect(failures).toEqual([]);
  });
}

test('Fixed: expanding a lower row never scrolls the fixed shell or jumps the list anchor', async ({ page }) => {
  await page.route(`${ADMIN}/api/admin/fixed`, async (route) => {
    const response = await route.fetch(unconditional(route));
    const rows = await response.json() as FixedRow[];
    const multi = rows.find((row) => row.bosses.length > 1)!;
    await route.fulfill({ response, json: [...rows, ...Array.from({ length: 20 }, (_, index) => ({ ...multi, id: `long-${index}`, short_id: `l${index}` }))] });
  });
  await page.goto(`${ADMIN}/fixed?sw=off`);
  const opener = page.locator('[data-fixed="long-15"]');
  await opener.scrollIntoViewIfNeeded();
  const before = await page.evaluate(() => ({ shell: document.querySelector('.fixed-window')!.getBoundingClientRect().top, scroll: document.querySelector('.fixed-list__table')!.scrollTop }));
  await opener.click();
  await expect(opener).toHaveAttribute('aria-current', 'true');
  await page.waitForTimeout(300);
  const after = await page.evaluate(() => ({ shell: document.querySelector('.fixed-window')!.getBoundingClientRect().top, scroll: document.querySelector('.fixed-list__table')!.scrollTop, body: document.scrollingElement!.scrollTop, moved: [...document.querySelectorAll<HTMLElement>('.frame, .shell, .fixed-window, .fixed-window__body')].some((el) => el.scrollTop !== 0) }));
  expect(after.shell).toBe(before.shell);
  expect(after.scroll).toBe(before.scroll);
  expect(after.body).toBe(0);
  expect(after.moved).toBe(false);
});

// Inbox and Bosses replace their phone list with a detail. Measure that same
// selected DOM briefly at its original list width, then restore it before paint.
async function selectedHeight(row: Locator, listWidth: number): Promise<number> {
  return row.evaluate((element, width) => {
    const list = element.closest<HTMLElement>('.inbox__list, .bosses-list');
    if (!list || getComputedStyle(list).display !== 'none') return Math.round(element.getBoundingClientRect().height);
    const properties = ['display', 'position', 'width'];
    const previous = properties.map((property) => ({ property, value: list.style.getPropertyValue(property), priority: list.style.getPropertyPriority(property) }));
    try {
      list.style.setProperty('display', 'flex', 'important');
      list.style.setProperty('position', 'fixed');
      list.style.setProperty('width', `${width}px`);
      return Math.round(element.getBoundingClientRect().height);
    } finally {
      for (const { property, value, priority } of previous) {
        if (value) list.style.setProperty(property, value, priority);
        else list.style.removeProperty(property);
      }
    }
  }, listWidth);
}

const PHONE_ROWS = [
  { name: 'Fixed multi-boss', path: '/fixed', row: 'tr:has([data-fixed="f-carling"])', trigger: '[data-fixed="f-carling"]', state: 'aria-current' },
  { name: 'Fixed single-boss with a flag', path: '/fixed', row: 'tr:has([data-fixed="f-kalos"])', trigger: '[data-fixed="f-kalos"]', state: 'aria-current' },
  { name: 'Week', path: '/', row: '[data-run="r-carling"]', trigger: '.plan-card__open', state: 'aria-current' },
  { name: 'Inbox self-service', path: '/inbox?tab=self_service', row: '[data-item="p-carling-link"]', state: 'aria-selected' },
  { name: 'Inbox extractor', path: '/inbox?tab=extractor', row: '[data-item="p-bm-move"]', state: 'aria-selected' },
  { name: 'History', path: '/history', row: '[data-history="2"]', state: 'aria-current' },
  { name: 'Members', path: '/members', row: '[data-member="1003"]', state: 'aria-current' },
  { name: 'Bosses catalog', path: '/bosses', row: '.bossrow:has(a[href="/bosses/Carling/knowledge"])', trigger: 'a', state: 'aria-current' },
  { name: 'Bosses event', path: '/bosses', row: '.bosses-events li:has(a[href="/bosses/Kai/knowledge"])', trigger: 'a', state: 'aria-current' },
  { name: 'Config', path: '/config?section=pings', row: '.settings__tab[aria-controls$="-models"]', state: 'aria-selected' },
];

for (const size of [{ width: 390, height: 844 }, { width: 844, height: 390 }]) {
  for (const screen of PHONE_ROWS) {
    test(`${screen.name}: phone selection keeps the collapsed height at ${size.width}×${size.height}`, async ({ page }) => {
      await page.setViewportSize(size);
      await page.goto(`${ADMIN}${screen.path}${screen.path.includes('?') ? '&' : '?'}sw=off`);
      const row = page.locator(screen.row).first();
      const trigger = 'trigger' in screen ? row.locator(screen.trigger!) : row;
      await trigger.scrollIntoViewIfNeeded();
      await expect(row).toBeVisible();
      await page.evaluate(() => document.fonts.ready);
      await settle(page);
      const before = await height(row);
      const listWidth = await row.evaluate((element) => element.closest('.inbox__list, .bosses-list')?.getBoundingClientRect().width ?? 0);
      await trigger.click();
      await expect(trigger).toHaveAttribute(screen.state, 'true');
      await settle(page);
      expect(await selectedHeight(row, listWidth)).toBe(before);
      await expect(row.locator('.row-content--expanded')).toHaveCount(0);
      if (await row.locator('.row-content').count()) {
        await expect(row.locator('.row-content__compact')).toHaveAttribute('aria-hidden', 'false');
        await expect(row.locator('.row-content__reveal')).toHaveAttribute('aria-hidden', 'true');
      }
      expect(await page.evaluate(() => document.scrollingElement!.scrollTop)).toBe(0);
    });
  }
}

for (const width of [600, 899]) {
  test(`Config: the ${width}px sideways strip keeps its selected tab one line tall`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`${ADMIN}/config?section=pings&sw=off`);
    const models = page.locator('.settings__tab[aria-controls$="-models"]');
    await models.scrollIntoViewIfNeeded();
    const before = await height(models);
    await models.click();
    await expect(models).toHaveAttribute('aria-selected', 'true');
    await settle(page);
    expect(await height(models)).toBe(before);
    await expect(models.locator('.row-content--expanded')).toHaveCount(0);
    await expect(models.locator('.row-content__compact')).toBeVisible();
  });
}
