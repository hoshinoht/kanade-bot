import type { Locator, Page } from '@playwright/test';
import { ADMIN, csrf, expect, test } from './support';

// History graph (boards docs/research/2026-10-12-graph-presence-mockups). `POST
// /__mock/graph/board` grows History to head #39 and seeds the boards' drafts:
// Weekend reshuffle (on #36, 2 conflicts), Bellona clean-up (#38, 1), Kalos to
// Saturdays (#34, weekly timings, none), Tuesday Kalos (#31 → merged #33),
// Scratch (#32, discarded) and an expired draft from #12.

async function board(page: Page) {
  expect((await page.request.post(`${ADMIN}/__mock/graph/board`)).status()).toBe(204);
}

async function graph(page: Page, query = '') {
  await page.goto(`${ADMIN}/history?tab=graph${query}&sw=off`);
  const table = page.getByRole('grid', { name: /^History graph/ });
  await expect(table).toBeVisible();
  return table;
}

const row = (page: Page, key: string) => page.locator(`tr[data-row="${key}"]`);
const focused = (page: Page) => page.evaluate(() => document.activeElement?.closest('tr')?.dataset.row ?? document.activeElement?.tagName ?? '');

test('history graph: tab, roving rows, arrows into drafts, Enter opens, Escape closes', async ({ page }) => {
  await board(page);
  await graph(page);
  await expect(page.getByRole('tab', { name: 'Graph' })).toHaveAttribute('aria-selected', 'true');
  // The Week / Who filters stay with the Timeline.
  await expect(page.getByRole('search', { name: 'Filter the history' })).toBeHidden();
  // One tab stop in the grid.
  await expect(page.locator('tr[tabindex="0"]')).toHaveCount(1);
  await row(page, 't-d-weekend').focus();
  await page.keyboard.press('ArrowDown');
  expect(await focused(page)).toBe('t-d-bellona');
  await expect(page.locator('tr[tabindex="0"]')).toHaveAttribute('data-row', 't-d-bellona');
  await page.keyboard.press('End');
  expect(await focused(page)).toBe('more');
  await page.keyboard.press('Home');
  expect(await focused(page)).toBe('t-d-weekend');
  // Down to #38, Enter: the change pane; Escape: back on the row.
  for (let i = 0; i < 4; i++) await page.keyboard.press('ArrowDown');
  expect(await focused(page)).toBe('r38');
  await page.keyboard.press('Enter');
  const pane = page.getByRole('complementary', { name: 'Change details' });
  await expect(pane).toContainText('Change #38');
  await expect(pane.locator('[data-fid="graph-pane-drafts"], .graph-at')).toContainText('Bellona clean-up');
  await expect(row(page, 'r38')).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Escape');
  await expect(pane).toBeHidden();
  expect(await focused(page)).toBe('r38');
  // → into the row's Drafts chip, Enter opens the draft; ← back to the row.
  await page.keyboard.press('ArrowRight');
  await expect(page.locator(':focus')).toHaveText('Bellona clean-up starts here');
  await page.keyboard.press('ArrowLeft');
  expect(await focused(page)).toBe('r38');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Enter');
  const draft = page.getByRole('complementary', { name: 'Draft Bellona clean-up' });
  await expect(draft).toBeVisible();
  await expect(draft.getByRole('link', { name: 'Open draft' })).toHaveAttribute('href', '/drafts?item=d-bellona');
  // Opening it counts its conflicts: the tip now says so (base moved until then).
  await expect(draft).toContainText('Conflicts · 1');
  await expect(row(page, 't-d-bellona')).toContainText('1 conflict');
  await expect(row(page, 't-d-weekend')).toContainText('Base moved');
  await page.keyboard.press('Escape');
  await expect(draft).toBeHidden();
  expect(await focused(page)).toBe('t-d-bellona');
});

