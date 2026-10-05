import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import type { ChatTurn } from '@kanade/api-types';
import { ADMIN, expect, settle, test, unconditional } from './support';

// Chat turn facts (persona, reply profile, per-round model/effort/route/latency,
// calls grouped by round), the collapsed admin-only Model view of a masked turn,
// and the Extractions identity-leak filter. Mock: `c-when` is the masked turn.

async function go(page: Page, path: string) {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

async function serious(page: Page, label: string) {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical' || v.id === 'target-size');
  expect(bad.map((v) => `${label}: ${v.id} (${v.impact}) ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

/** Serves the mock's turn with `patch` applied: states the mock does not seed. */
async function patchTurn(page: Page, id: string, patch: (turn: ChatTurn) => void) {
  await page.route(`**/api/admin/chat/${id}`, async (route) => {
    const response = await route.fetch(unconditional(route));
    const turn = (await response.json()) as ChatTurn;
    patch(turn);
    await route.fulfill({ response, json: turn });
  });
}

test('model view: a masked turn ends with a collapsed, admin-only Model view that opens', async ({ page }) => {
  await go(page, '/chat/c-when');
  const panel = page.getByRole('tabpanel', { name: /Conversation/ });
  await expect(panel.getByText('External (masked)')).toBeVisible();
  await expect(panel.getByText('default voice (default)')).toBeVisible();
  await expect(panel.getByText('pseudonymized')).toBeVisible();

  const toggle = panel.getByRole('button', { name: 'Model view (masked)' });
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  const region = page.locator(`#${await toggle.getAttribute('aria-controls')}`);
  await expect(region).toBeHidden();
  await expect(panel.getByText(/Admin only, sensitive/)).toBeVisible();
  // Last in the turn, its warning after it (B_Chat): nothing of the conversation follows.
  expect(
    await panel.evaluate((el) => [el.lastElementChild?.classList.contains('chat-modelview__note'), el.lastElementChild?.previousElementSibling?.classList.contains('chat-modelview')]),
  ).toEqual([true, true]);

  await toggle.focus();
  await page.keyboard.press('Enter');
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  await expect(region).toBeVisible();
  const mapping = region.getByRole('table', { name: /Fake names/ });
  await expect(mapping.getByRole('columnheader')).toHaveText(['Fake name', 'Member']);
  await expect(mapping.getByRole('row', { name: 'Haruka Ren' })).toBeVisible();
  await expect(region.getByText('Haruka: @Kanade when is carling this week').first()).toBeVisible();
  await expect(region.locator('pre', { hasText: /"tool_call_id": "call_1"/ })).toBeVisible();
  await expect(region.locator('pre', { hasText: 'Tuesday 22:00, Haruka — 4 of 7' })).toBeVisible();
  await expect(region.locator('pre', { hasText: '{"bosses":["HCarling"]}' }).first()).toBeVisible();
  await expect(region.getByRole('heading', { name: 'Final reply members saw' })).toBeVisible();
  expect(await region.locator('pre').first().evaluate((el) => getComputedStyle(el).whiteSpace)).toBe('pre-wrap');
  await serious(page, 'model view open');

  await page.keyboard.press('Space');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(region).toBeHidden();
});

test('model view: unmasked external and passthrough turns show none; a withheld masked turn says why', async ({ page }) => {
  await go(page, '/chat/c-move');
  await expect(page.getByRole('tabpanel', { name: /Conversation/ }).getByText('Homelab')).toBeVisible();
  await expect(page.getByText(/Model view/)).toHaveCount(0);

  await patchTurn(page, 'c-blocked', (t) => {
    t.route = 'external_unmasked';
    t.masked = false;
    t.model_view = null;
  });
  await go(page, '/chat/c-blocked');
  await expect(page.getByRole('tabpanel', { name: /Conversation/ }).getByText('External (unmasked)')).toBeVisible();
  await expect(page.getByText(/Model view/)).toHaveCount(0);

  await patchTurn(page, 'c-withheld', (t) => {
    t.masked = true;
    t.model_view = null;
  });
  await go(page, '/chat/c-withheld');
  await expect(page.getByText('Model view unavailable for withheld questions.')).toBeVisible();
  await expect(page.getByRole('button', { name: /Model view/ })).toHaveCount(0);
});

test('turn facts: rounds show model, effort, route and latency; calls sit under their round', async ({ page }) => {
  await go(page, '/chat/c-guide');
  await expect(page.getByText('gentle (saved)')).toBeVisible();
  await page.getByRole('tab', { name: /Model trace/ }).click();
  // B_ChatTrace: one card per round; the round's calls sit inside it.
  const first = page.getByRole('region', { name: 'Round 1' });
  await expect(first.locator('.chat-round__head > *')).toHaveText(['Round 1', 'kanata/chat', 'effort low', 'Homelab', '3.0 s']);
  await expect(first.locator('dd')).toHaveText(['tool_calls', 'knowledge.read', '—', 'kanade-chat-1a2b3c4d-7-1, kanade-chat-1a2b3c4d-7-2']);
  await expect(first.locator('.chat-tool')).toContainText(['knowledge.read']);
  await expect(first.locator('.chat-tool')).toContainText('12 ms');
  await expect(first.locator('.chat-tool__return')).toContainText('Return: “');
  await expect(page.getByRole('region', { name: 'Round 2' }).locator('.chat-tool')).toHaveCount(0);
  await serious(page, 'chat turn model trace');

  await page.getByRole('tab', { name: /Tool trace/ }).click();
  const trace = page.getByRole('table', { name: 'Tool calls' });
  const group = trace.locator('tbody').first();
  await expect(group.getByRole('rowheader').first()).toHaveText('Round 1 · kanata/chat · effort low · Homelab · 3.0 s');
  await expect(group.getByRole('row', { name: /knowledge\.read/ })).toBeVisible();
  await serious(page, 'chat turn tool trace');
});

test('turn facts: an unknown time reads "unknown"; a failed turn shows its error and code', async ({ page }) => {
  await patchTurn(page, 'c-guide', (t) => {
    t.tools[0]!.took_ms = null;
    t.rounds[1]!.latency_ms = null;
  });
  await go(page, '/chat/c-guide');
  await page.getByRole('tab', { name: /Tool trace/ }).click();
  await expect(page.getByRole('row', { name: /knowledge\.read/ }).getByRole('cell', { name: 'unknown' })).toBeVisible();
  await page.getByRole('tab', { name: /Model trace/ }).click();
  await expect(page.getByRole('region', { name: 'Round 2' }).locator('.chat-round__latency')).toHaveText('unknown');

  await go(page, '/chat/c-error');
  const failed = page.getByRole('paragraph').filter({ hasText: /^Failed:/ });
  await expect(failed).toHaveText('Failed: no answer within 60s (timeout)');
});

test('extractions: the identity-leak outcome filters to the refused calls', async ({ page }) => {
  await go(page, '/extractions');
  await page.getByRole('button', { name: 'Filters (0)' }).click();
  await page.getByRole('group', { name: 'Filters' }).getByRole('checkbox', { name: 'identity leak blocked' }).check();
  await expect(page).toHaveURL(/outcome=identity_leak/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('3 of 34 model calls');
  const calls = page.getByRole('listbox', { name: /Extraction calls/ });
  await expect(calls.getByRole('option')).toHaveCount(3);
  await expect(calls.getByText('identity leak blocked')).toHaveCount(3);
  await serious(page, 'extractions identity leak');
});

test('model view: at 390×844 only the tab panel scrolls, even when open', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/chat/c-when');
  await page.getByRole('button', { name: 'Model view (masked)' }).click();
  const panel = page.getByRole('tabpanel', { name: /Conversation/ });
  const tab = page.getByRole('tab', { name: /Conversation/ });
  const before = await tab.boundingBox();
  const got = await panel.evaluate((el) => {
    const doc = document.scrollingElement!;
    window.scrollTo(0, 400);
    el.scrollTop = el.scrollHeight;
    return { doc: doc.scrollTop, docFits: doc.scrollHeight <= doc.clientHeight + 1, panel: el.scrollTop, scrolls: el.scrollHeight > el.clientHeight };
  });
  expect(got).toEqual({ doc: 0, docFits: true, panel: expect.any(Number), scrolls: true });
  expect(got.panel).toBeGreaterThan(0);
  // Sub-pixel layout can settle by under a pixel; the strip must not scroll.
  expect(Math.abs((await tab.boundingBox())!.y - before!.y)).toBeLessThanOrEqual(1);
  await expect(page.getByRole('heading', { name: 'Final reply members saw' })).toBeInViewport();
});
