import type { Page } from '@playwright/test';
import type { PastItem, PastPage } from '@kanade/api-types';
import { ADMIN, expect, test } from './support';

// The Inbox's read-only Past tab (GET /api/admin/inbox/past): closed
// proposals and member requests, newest closed first, with the outcome,
// who decided, when, why, the cited messages and links to History.

const PHONE = { width: 390, height: 844 };
const PAST = /\/api\/admin\/inbox\/past(\?|$)/;

async function allPast(page: Page): Promise<PastItem[]> {
  const res = await page.request.get(`${ADMIN}/api/admin/inbox/past?limit=200`);
  return ((await res.json()) as PastPage).items;
}

const pastList = (page: Page) => page.getByRole('listbox', { name: 'Past items' });

test('Past loads only when its tab opens, newest closed first, every outcome in words', async ({ page }) => {
  const calls: string[] = [];
  page.on('request', (r) => {
    if (PAST.test(r.url())) calls.push(r.url());
  });
  await page.goto(`${ADMIN}/inbox?sw=off`);
  await expect(page.getByRole('listbox', { name: 'Extractor items' })).toBeVisible();
  const tab = page.getByRole('tab', { name: 'Past' });
  await expect(tab).toHaveText('Past');
  expect(calls).toEqual([]);

  await tab.click();
  await expect(page).toHaveURL(/[?&]tab=past/);
  await expect(tab).toHaveAttribute('aria-selected', 'true');
  const items = await allPast(page);
  const at = items.map((i) => i.decided_at);
  expect([...at].sort().reverse()).toEqual(at);
  const options = pastList(page).getByRole('option');
  await expect(options).toHaveCount(items.length);
  for (const [n, i] of items.entries()) {
    await expect(options.nth(n)).toContainText(i.summary);
    await expect(options.nth(n)).toContainText(i.kind_label);
  }
  for (const label of ['Approved', 'Rejected', 'Superseded', 'Discarded', 'Withdrawn', 'Expired']) {
    await expect(pastList(page).locator('.status-chip', { hasText: label }).first()).toBeVisible();
  }
  // Wide: the newest is open beside the list.
  await expect(options.first()).toHaveAttribute('aria-selected', 'true');
  expect(calls).toHaveLength(1);

  // Keyboard: the arrows move the selection, as on the live tabs.
  await pastList(page).focus();
  await page.keyboard.press('ArrowDown');
  await expect(options.nth(1)).toHaveAttribute('aria-selected', 'true');
  await expect(page).toHaveURL(new RegExp(`item=${items[1]!.id}`));
});

test('a closed item reads as its outcome, decider, reason, evidence and links, with no decision controls', async ({ page }) => {
  const items = await allPast(page);
  const approved = items.find((i) => i.outcome === 'approved' && i.history_seq !== null)!;
  await page.goto(`${ADMIN}/inbox?tab=past&item=${approved.id}&sw=off`);
  const detail = page.locator('.inbox__detail');
  await expect(detail.getByRole('heading', { level: 2 })).toHaveText(approved.summary);
  await expect(detail.locator('.past__sentence')).toContainText(`Approved by ${approved.decided_by!.name} · `);
  await expect(detail.getByRole('link', { name: `History record #${approved.history_seq}` })).toHaveAttribute('href', '/history');
  await expect(detail.getByRole('link', { name: 'Extraction log entry' })).toHaveAttribute('href', `/extractions/${approved.source_id}`);
  const card = detail.getByRole('link', { name: /See the card/ });
  await expect(card).toHaveAttribute('target', '_blank');
  await expect(card).toHaveAttribute('rel', /noopener/);
  // Evidence: every message opens in Discord; an uncached one says so and keeps its link.
  const messages = detail.locator('.msg');
  await expect(messages).toHaveCount(approved.evidence.length);
  for (const link of await detail.locator('.msg a.msg__at').all()) {
    await expect(link).toHaveAttribute('target', '_blank');
    await expect(link).toHaveAttribute('rel', /noopener/);
  }
  const gone = approved.evidence.find((e) => e.missing)!;
  const goneRow = messages.filter({ hasText: 'Message no longer cached' });
  await expect(goneRow).toHaveCount(1);
  await expect(goneRow.locator('a.msg__at')).toHaveAttribute('href', gone.url!);
  // Read-only: nothing to decide.
  await expect(page.locator('.inbox').getByRole('button', { name: /Approve|Reject|Move/ })).toHaveCount(0);

  // A reason, and a member request's requester; no History link without a record.
  const request = items.find((i) => i.tab === 'self_service' && i.reason)!;
  await pastList(page).getByRole('option', { name: new RegExp(request.summary) }).click();
  await expect(page).toHaveURL(new RegExp(`item=${request.id}`));
  await expect(detail.locator('.past__reason')).toContainText(request.reason!);
  await expect(detail.locator('.past__asked')).toContainText(request.requester!.name);
  await expect(detail.getByRole('link', { name: /History record/ })).toHaveCount(0);
  await expect(detail.getByRole('link', { name: /Extraction log entry|Chat interaction/ })).toHaveCount(0);

  // A chat proposal links its chat interaction.
  const chat = items.find((i) => i.source === 'chat')!;
  await pastList(page).getByRole('option', { name: new RegExp(chat.summary) }).click();
  await expect(detail.getByRole('link', { name: 'Chat interaction' })).toHaveAttribute('href', `/chat/${chat.source_id}`);
  // Kanade's own expiry names no actor.
  const expired = items.find((i) => i.outcome === 'expired')!;
  await pastList(page).getByRole('option', { name: new RegExp(expired.summary) }).click();
  await expect(detail.locator('.past__sentence')).toContainText(/Expired · \w{3} \d{2} \w{3} \d{2}:\d{2}/);
});