test('history graph: every branch the lanes draw is said in the row; List shows the same words', async ({ page }) => {
  await board(page);
  await graph(page);
  const rows = page.locator('tr[data-row]');
  const events = page.locator('td.glane[data-events]');
  expect(await events.count()).toBeGreaterThan(5);
  const drawn = new Set<string>();
  for (const cell of await events.all()) {
    const tr = cell.locator('xpath=..');
    const kind = (await tr.getAttribute('data-row'))!;
    const words = kind.startsWith('t-') ? await tr.innerText() : await tr.locator('td.gx__drafts').innerText();
    for (const event of (await cell.getAttribute('data-events'))!.split('|')) {
      const [seg, title] = event.split(/:(.*)/s) as [string, string];
      drawn.add(seg);
      expect(words, `${kind}: ${event}`).toContain(title);
    }
  }
  // The first page draws tips, starts, a merge, a discard and a lane from further down.
  expect([...drawn].sort()).toEqual(['base', 'end', 'merge', 'stub', 'tip']);
  const text = async () => (await rows.locator('td.gx__drafts').allInnerTexts()).join('\n');
  const before = await text();
  expect(before).toContain('Tuesday Kalos merged · from #31');
  expect(before).toContain('Scratch discarded · from #32');
  await page.getByRole('group', { name: 'Show as' }).getByRole('button', { name: 'List' }).click();
  await expect(page.locator('td.glane')).toHaveCount(0);
  await expect(page.getByText('The same rows without the lanes')).toBeVisible();
  expect(await text()).toBe(before);
  // The lane cells are hidden from assistive tech in Graph view.
  await page.getByRole('group', { name: 'Show as' }).getByRole('button', { name: 'Graph' }).click();
  await expect(page.locator('td.glane:not([aria-hidden="true"])')).toHaveCount(0);
});

