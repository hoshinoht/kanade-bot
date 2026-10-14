import type { Page, Request } from '@playwright/test';
import { ADMIN, expect, settle, test } from './support';

// Admin presence (boards docs/research/2026-10-12-graph-presence-mockups B_Presence*).
// The mock has one admin (this page); the other admins are simulated:
// `POST /__mock/presence` puts one at a place (aging in real time unless held),
// `/__mock/presence/board` seeds the boards (Ren editing request p-kalos-fixed
// with Kaito viewing it 40 s ago, Ren editing run r-carling; held).

const REQUEST = '/inbox?tab=self_service&item=p-kalos-fixed';
const PRESENCE = '/api/admin/presence';

async function board(page: Page) {
  expect((await page.request.post(`${ADMIN}/__mock/presence/board`)).status()).toBe(204);
}

async function arrive(page: Page, who: { at: string; id: string; name: string; editing?: boolean; seen_s?: number; held?: boolean; leave?: boolean }) {
  expect((await page.request.post(`${ADMIN}/__mock/presence`, { data: who })).status()).toBe(204);
}

/**
 * Every presence call the page makes, as `METHOD body-or-query` with its `seq`
 * taken out; the seqs, in call order, are `.seqs` (GET reads carry none).
 */
function calls(page: Page): string[] & { seqs: number[] } {
  const seen = Object.assign([] as string[], { seqs: [] as number[] });
  page.on('request', (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname !== PRESENCE) return;
    if (request.method() === 'PUT') {
      const { seq, ...body } = request.postDataJSON() as { seq: number };
      seen.seqs.push(seq);
      seen.push(`PUT ${JSON.stringify(body)}`);
      return;
    }
    const seq = url.searchParams.get('seq');
    if (seq !== null) seen.seqs.push(Number(seq));
    url.searchParams.delete('seq');
    seen.push(`${request.method()} ${url.search}`);
  });
  return seen;
}

async function inbox(page: Page) {
  await page.goto(`${ADMIN}${REQUEST}&sw=off`);
  await expect(page.getByRole('heading', { level: 2, name: /Weekly timing change/ })).toBeVisible();
}

/** HCarling + HStar in the full sheet (the pane's larger view on a laptop, the sheet on a phone). */
async function carlingSheet(page: Page) {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  if (page.viewportSize()!.width >= 900) {
    await page.getByRole('complementary', { name: 'HCarling + HStar' }).getByRole('button', { name: 'Open in a larger view' }).click();
  }
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await expect(sheet).toBeVisible();
  return sheet;
}

test('presence: Inbox request shows who else is here, as words, and never locks', async ({ page }) => {
  const seen = calls(page);
  await board(page);
  await inbox(page);
  const button = page.getByRole('button', { name: 'Other admins here: Ren, editing; Kaito, viewing, last seen 40 seconds ago' });
  await expect(button).toBeVisible();
  await expect(button).toContainText('Ren editing');
  await expect(button).toContainText('Kaito viewing');
  await expect(button).toHaveAttribute('aria-expanded', 'false');
  // The first check-in says this page is viewing the request.
  expect(seen[0]).toBe(`PUT {"at":{"kind":"request","id":"p-kalos-fixed"},"mode":"viewing"}`);
  // The note near the action says what the version check will do; nothing is disabled.
  await expect(page.getByRole('complementary', { name: 'Decide this change' }).locator('.presence-note')).toContainText('Ren is editing this too. The first approve wins; the other is refused and starts over.');
  await expect(page.getByRole('button', { name: 'Approve', exact: true })).toBeEnabled();

  // The popover: one row each, with mode and age; Escape closes it back onto the button.
  await button.click();
  const pop = page.getByRole('dialog', { name: 'Admins here' });
  await expect(pop).toContainText('Here now · admins only');
  await expect(pop.locator('.presence__row')).toHaveText([/Ren · editing this request\s*now/, /Kaito · viewing\s*40 s ago/]);
  await expect(pop.locator('.presence__row--stale')).toHaveCount(1);
  await expect(pop).toContainText('A hint, not a lock.');
  await page.keyboard.press('Escape');
  await expect(pop).toBeHidden();
  await expect(button).toBeFocused();

  // Editing before approving checks in as editing at once; Approve edited stays enabled.
  await page.getByRole('button', { name: /Edit before approving/ }).click();
  await expect.poll(() => seen.at(-1)).toBe(`PUT {"at":{"kind":"request","id":"p-kalos-fixed"},"mode":"editing"}`);
  await expect(page.getByRole('button', { name: 'Approve edited' })).toBeEnabled();
  await expect(page.locator('.presence-note')).toBeVisible();
});

