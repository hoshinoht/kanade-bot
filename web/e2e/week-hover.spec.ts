import type { Page } from '@playwright/test';
import { ADMIN, expect, settle, test } from './support';

// Week mini cards grow on hover (fine pointers) and keyboard focus, in flow:
// the cards below in that day move down rather than being covered (an
// overlay once hid the next card's clock, 2026-10-04); a click only opens the
// run. The run sheet (below 840 px) closes on a backdrop click unless it holds
// unsaved input.

// Layout boxes (offset*), so the hover lift's 1 px transform does not count.
const box = (page: Page, sel: string) => page.locator(sel).evaluate((el: HTMLElement) => ({ y: el.offsetTop, h: el.offsetHeight }));

async function openWeek(page: Page) {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
}

test('hover grows a card in flow: it stays put, the card below moves down and back; a click opens the pane, not the grow', async ({ page }) => {
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
  await expect.poll(async () => (await box(page, '[data-run="r-carling"]')).h).toBeGreaterThan(rest.h + 10);
  await settle(page);
  // The hovered card's top stays under the pointer; the card below makes room
  // (its top at or below the grown card's bottom) instead of being covered.
  const grown = await box(page, '[data-run="r-carling"]');
  expect(grown.y).toBe(rest.y);
  const moved = await box(page, '[data-run="r-bm"]');
  expect(moved.y).toBeGreaterThanOrEqual(grown.y + grown.h);
  expect(moved.y - next.y).toBe(grown.h - rest.h);
  const [a, b] = [(await card.boundingBox())!, (await page.locator('[data-run="r-bm"]').boundingBox())!];
  expect(b.y).toBeGreaterThanOrEqual(a.y + a.height - 1);
  // Growth never scrolls the document.
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight)).toBe(true);

  // Hover-out returns the card and its neighbour.
  await page.mouse.move(5, 400);
  await expect(card).not.toHaveClass(/plan-card--grown/);
  await expect.poll(() => box(page, '[data-run="r-bm"]')).toEqual(next);
  expect(await box(page, '[data-run="r-carling"]')).toEqual(rest);

  await face.hover();
  await expect(card).toHaveClass(/plan-card--grown/);
  await face.click();
  await expect(page.getByRole('complementary', { name: 'HCarling + HStar' })).toBeVisible();
  await expect(card).not.toHaveClass(/plan-card--grown/);
  await expect(card).toHaveClass(/plan-card--selected/);
  await expect(card.locator('.row-content__compact')).toBeVisible();
  // Leaving and coming back grows it again.
  await page.mouse.move(5, 400);
  await face.hover();
  await expect(card).toHaveClass(/plan-card--grown/);

  // A press settles a card at once. Pressed with the pane already open, so the board keeps its width.
  const below = page.locator('[data-run="r-bm"]');
  await below.locator('.plan-card__open').hover();
  await expect(below).toHaveClass(/plan-card--grown/);
  await below.locator('.plan-card__open').click();
  await expect(below).toHaveClass(/plan-card--selected/);
  expect(await below.locator('.row-content__reveal').evaluate((el) => getComputedStyle(el).transitionDuration)).toBe('0s');
});

test('the selected card clips its art, wash and status mark to its rounded shape', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  const card = page.locator('[data-run="r-carling"]');
  await card.locator('.plan-card__open').click();
  await expect(card).toHaveClass(/plan-card--selected/);
  await page.mouse.move(5, 400);
  for (const grow of [false, true]) {
    if (grow) {
      await card.locator('.plan-card__open').hover();
      await expect(card).toHaveClass(/plan-card--grown/);
      await settle(page);
    }
    const geo = await card.evaluate((li) => {
      const c = li.getBoundingClientRect();
      const art = li.querySelector('.runcard__art')!.getBoundingClientRect();
      const style = getComputedStyle(li);
      const mark = getComputedStyle(li, '::before');
      return {
        overflow: style.overflow,
        radius: parseFloat(style.borderTopRightRadius),
        // The art may reach past the card (above its name band) only where the card clips it.
        artLeft: art.left >= c.left - 0.5 && art.right <= c.right + 0.5,
        markInside: mark.position === 'absolute' && mark.left === '0px' && parseFloat(mark.width) <= 4,
      };
    });
    expect(geo.overflow, `grown ${grow}`).toBe('clip');
    expect(geo.radius).toBeGreaterThanOrEqual(10);
    expect(geo.artLeft).toBe(true);
    expect(geo.markInside).toBe(true);
  }
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

// The Changes tab once ran a four-column table past a 340 px pane. Its change
// log stacks each row (summary over who/when/how) and opens in place, so
// nothing may cross the scope's right edge, opened or not.
async function logFits(page: Page, scope: string) {
  const log = page.locator(`${scope} .runlog`);
  const rows = log.locator('.runlog__row');
  await expect(rows.first()).toBeVisible();
  for (const row of await rows.all()) await row.click();
  await expect(log.locator('dl:not([hidden])').first()).toBeVisible();
  const fit = await page.locator(scope).evaluate((pane) => {
    const edge = pane.getBoundingClientRect().right;
    const parts = [...pane.querySelectorAll<HTMLElement>('.runlog__row, .runlog__row > *, .runlog__meta > *, .runlog__field, .runlog__field > *')].filter((el) => el.checkVisibility());
    const over = parts.filter((el) => el.getBoundingClientRect().right > edge + 0.5).map((el) => el.textContent?.trim());
    return { over, parts: parts.length };
  });
  expect(fit.parts).toBeGreaterThan(8);
  expect(fit.over).toEqual([]);
}

test('the run pane Changes tab fits the side pane', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  await page.locator('[data-run="r-kalos"] .plan-card__open').click();
  const pane = page.getByRole('complementary', { name: 'XKalos' });
  await pane.getByRole('tab', { name: 'Changes' }).click();
  await logFits(page, 'aside.week-pane');
});