test('history graph: quiet stretches fold, Older changes pages, Go to # windows the view', async ({ page }) => {
  await board(page);
  await graph(page);
  // #39–#20: #25–#30 and #20–#24 are quiet (the expired draft's lane runs through the lower one).
  const fold = row(page, 'f30-25');
  await expect(fold).toContainText('#25 – #30 · 6 changes');
  await expect(row(page, 'f24-20')).toContainText('No draft starts or ends in these');
  await expect(row(page, 'more')).toContainText('starts at #12, further down');
  await fold.getByRole('button', { name: 'Show 6' }).click();
  await expect(row(page, 'r27')).toBeVisible();
  await expect(fold).toHaveCount(0);
  // Older changes: #19 down to #1 (the oldest quiet ones fold too); the expired draft now starts in view.
  await row(page, 'more').getByRole('button', { name: 'Older changes' }).click();
  await expect(row(page, 'f11-1')).toContainText('#1 – #11 · 11 changes');
  await expect(row(page, 'more')).toHaveCount(0);
  await expect(row(page, 'r12')).toContainText('starts here');
  // Go to #18: #18–#28 shown, the newer ones and the tips fold above.
  await page.getByRole('button', { name: 'Go to #…' }).click();
  await page.getByRole('textbox', { name: 'Change number' }).fill('18');
  await page.getByRole('button', { name: 'Go', exact: true }).click();
  await expect(page.getByText('Showing #18 – #28')).toBeVisible();
  await expect(row(page, 'tips')).toContainText('3 open drafts');
  await expect(row(page, 'newer')).toContainText('#29 – #39 · 11 newer changes');
  await expect(row(page, 'newer')).toContainText('3 drafts start in here');
  // It moves the view and the focus; it opens no pane (B_HistoryGraphLong).
  expect(await focused(page)).toBe('r18');
  await expect(row(page, 'r18')).toHaveAttribute('aria-selected', 'false');
  await expect(page.getByRole('complementary', { name: 'Change details' })).toHaveCount(0);
  // Show 11 brings the view back to the head.
  await row(page, 'newer').getByRole('button', { name: 'Show 11' }).click();
  await expect(row(page, 'r39')).toBeVisible();
  await expect(page.getByText(/^Showing #/)).toHaveCount(0);
  // A change that doesn't exist is refused in words.
  await page.getByRole('textbox', { name: 'Change number' }).fill('99');
  await page.getByRole('button', { name: 'Go', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'There is no change #99.' })).toBeAttached();
});

test('history graph: no drafts to draw yet', async ({ page }) => {
  await page.route('**/api/admin/drafts/graph?*', (route) => route.fulfill({ json: { head: 9, drafts: [] } }));
  await page.goto(`${ADMIN}/history?tab=graph&sw=off`);
  await expect(page.getByRole('heading', { name: 'No drafts to draw yet' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'How the graph reads' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'New draft' })).toHaveAttribute('href', '/drafts');
  await page.getByRole('button', { name: 'Open Timeline' }).click();
  await expect(page.getByRole('tab', { name: /^Timeline/ })).toHaveAttribute('aria-selected', 'true');
});

test('history graph: Drafts links to the draft in the graph', async ({ page }) => {
  await page.goto(`${ADMIN}/drafts?item=d-carling&sw=off`);
  await expect(page.getByRole('link', { name: 'See them in History' })).toHaveAttribute('href', '/history?tab=graph&draft=d-carling');
  await page.goto(`${ADMIN}/history?tab=graph&draft=d-carling&sw=off`);
  const draft = page.getByRole('complementary', { name: 'Draft Carling retime' });
  await expect(draft).toContainText('Conflicts · 2');
  await expect(row(page, 't-d-carling')).toHaveAttribute('aria-selected', 'true');
});

// An answer's record carries no run row: the summary names the run from the week, says the answer once.
test('history: an answer reads "→ on for <run>" in the Graph, the Timeline and the change pane', async ({ page }) => {
  await board(page);
  const week = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number; runs: { id: string; participants: { id: string; name: string; answer: string }[] }[] };
  const who = week.runs.find((r) => r.id === 'r-carling')!.participants.find((p) => p.answer !== 'yes')!;
  const answered = await page.request.post(`${ADMIN}/api/admin/runs/r-carling/rsvp`, { headers: await csrf(page.request), data: { member_id: who.id, answer: 'yes', version: week.version } });
  expect(answered.ok()).toBe(true);
  const line = new RegExp(`${who.name}(?: \\(\\d\\))? → on for HCarling \\+ HStar`);
  await graph(page);
  const head = row(page, 'r40');
  await expect(head.locator('.gx__l2')).toHaveText(line);
  await expect(head).not.toContainText('on on');
  await expect(head).not.toContainText('r-carling');
  await head.click();
  await expect(page.getByRole('complementary', { name: 'Change details' }).getByRole('heading', { level: 2 })).toHaveText(line);
  await page.getByRole('tab', { name: /^Timeline/ }).click();
  await expect(page.locator('[data-history="40"] .history-row__summary').first()).toHaveText(line);
});

test.describe('phone', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('history graph: chips fold into the row, a tip opens the bottom sheet, a change its page', async ({ page }) => {
    await board(page);
    await graph(page);
    await expect(page.locator('td.gx__drafts')).toHaveCount(0);
    await expect(row(page, 'r38').locator('td.gx__change')).toContainText('Bellona clean-up starts here');
    await row(page, 't-d-weekend').click();
    const sheet: Locator = page.getByRole('dialog', { name: 'Weekend reshuffle' });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole('link', { name: 'Open draft' })).toBeVisible();
    await expect(sheet).toContainText('Conflicts · 2');
    await sheet.getByRole('button', { name: 'Close' }).first().click();
    await expect(sheet).toBeHidden();
    await row(page, 'r38').click();
    const change = page.getByRole('dialog', { name: 'Change #38' });
    await expect(change).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(change).toBeHidden();
    // The document never scrolls: only the graph does.
    expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight)).toBe(true);
  });
});