test('presence: a second admin arriving shows at once, and starting to edit is announced once', async ({ page }) => {
  await inbox(page);
  await page.getByRole('button', { name: /Edit before approving/ }).click();
  await expect(page.locator('[data-fid="presence"], .presence__btn')).toHaveCount(0);
  // Kaito opens the request: the presence hint re-reads without waiting for a check-in.
  await arrive(page, { at: 'request:p-kalos-fixed', id: 'discord:1009', name: 'Kaito' });
  await expect(page.getByRole('button', { name: 'Other admins here: Kaito, viewing' })).toBeVisible();
  await expect(page.locator('.presence-note')).toHaveCount(0);
  // Ren starts editing what this page edits: one polite announcement, and the note.
  await arrive(page, { at: 'request:p-kalos-fixed', id: 'discord:1002', name: 'Ren', editing: true });
  await expect(page.getByRole('status').filter({ hasText: 'Ren started editing this request' })).toHaveCount(1);
  await expect(page.locator('.presence-note')).toContainText('Ren is editing this too.');
  // Ren leaves.
  await arrive(page, { at: 'request:p-kalos-fixed', id: 'discord:1002', name: 'Ren', leave: true });
  await expect(page.getByRole('button', { name: 'Other admins here: Kaito, viewing' })).toBeVisible();
});

test('presence: run sheet title bar and strip; Move, Swap and status stay enabled', async ({ page }) => {
  const seen = calls(page);
  await board(page);
  const sheet = await carlingSheet(page);
  const button = sheet.getByRole('button', { name: 'Other admins here: Ren, editing' });
  await expect(button).toBeVisible();
  await expect(sheet.locator('.modal__head .presence__btn')).toHaveCount(1);
  await expect(sheet.locator('.presence-note--strip')).toContainText('Ren is editing this run too. Nothing is locked: if Ren’s change lands first, yours is refused as out of date and the sheet shows the new run.');
  await expect(sheet.getByRole('button', { name: /^Move/ })).toBeEnabled();
  await expect(sheet.getByRole('button', { name: /Swap/ }).first()).toBeEnabled();
  await expect(sheet.getByRole('group', { name: 'Status' }).getByRole('button').first()).toBeEnabled();
  expect(seen[0]).toBe(`PUT {"at":{"kind":"run","id":"r-carling"},"mode":"viewing"}`);
  // Move is editing.
  await sheet.getByRole('button', { name: /^Move/ }).click();
  await expect.poll(() => seen.at(-1)).toBe(`PUT {"at":{"kind":"run","id":"r-carling"},"mode":"editing"}`);
  // Closing the sheet leaves.
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect.poll(() => seen.at(-1)).toBe('DELETE ?at=run%3Ar-carling');
  // Every call's seq is newer than the last, the leave's included.
  expect(seen.seqs.every((seq, i) => i === 0 || seq > seen.seqs[i - 1]!)).toBe(true);
});

test('presence: phone rows carry the note (Inbox and run sheet)', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await board(page);
  await inbox(page);
  const row = page.locator('.presence--wide .presence__btn');
  await expect(row).toContainText('Ren is editing this too · Kaito viewing, 40 s ago');
  await expect(row).toContainText('The first approve wins; the other is refused.');
  await expect(page.locator('.presence-note')).toHaveCount(0);

  const sheet = await carlingSheet(page);
  const sheetRow = sheet.locator('.presence--wide .presence__btn');
  await expect(sheetRow).toContainText('Ren is editing this run too');
  await expect(sheetRow).toContainText('Nothing is locked: if Ren’s change lands first, yours is refused and the sheet shows the new run.');
  await settle(page);
});

test('presence: checks in every 15 s while visible, stops while hidden, fades and drops by its age', async ({ page }) => {
  await page.clock.install();
  const seen = calls(page);
  // Mika viewing, last seen 55 s ago (aging): gone at 60 s without a check-in.
  await arrive(page, { at: 'request:p-kalos-fixed', id: 'discord:1003', name: 'Mika', seen_s: 55 });
  await inbox(page);
  const mika = page.getByRole('button', { name: /^Other admins here: Mika, viewing, last seen 50 seconds ago/ });
  await expect(mika).toBeVisible();
  await page.clock.runFor(6_000);
  await expect(mika).toBeHidden();

  const puts = () => seen.filter((call) => call.startsWith('PUT')).length;
  await expect.poll(puts).toBe(1);
  await page.clock.runFor(15_500);
  await expect.poll(puts).toBe(2);
  // Hidden: no check-ins, whatever time passes.
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await page.clock.runFor(60_000);
  expect(puts()).toBe(2);
  // Shown again: it checks in at once.
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect.poll(puts).toBe(3);
});

test('presence: leaving the request sends a DELETE', async ({ page }) => {
  const seen = calls(page);
  await inbox(page);
  await expect.poll(() => seen.length).toBeGreaterThan(0);
  const left = page.waitForRequest((request) => request.method() === 'DELETE' && request.url().includes(`${PRESENCE}?at=request%3Ap-kalos-fixed`));
  await page.getByRole('option', { name: /Join — HCarling/ }).first().click();
  expect((await (await left).response())?.status()).toBe(204);
  // The next request checks in instead.
  await expect.poll(() => seen.at(-1)).toMatch(/^PUT \{"at":\{"kind":"request","id":"(?!p-kalos-fixed)/);
});

test('presence: Draft detail never checks in (drafts are their author’s only)', async ({ page }) => {
  const seen = calls(page);
  await page.goto(`${ADMIN}/drafts?item=d-weekend&sw=off`);
  await expect(page.getByRole('heading', { level: 2, name: 'Weekend reshuffle' })).toBeVisible();
  await page.waitForLoadState('networkidle');
  expect([...seen]).toEqual([]);
  await expect(page.locator('.presence')).toHaveCount(0);
});