test('"Load older items" appends the next page until the last', async ({ page }) => {
  // Small pages, so the mock's ten items take four.
  await page.route(PAST, (route) => {
    const url = new URL(route.request().url());
    if (!url.searchParams.has('limit')) url.searchParams.set('limit', '3');
    return route.continue({ url: url.toString() });
  });
  const items = await allPast(page);
  await page.goto(`${ADMIN}/inbox?tab=past&sw=off`);
  const options = pastList(page).getByRole('option');
  await expect(options).toHaveCount(3);
  const more = page.getByRole('button', { name: 'Load older items' });
  for (const count of [6, 9, 10]) {
    await more.click();
    await expect(options).toHaveCount(count);
  }
  await expect(more).toHaveCount(0);
  await expect(pastList(page)).toBeFocused();
  expect(await options.evaluateAll((els) => els.map((el) => el.getAttribute('data-item')))).toEqual(items.map((i) => i.id));
});

test('Past says when nothing has closed, and shows a load error', async ({ page }) => {
  await page.route(PAST, (route) => route.fulfill({ json: { items: [], next_before: null } satisfies PastPage }));
  await page.goto(`${ADMIN}/inbox?tab=past&sw=off`);
  await expect(page.getByText('No closed items yet.')).toBeVisible();
  await expect(page.locator('.inbox__detail')).toBeHidden();

  await page.unroute(PAST);
  await page.route(PAST, (route) => route.fulfill({ status: 500, json: { error: 'internal', message: 'The store is unavailable.' } }));
  await page.goto(`${ADMIN}/inbox?tab=past&sw=off`);
  await expect(page.locator('.inbox').getByRole('alert')).toContainText('The store is unavailable.');
});

test('phone: Past list, then the detail with "‹ Inbox", forward and back', async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as unknown as { __animated: { target: string; from: string }[] };
    w.__animated = [];
    const original = Element.prototype.animate;
    Element.prototype.animate = function (this: Element, frames, options) {
      const first = Array.isArray(frames) ? (frames[0] as Record<string, unknown> | undefined) : undefined;
      w.__animated.push({ target: String(this instanceof HTMLElement ? this.className : this.tagName), from: String(first?.transform ?? '') });
      return original.call(this, frames, options);
    };
  });
  const animated = () => page.evaluate(() => (window as unknown as { __animated: { target: string; from: string }[] }).__animated.splice(0));
  await page.setViewportSize(PHONE);
  const items = await allPast(page);
  await page.goto(`${ADMIN}/inbox?tab=past&sw=off`);
  await expect(pastList(page)).toBeVisible();
  // Phones open nothing until a row is picked.
  await expect(page.locator('.inbox__detail')).toBeHidden();
  await animated();
  const target = items[2]!;
  await pastList(page).getByRole('option', { name: new RegExp(target.summary) }).click();
  await expect(page).toHaveURL(new RegExp(`tab=past&item=${target.id}`));
  await expect(pastList(page)).toBeHidden();
  await expect(page.locator('.inbox--compact')).toHaveCount(1);
  await expect(page.locator('.inbox__detail')).toBeFocused();
  await expect(page.locator('.inbox__detail').getByRole('heading', { level: 2 })).toHaveText(target.summary);
  expect((await animated()).some((a) => a.target.includes('inbox__detail') && a.from === 'translateX(24px)')).toBe(true);
  // The document never scrolls; the item is the one scroller.
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight)).toBe(true);

  await page.locator('.topbar').getByRole('button', { name: 'Back to the list (Inbox)' }).click();
  await expect(pastList(page)).toBeVisible();
  await expect(page).not.toHaveURL(/item=/);
  await expect(pastList(page)).toBeFocused();
  expect((await animated()).some((a) => a.target.includes('inbox__list') && a.from === 'translateX(-24px)')).toBe(true);
  // Forward reopens it.
  await page.goForward();
  await expect(page.locator('.inbox__detail').getByRole('heading', { level: 2 })).toHaveText(target.summary);
});