test('the phone run sheet change log fits the sheet', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-kalos"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'XKalos' });
  // No disclosure: the log sits at the sheet's foot and loads when scrolled to.
  await sheet.getByRole('heading', { name: 'Changes' }).scrollIntoViewIfNeeded();
  await logFits(page, 'dialog[open] .modal__panel');
});

test('at a glance is about the next run: its party with answers in words, waiting first', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  const glance = page.getByRole('complementary', { name: 'At a glance' });
  const party = glance.getByRole('region', { name: /^Party/ });
  await expect(party).toBeVisible();
  // The week-wide list and its total are gone (they live in the Answers tab and the footer).
  await expect(glance.getByRole('heading', { name: /Waiting on answers/ })).toHaveCount(0);
  const run = await page.evaluate(async () => {
    const week = await (await fetch('/api/admin/week')).json();
    const summary = await (await fetch('/api/admin/summary')).json();
    return week.runs.find((r: { id: string }) => r.id === summary.next.run_id);
  });
  const rows = party.locator('.week-glance__row');
  await expect(rows).toHaveCount(run.participants.length);
  const words = await party.locator('.week-glance__answer').allTextContents();
  const order = ['waiting', 'maybe', 'on', 'out'];
  const ranks = words.map((w) => order.indexOf(w.replace(/^\S+\s/, '').trim()));
  expect(ranks.every((r) => r >= 0)).toBe(true);
  expect([...ranks].sort((x, y) => x - y)).toEqual(ranks);
  const waiting = run.participants.filter((p: { answer: string }) => p.answer === 'waiting').length;
  await expect(party.locator('.week-glance__hint')).toHaveText(waiting ? new RegExp(`^${waiting} (hasn't|haven't) answered`) : /Everyone has answered/);
});

test('the channel filter offers only the week\'s party channels and narrows every view', async ({ page }) => {
  // The real API sends the channel's name as `party` (the mock sends the id),
  // which is why matching on it never filtered: rewrite it as production does.
  await page.route('**/api/admin/week*', async (route) => {
    const response = await route.fetch();
    const week = await response.json();
    for (const run of week.runs) run.party = run.channel;
    await route.fulfill({ response, json: week });
  });
  // Guild channels outside the bossing category, as the real channel list has them.
  await page.route('**/api/admin/channels', async (route) => {
    const response = await route.fetch();
    const channels = await response.json();
    channels.push({ id: '900000000000000001', name: '#general' }, { id: '900000000000000002', name: '#bot-spam' });
    await route.fulfill({ response, json: channels });
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  const { runs, all } = await page.evaluate(async () => ({
    runs: (await (await fetch('/api/admin/week')).json()).runs as { id: string; channel_id: string; channel: string; status: string }[],
    all: (await (await fetch('/api/admin/channels')).json()) as { id: string }[],
  }));
  const party = [...new Set(runs.map((r) => r.channel_id))];
  await page.getByRole('button', { name: 'Filters (0)' }).click();
  const filters = page.getByRole('search', { name: 'Filter the week' });
  const select = filters.getByLabel('Channel');
  const offered = await select.locator('option').evaluateAll((options) => (options as HTMLOptionElement[]).map((o) => o.value));
  expect(offered[0]).toBe('');
  await expect(select.locator('option').first()).toHaveText('All channels');
  expect(offered.slice(1).sort()).toEqual([...party].sort());
  // Guild channels with no run this week are not offered.
  expect(all.some((c) => !party.includes(c.id))).toBe(true);
  for (const c of all.filter((c) => !party.includes(c.id))) expect(offered).not.toContain(c.id);

  // Pick the channel with the most live runs.
  const live = runs.filter((r) => r.status !== 'done' && r.status !== 'cancelled');
  const pick = party.map((id) => ({ id, n: live.filter((r) => r.channel_id === id).length })).sort((a, b) => b.n - a.n)[0]!;
  const mine = live.filter((r) => r.channel_id === pick.id).map((r) => r.id).sort();
  await select.selectOption(pick.id);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${mine.length} run${mine.length === 1 ? '' : 's'}, filtered`);
  expect((await page.locator('.board [data-run]').evaluateAll((els) => els.map((el) => (el as HTMLElement).dataset.run!))).sort()).toEqual(mine);
  await page.keyboard.press('Escape');

  await page.getByRole('tab', { name: 'Runs' }).click();
  expect((await page.locator('[data-row]').evaluateAll((els) => els.map((el) => (el as HTMLElement).dataset.row!))).sort()).toEqual(mine);

  await page.getByRole('tab', { name: 'Answers' }).click();
  await expect(page.getByRole('heading', { name: 'Still waiting' })).toBeVisible();
  // Every member listed as still waiting owes an answer on one of this channel's runs.
  const listed = await page.locator('.week-answers__member').count();
  const expected = new Set(
    (await page.evaluate(async () => (await (await fetch('/api/admin/week')).json()).runs as { id: string; participants: { id: string; answer: string }[] }[]))
      .filter((r) => mine.includes(r.id))
      .flatMap((r) => r.participants.filter((p) => p.answer === 'waiting' || p.answer === 'maybe').map((p) => p.id)),
  );
  expect(listed).toBe(expected.size);
});

test('the run pane grows with the window', async ({ page }) => {
  for (const [width, height, want] of [
    [1280, 800, 1280 * 0.32],
    [1600, 900, 480],
  ] as const) {
    await page.setViewportSize({ width, height });
    await openWeek(page);
    await page.locator('[data-run="r-kalos"] .plan-card__open').click();
    const pane = page.getByRole('complementary', { name: 'XKalos' });
    await expect(pane).toBeVisible();
    expect(Math.abs((await pane.boundingBox())!.width - want)).toBeLessThan(2);
  }
});

test('clicking the open run again closes its pane; another run switches it', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  const card = page.locator('[data-run="r-kalos"]');
  const pane = page.getByRole('complementary', { name: 'XKalos' });
  await card.locator('.plan-card__open').click();
  await expect(pane).toBeVisible();
  await card.locator('.plan-card__open').click();
  await expect(pane).toHaveCount(0);
  await expect(card).not.toHaveClass(/plan-card--selected/);
  await expect(card.locator('.plan-card__open')).toBeFocused();
  // Another run still switches the pane instead of closing it.
  await card.locator('.plan-card__open').click();
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  await expect(page.getByRole('complementary', { name: 'HCarling + HStar' })).toBeVisible();
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  await expect(page.locator('aside.week-pane')).toHaveCount(0);
});

test('the pane pops out to the full sheet on the same tab and comes back to the pane', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openWeek(page);
  await page.locator('[data-run="r-kalos"] .plan-card__open').click();
  const pane = page.getByRole('complementary', { name: 'XKalos' });
  await pane.getByRole('tab', { name: 'Answers' }).click();
  const pop = pane.getByRole('button', { name: 'Open in a larger view' });
  await expect(pop).toHaveAttribute('title', 'Open in a larger view');
  await pop.click();
  const sheet = page.getByRole('dialog', { name: 'XKalos' });
  await expect(sheet).toBeVisible();
  await expect(pane).toHaveCount(0);
  // Same tab: the sheet's Answers section is open.
  await expect(sheet.locator('details.run__answers')).toHaveAttribute('open', '');
  expect(await page.evaluate(() => !!document.activeElement?.closest('dialog[open]'))).toBe(true);
  // The run stays selected on the board meanwhile.
  await expect(page.locator('[data-run="r-kalos"]')).toHaveClass(/plan-card--selected/);
  await page.keyboard.press('Escape');
  await expect(sheet).toBeHidden();
  await expect(pane).toBeVisible();
  await expect(pane.getByRole('tab', { name: 'Answers' })).toHaveAttribute('aria-selected', 'true');
  await expect(pane.getByRole('button', { name: 'Open in a larger view' })).toBeFocused();

  // Changes pops out to the change log, scrolled into view; the backdrop closes it back to the pane when clean.
  await pane.getByRole('tab', { name: 'Changes' }).click();
  await pane.getByRole('button', { name: 'Open in a larger view' }).click();
  await expect(sheet.locator('.runlog__row').first()).toBeInViewport();
  const panel = (await sheet.locator('.modal__panel').boundingBox())!;
  await page.mouse.click(Math.max(4, panel.x / 2), 400);
  await expect(sheet).toBeHidden();
  await expect(pane.getByRole('tab', { name: 'Changes' })).toHaveAttribute('aria-selected', 'true');
});
