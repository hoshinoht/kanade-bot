import type { Page } from '@playwright/test';
import { ADMIN, csrf, expect, test } from './support';

// Knowledge (tracked boss/knowledge, schema v2), Inbox, Extractions + rescan,
// Chat, Limits, History (revert / restore / by member / checkpoints) and the
// run sheet's blame panel. Mock clock pinned (playwright.config.ts).

async function go(page: Page, path: string) {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

const toast = (page: Page, text: string | RegExp) => page.getByRole('group', { name: 'Notification' }).filter({ hasText: text });

test('knowledge: opens on the difficulty the guild runs, switches, credits sources', async ({ page }) => {
  await go(page, '/bosses/MaleficStar/knowledge');
  const switcher = page.getByRole('group', { name: 'Difficulty' });
  await expect(switcher.getByRole('button', { name: /^Hard/ })).toHaveAttribute('aria-pressed', 'true');
  const force = page.locator('.knowledge-facts > div').filter({ has: page.getByText('Sacred force', { exact: true }) });
  await expect(page.getByRole('heading', { name: 'Hard facts' })).toBeAttached();
  await expect(force.locator('dd')).toHaveText('550');
  await switcher.getByRole('button', { name: /^Normal/ }).click();
  await expect(page.getByRole('heading', { name: 'Normal facts' })).toBeAttached();
  await expect(force.locator('dd')).toHaveText('400');
  await expect(page.getByText(/by iSIingGunz · guide · fetched \d{4}-\d{2}-\d{2}/).first()).toBeVisible();

  await go(page, '/bosses/MaleficStar/knowledge?difficulty=n');
  await expect(page.getByRole('group', { name: 'Difficulty' }).getByRole('button', { name: /^Normal/ })).toHaveAttribute('aria-pressed', 'true');

  await go(page, '/bosses');
  await page.getByRole('link', { name: 'Kai' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'Kai', exact: true })).toBeVisible();
  await expect(page.locator('.knowledge-hero .status-chip')).toHaveText('Seasonal boss · CW3');
  await expect(page.getByText('Event boss.')).toBeVisible();
  await expect(page.locator('.knowledge-facts > div').filter({ has: page.getByText('Party', { exact: true }) }).locator('dd')).toHaveText('Solo only');
});

test('inbox: extractor tab — list and detail, edit then approve, reject, a chat proposal', async ({ page }) => {
  await go(page, '/inbox');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 changes waiting');
  const tabs = page.getByRole('tablist', { name: 'Inbox' });
  await expect(tabs.getByRole('tab', { name: /Extractor/ })).toHaveAttribute('aria-selected', 'true');
  await expect(tabs.getByRole('tab', { name: /Extractor/ })).toContainText('3');
  await expect(tabs.getByRole('tab', { name: /Self-service/ })).toContainText('6');
  const list = page.getByRole('listbox', { name: 'Extractor items' });
  await expect(list.getByRole('option')).toHaveCount(3);
  // Wide screens open the first item; the list is keyboard-navigable and deep-linked.
  const detail = page.locator('.inbox__detail');
  await expect(detail.getByRole('heading', { level: 2 })).toContainText('Black Mage');
  await expect(detail).toContainText('Read from chat');
  await expect(detail.getByLabel('Evidence')).toContainText('tue cannot, wed same time ok?');
  await list.focus();
  await page.keyboard.press('ArrowDown');
  await expect(page).toHaveURL(/tab=extractor&item=p-limbo-add/);
  await expect(detail.getByRole('heading', { level: 2 })).toContainText('New run');
  await page.keyboard.press('ArrowUp');

  // One approval at a corrected time: day and HH:MM in the proposal's own boss week.
  await detail.getByRole('textbox', { name: 'Edit, then approve' }).fill('soon');
  await detail.getByRole('button', { name: 'Move & approve' }).click();
  await expect(detail.getByRole('alert')).toContainText('Write a day');
  const sent = page.waitForRequest((r) => r.method() === 'POST' && r.url().endsWith('/api/admin/inbox/p-bm-move/approve'));
  await detail.getByRole('textbox', { name: 'Edit, then approve' }).fill('wed 22:30');
  await detail.getByRole('button', { name: 'Move & approve' }).click();
  expect((await sent).postDataJSON()).toEqual({ version: 1, day: 6, time: '22:30' });
  await expect(toast(page, 'Approved: move #a7c1e9d2.')).toBeVisible();
  await expect(list.getByRole('option')).toHaveCount(2);

  // Kanade's proposals reject without a reason, as in v4: no reason field at all.
  await expect(detail.getByRole('heading', { level: 2 })).toContainText('New run');
  await detail.getByRole('button', { name: 'Reject…' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('textbox', { name: 'Reason' })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Reject change' }).click();
  await expect(toast(page, 'Rejected: new run #c8e0a2b4.')).toBeVisible();

  // Asked of Kanade in chat: approved like any proposal.
  await expect(detail).toContainText('Asked of Kanade');
  await detail.getByRole('button', { name: 'Approve', exact: true }).click();
  await expect(toast(page, 'Approved: move #b2c4d6e8.')).toBeVisible();
  // An empty tab says why, visibly, not just that it is empty.
  await expect(page.getByText('Nothing waiting here. The extractor posts a card when it reads a change in a watched channel.')).toBeVisible();

  await page.getByRole('link', { name: 'Week' }).click();
  const wed = page.locator('section.board__col').filter({ has: page.locator('h2 .board__dow:text-is("Wed")') });
  await expect(wed.locator('[data-run="r-bm"]')).toContainText('22:30');
});

test('inbox: a token or Tailscale session cannot decide proposals, but can decide requests', async ({ page }) => {
  const switched = await page.request.post(`${ADMIN}/__mock/session`, { data: { method: 'token' } });
  expect(switched.status()).toBe(204);
  const approvals: string[] = [];
  page.on('request', (r) => {
    if (r.method() === 'POST' && /\/api\/admin\/inbox\/p-[a-z-]+\/approve$/.test(r.url())) approvals.push(r.url());
  });
  await go(page, '/inbox');
  const detail = page.locator('.inbox__detail');
  await expect(detail.getByRole('heading', { level: 2 })).toContainText('Black Mage');
  // The session says how it signed in: proposal decisions are off before any attempt.
  await expect(detail.getByRole('button', { name: 'Approve', exact: true })).toBeDisabled();
  await expect(detail.getByRole('button', { name: 'Reject…' })).toBeDisabled();
  await expect(detail.getByRole('button', { name: 'Move & approve' })).toBeDisabled();
  await expect(detail).toContainText("Sign in with Discord to approve or reject Kanade's proposals. Members' requests can still be decided here.");
  expect(approvals).toEqual([]);
  const list = page.getByRole('listbox', { name: 'Extractor items' });
  await expect(list.getByRole('option')).toHaveCount(3);

  await page.getByRole('tab', { name: /Self-service/ }).click();
  await page.getByRole('listbox', { name: 'Self-service items' }).getByRole('option', { name: /HFA/ }).click();
  await detail.getByRole('button', { name: 'Reject…' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('textbox', { name: 'Reason' }).fill('The run moved; ask again.');
  await dialog.getByRole('button', { name: 'Reject change' }).click();
  await expect(toast(page, 'Rejected: leave #e5f7a9b1.')).toBeVisible();
});

test('inbox: a 403 discord_session_required still locks proposals when the session did not say its method', async ({ page }) => {
  await page.request.post(`${ADMIN}/__mock/session`, { data: { method: 'token' } });
  // An older session answer without `method`: the refusal is the fallback.
  await page.route(`${ADMIN}/api/admin/session`, async (route) => {
    const response = await route.fetch();
    await route.fulfill({ response, json: { display: 'Break-glass token' } });
  });
  await go(page, '/inbox');
  const detail = page.locator('.inbox__detail');
  await expect(detail.getByRole('heading', { level: 2 })).toContainText('Black Mage');
  await detail.getByRole('button', { name: 'Approve', exact: true }).click();
  await expect(detail.getByRole('alert')).toHaveText("Sign in with Discord to approve or reject Kanade's proposals.");
  await expect(detail.getByRole('button', { name: 'Approve', exact: true })).toBeDisabled();
  await expect(detail.getByRole('button', { name: 'Reject…' })).toBeDisabled();
});

test('inbox: self-service tab — request types, badges, conflicts, choices, reasons and refusals', async ({ page }) => {
  await go(page, '/inbox?tab=self_service');
  const list = page.getByRole('listbox', { name: 'Self-service items' });
  await expect(list.getByRole('option')).toHaveCount(6);
  await expect(list.getByRole('option', { name: /XKalos/ }).first()).toContainText('expired');
  await expect(list.getByRole('option', { name: /HFA/ })).toContainText('conflict');
  await expect(list.getByRole('option', { name: /HJupiter/ })).toContainText('requester not allowed');
  await expect(list.getByRole('option', { name: /HJupiter/ })).toContainText('already in effect');
  const detail = page.locator('.inbox__detail');

  // A member asking to join this week's run: preview, the member's summary, approve.
  await list.getByRole('option', { name: /HCarling/ }).click();
  await expect(page).toHaveURL(/tab=self_service&item=p-carling-link/);
  await expect(detail.getByRole('heading', { level: 2 })).toContainText('Join');
  await expect(detail).toContainText('Sent by Nagi as a member request.');
  await expect(detail).toContainText('The member sees: “member request: join HCarling + HStar Tue 29 Sep 22:00”');
  await expect(detail.locator('.proposal__changes')).toContainText('Nagi');
  await expect(detail.getByRole('textbox', { name: 'Edit, then approve' })).toHaveCount(0);

  // A conflict always blocks: no "approve anyway", only reject.
  await list.getByRole('option', { name: /HFA/ }).click();
  await expect(detail.getByRole('group', { name: 'Changed since the member asked' })).toContainText('it was based on');
  await expect(detail.getByRole('button', { name: 'Approve', exact: true })).toBeDisabled();
  await expect(detail).toContainText('cannot be approved; reject it');
  await expect(detail.getByRole('checkbox')).toHaveCount(0);

  // Expired items can only be rejected, and say why; requests need a reason (1–500 characters).
  await list.getByRole('option', { name: /Swap/ }).click();
  await expect(detail.getByRole('button', { name: 'Approve', exact: true })).toBeDisabled();
  await expect(detail).toContainText('It expired; it can only be rejected.');
  await detail.getByRole('button', { name: 'Reject…' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Reject change' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Say why');
  await dialog.getByRole('textbox', { name: 'Reason' }).fill('It expired before the run.');
  await dialog.getByRole('button', { name: 'Reject change' }).click();
  await expect(toast(page, 'Rejected: swap #f6a8b0c2.')).toBeVisible();

  // A weekly-timing change lists only its amended runs, and always names its choices.
  await list.getByRole('option', { name: /Weekly timing change/ }).click();
  const refused = await page.request.post(`${ADMIN}/api/admin/inbox/p-kalos-fixed/approve`, { headers: await csrf(page.request), data: { version: 1 } });
  expect(refused.status()).toBe(422);
  expect(((await refused.json()) as { error: string }).error).toBe('choices_required');
  const choices = detail.getByRole('group', { name: 'Runs of this weekly timing' });
  await expect(choices.getByRole('radiogroup')).toHaveCount(1);
  await choices.getByRole('radio', { name: 'Keep as it is' }).check();
  const sent = page.waitForRequest((r) => r.url().endsWith('/api/admin/inbox/p-kalos-fixed/approve'));
  await detail.getByRole('button', { name: 'Approve', exact: true }).click();
  expect((await sent).postDataJSON()).toEqual({ version: 1, choices: { 'r-kalos': 'keep' } });
  await expect(toast(page, 'Approved: weekly timing change #d4e6f8a0.')).toBeVisible();

  await list.getByRole('option', { name: /New weekly run/ }).click();
  await detail.getByRole('button', { name: 'Approve', exact: true }).click();
  await expect(toast(page, 'Approved: new weekly run #c1d3e5f7.')).toBeVisible();

  // The API speaks the contract's codes.
  const post = async (path: string, data: object) => page.request.post(`${ADMIN}/api/admin/inbox/${path}`, { headers: await csrf(page.request), data });
  const code = async (r: Awaited<ReturnType<typeof post>>) => [r.status(), ((await r.json()) as { error: string }).error];
  expect(await code(await post('p-jupiter-same/approve', { version: 1 }))).toEqual([409, 'requester_unauthorised']);
  expect(await code(await post('p-carling-link/reject', { version: 7, reason: 'x' }))).toEqual([409, 'stale']);
  expect(await code(await post('p-carling-link/approve', { version: 1, force: true }))).toEqual([422, 'force_unsupported']);
  expect(await code(await post('p-carling-link/approve', {}))).toEqual([422, 'version_required']);
});

test('inbox on a phone: the list, then the detail with a back action', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/inbox?tab=self_service');
  const list = page.getByRole('listbox', { name: 'Self-service items' });
  await expect(list).toBeVisible();
  await expect(list).toHaveAttribute('tabindex', '0');
  await expect(list.getByRole('option').first()).toHaveAttribute('aria-selected', 'false');
  await expect(page.locator('.inbox__detail')).toBeHidden();
  await list.getByRole('option', { name: /HCarling/ }).click();
  await expect(list).toBeHidden();
  await expect(page.locator('.inbox__detail').getByRole('heading', { level: 2 })).toContainText('Carling');
  // B_PhoneInbox: "‹ Inbox" replaces the menu and title; no page line or tabs;
  // the decision is a bottom action bar with the pencil opening the edit field.
  const back = page.locator('.topbar').getByRole('button', { name: 'Back to the list (Inbox)' });
  await expect(back).toHaveText('Inbox');
  await expect(page.getByRole('button', { name: 'Open the navigation' })).toHaveCount(0);
  await expect(page.getByRole('tablist', { name: 'Inbox' })).toBeHidden();
  await expect(page.locator('.pageline')).toHaveClass(/pageline--echo/);
  const bar = page.locator('.inbox__detail').getByRole('complementary', { name: 'Decide this change' });
  const barBox = (await bar.boundingBox())!;
  expect(barBox.y + barBox.height).toBeGreaterThan(844 - 2);
  const pencil = bar.getByRole('button', { name: 'Show the edit field' });
  if (await pencil.count()) {
    await expect(bar.getByRole('textbox', { name: 'Edit, then approve' })).toBeHidden();
    await pencil.click();
    await expect(pencil).toHaveAttribute('aria-expanded', 'true');
    await expect(bar.getByRole('textbox', { name: 'Edit, then approve' })).toBeVisible();
  }
  await page.getByRole('button', { name: /Back to the list/ }).click();
  await expect(list).toBeVisible();
  await expect(page).not.toHaveURL(/item=/);
  // A deep link opens the detail directly; Back still returns to the list.
  await go(page, '/inbox?tab=self_service&item=p-fa-request');
  const detail = page.locator('.inbox__detail');
  await expect(detail.getByRole('heading', { level: 2 })).toContainText('First Adversary');
  const thread = detail.getByRole('region', { name: 'Proposal thread and changes' });
  const decision = detail.getByRole('complementary', { name: 'Decide this change' });
  await expect(decision).toBeInViewport();
  await thread.evaluate((element) => (element.scrollTop = element.scrollHeight));
  await expect(decision).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollTop || document.body.scrollTop)).toBe(0);
  await page.getByRole('button', { name: /Back to the list/ }).click();
  await expect(list).toBeVisible();
});

test('inbox on a phone: the action bar is focused in the order it is seen', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/inbox?tab=extractor&item=p-bm-move');
  const bar = page.locator('.inbox__detail').getByRole('complementary', { name: 'Decide this change' });
  const pencil = bar.getByRole('button', { name: 'Show the edit field' });
  const reject = bar.getByRole('button', { name: 'Reject…' });
  const approve = bar.getByRole('button', { name: 'Approve', exact: true });
  // Seen left to right: pencil, Reject…, Approve.
  const xs = await Promise.all([pencil, reject, approve].map(async (b) => (await b.boundingBox())!.x));
  expect(xs).toEqual([...xs].sort((a, b) => a - b));
  // Tab follows the same order (review-2 finding 2: no CSS `order` on controls).
  await pencil.focus();
  await page.keyboard.press('Tab');
  await expect(reject).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(approve).toBeFocused();
  // The edit field opens above the bar and comes before the pencil.
  await pencil.click();
  await expect(bar.getByRole('textbox', { name: 'Edit, then approve' })).toBeVisible();
  await pencil.focus();
  await page.keyboard.press('Shift+Tab');
  await expect(bar.getByRole('button', { name: 'Move & approve' })).toBeFocused();
});

test('inbox on a phone by keyboard: arrows move the active option, Enter opens, Back restores it', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/inbox?tab=self_service');
  const list = page.getByRole('listbox', { name: 'Self-service items' });
  const options = list.getByRole('option');
  await expect(options).toHaveCount(6);
  const detail = page.locator('.inbox__detail');
  // Read up front: the list (and its options) is hidden while a detail is open.
  const ids = await options.evaluateAll((els) => els.map((el) => el.id));
  const items = await options.evaluateAll((els) => els.map((el) => (el as HTMLElement).dataset.item));
  const idOf = (n: number) => ids[n]!;

  await list.focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  // Moving does not open anything: the list keeps focus and the URL has no item.
  await expect(list).toBeFocused();
  await expect(list).toHaveAttribute('aria-activedescendant', idOf(1));
  await expect(page).not.toHaveURL(/item=/);
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(new RegExp(`item=${items[1]}`));
  await expect(detail).toBeFocused();
  // Back by keyboard returns focus to the list, the opened option still active.
  // On a phone the back step is the top bar's "‹ Inbox", before the page (and the Inbox link).
  await page.keyboard.press('Shift+Tab');
  await page.keyboard.press('Shift+Tab');
  await expect(page.getByRole('button', { name: /Back to the list/ })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(list).toBeFocused();
  await expect(list).toHaveAttribute('aria-activedescendant', idOf(1));

  // A middle item, opened with Space; the browser's Back restores it too.
  await page.keyboard.press('ArrowDown');
  await expect(list).toHaveAttribute('aria-activedescendant', idOf(2));
  await page.keyboard.press(' ');
  await expect(page).toHaveURL(new RegExp(`item=${items[2]}`));
  await expect(detail).toBeFocused();
  await page.goBack();
  await expect(list).toBeFocused();
  await expect(list).toHaveAttribute('aria-activedescendant', idOf(2));
  // Forward lands on the detail with focus, not on the hidden list.
  await page.goForward();
  await expect(detail).toBeFocused();
  await page.goBack();
  await expect(list).toBeFocused();

  // A tap opens and focuses the detail the same way.
  await options.nth(3).click();
  await expect(detail).toBeFocused();
});

test('inbox on a phone: approving returns to the list without a dead Back step', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/');
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  await page.getByRole('link', { name: /^Inbox/ }).first().click();
  await page.getByRole('tab', { name: /Self-service/ }).click();
  const list = page.getByRole('listbox', { name: 'Self-service items' });
  const options = list.getByRole('option');
  await expect(options).toHaveCount(6);
  const ids = await options.evaluateAll((els) => els.map((el) => el.id));
  const carling = ids.findIndex((id) => id.endsWith('-p-carling-link'));
  const neighbour = ids[carling + 1] ?? ids[carling - 1]!;
  await options.nth(carling).click();
  await page.locator('.inbox__detail').getByRole('button', { name: 'Approve', exact: true }).click();
  await expect(toast(page, /Approved/)).toBeVisible();
  await expect(list).toBeVisible();
  await expect(list).toBeFocused();
  // The item after the approved one is active, ready for the next decision.
  await expect(list).toHaveAttribute('aria-activedescendant', neighbour);
  await expect(page).not.toHaveURL(/item=/);
  // A later pick restores itself on Back, not the earlier neighbour.
  const other = options.filter({ hasText: 'HJupiter' });
  const otherId = (await other.getAttribute('id'))!;
  expect(otherId).not.toBe(neighbour);
  await other.click();
  await page.getByRole('button', { name: /Back to the list/ }).click();
  await expect(list).toBeFocused();
  await expect(list).toHaveAttribute('aria-activedescendant', otherId);
  // One Back leaves the Inbox for the Week, not a copy of the list.
  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`^${ADMIN}/(\\?|$)`));
});

test('extractions: pager, detail tabs and a rescan job', async ({ page }) => {
  await go(page, '/extractions');
  await expect(page.getByText(/Page 1 of 2 · 34 calls/)).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('34 model calls');
  await page.getByRole('button', { name: 'Older →' }).click();
  await expect(page.getByText(/Page 2 of 2/)).toBeVisible();
  await page.getByRole('button', { name: '← Newer' }).click();

  await page.getByText('Re-read the party channels').click();
  await page.getByRole('checkbox', { name: '#limbo-trio' }).check();
  await page.getByRole('checkbox', { name: '#fa-night' }).check();
  await page.getByRole('button', { name: 'Re-read' }).click();
  await expect(page.locator('.rescan__status')).toHaveText(/Done: 2 channels read/, { timeout: 10_000 });

  await page.getByRole('link', { name: 'Open call c5d6e7f8' }).click();
  await expect(page).toHaveURL(`${ADMIN}/extractions/x-kalos`);
  await expect(page.getByRole('row', { name: /move/ })).toContainText('0.93');
  await page.getByRole('tab', { name: /Chat read/ }).click();
  await expect(page.getByText('kalos 10pm instead?')).toBeVisible();
  await page.getByRole('tab', { name: 'Prompt as sent' }).click();
  await expect(page.locator('pre')).toContainText('Messages:');
});

test('chat: interactions and one interaction in detail', async ({ page }) => {
  await go(page, '/chat');
  await expect(page.getByText('p50').first()).toBeVisible();
  await page.getByRole('link', { name: 'can you move bm to wed' }).click();
  await expect(page).toHaveURL(`${ADMIN}/chat/c-move`);
  await page.getByRole('tab', { name: /Tool trace/ }).click();
  await expect(page.getByRole('row', { name: /schedule.propose/ })).toBeVisible();
  await expect(page.getByRole('row', { name: /schedule.propose/ })).toContainText('63 ms');
  await page.getByRole('tab', { name: /Produced/ }).click();
  await expect(page.getByRole('link', { name: 'proposal card' })).toBeVisible();
});

test('chat filters: deep-linked, combinable, summarised, cleared', async ({ page }) => {
  await go(page, '/chat?outcome=timeout,error');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('2 of 13 interactions');
  await expect(page.getByRole('button', { name: /Outcome: timeout, error/ })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Filters (1)' })).toBeVisible();
  await expect(page.getByRole('row')).toHaveCount(3);

  // Add a model and a minimum latency through the panel; the URL follows.
  await page.getByRole('button', { name: 'Filters (1)' }).click();
  const panel = page.getByRole('group', { name: 'Filters' });
  await panel.getByLabel('Model').selectOption('kanata/chat');
  await expect(page).toHaveURL(/model=kanata%2Fchat/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 of 13 interactions');
  await panel.getByLabel('At least (ms)').fill('70000');
  await panel.getByLabel('At least (ms)').press('Tab');
  await expect(page.getByText('Nothing matches these filters.')).toBeVisible();
  await page.getByRole('button', { name: /≥ 70000 ms/ }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 of 13 interactions');

  // Tool used, a date preset (guild time) and text, then Clear.
  await page.getByRole('button', { name: 'Clear' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 interactions');
  // The panel stays open after Clear.
  await expect(page.getByRole('button', { name: 'Filters (0)' })).toHaveAttribute('aria-expanded', 'true');
  await page.getByRole('group', { name: 'Filters' }).getByLabel('Tool used').selectOption('schedule.read');
  // Includes the withheld turn: its tool name shows even though its traffic does not.
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('4 of 13 interactions');
  await page.getByRole('group', { name: 'Filters' }).getByRole('button', { name: 'This boss week' }).click();
  await expect(page).toHaveURL(/from=2026-09-24&to=2026-09-30/);
  await page.getByRole('searchbox', { name: 'Search interactions' }).fill('carling');
  await expect(page).toHaveURL(/q=carling/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 of 13 interactions');
  await page.getByRole('link', { name: /when is carling this week$/ }).click();
  await expect(page).toHaveURL(`${ADMIN}/chat/c-when`);

  // Nonsense is refused by the server, not silently ignored.
  const bad = await page.request.get(`${ADMIN}/api/admin/chat?outcome=nope`);
  expect(bad.status()).toBe(422);
  const badMs = await page.request.get(`${ADMIN}/api/admin/chat?min_ms=1e3`);
  expect(badMs.status()).toBe(422);
  expect(((await badMs.json()) as { error: string }).error).toBe('invalid_filter');

  // A refused filter shows its error without the previous filter's rows.
  await go(page, '/chat');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('13 interactions');
  await page.getByRole('button', { name: 'Filters (0)' }).click();
  await page.getByRole('group', { name: 'Filters' }).getByLabel('At least (ms)').fill('1e3');
  await page.getByRole('group', { name: 'Filters' }).getByLabel('At least (ms)').press('Tab');
  await expect(page.getByRole('alert').filter({ hasText: 'whole milliseconds' })).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Chat');
  await expect(page.getByRole('table')).toHaveCount(0);
});

test('extraction filters: outcome, model and member, deep-linked', async ({ page }) => {
  await go(page, '/extractions?outcome=proposed');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('3 of 34 model calls');
  await expect(page.getByRole('row', { name: /bm-trio/ })).toContainText('proposed');
  await page.getByRole('button', { name: 'Filters (1)' }).click();
  const panel = page.getByRole('group', { name: 'Filters' });
  await panel.getByRole('checkbox', { name: 'failed' }).check();
  await panel.getByRole('checkbox', { name: 'self-service link sent' }).check();
  await expect(page).toHaveURL(/outcome=proposed%2Cfailed%2Cself_service_link|outcome=proposed,failed,self_service_link/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('7 of 34 model calls');
  await panel.getByLabel('Member').selectOption({ label: 'Minato' });
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 of 34 model calls');
  await page.getByRole('button', { name: 'Clear' }).click();
  await panel.getByLabel('Model').selectOption('kanata/legacy');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('8 of 34 model calls');
  expect((await page.request.get(`${ADMIN}/api/admin/extractions?tool=x`)).status()).toBe(422);

  // A Chat link's tool/latency keys leave the URL rather than count as filters that do nothing.
  await go(page, '/extractions?outcome=proposed&tool=schedule.read&min_ms=5000');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('3 of 34 model calls');
  await expect(page).not.toHaveURL(/tool=|min_ms=/);
  await expect(page.getByRole('button', { name: 'Filters (1)' })).toBeVisible();
  await expect(page.getByRole('button', { name: /Tool:|ms — remove/ })).toHaveCount(0);
});

test('limits: backends, queue, admission by kind and an allowance reset', async ({ page }) => {
  await go(page, '/limits');
  const chat = page.getByRole('region', { name: /^chat · / });
  await expect(chat).toContainText('half-open — probing');
  await expect(page.getByRole('region', { name: /^rewrite · / })).toContainText('open — calls refused');
  await page.getByRole('tab', { name: /Queue/ }).click();
  await expect(page.getByRole('row', { name: /rescan/ })).toContainText('1');
  await page.getByRole('tab', { name: /Admission/ }).click();
  await expect(page.getByRole('row', { name: /key rate limit/ })).toContainText('key-level');
  await page.getByRole('tab', { name: /Allowances/ }).click();
  const reset = page.getByRole('button', { name: /^Reset .*'s window$/ }).first();
  await reset.click();
  await expect(toast(page, /window is reset/)).toBeVisible();
});

test('limits: a server without the route shows the page and says so, and stops asking', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  // Chrome logs every 4xx fetch as "Failed to load resource"; anything else is the app's.
  page.on('console', (m) => {
    if (m.type() === 'error' && !m.text().startsWith('Failed to load resource')) errors.push(m.text());
  });
  let asked = 0;
  // The Rust server's generic answer for an unmounted /api/admin path.
  await page.route(`${ADMIN}/api/admin/limits`, (route) => {
    asked += 1;
    return route.fulfill({ status: 404, json: { error: 'not_found', message: 'No such endpoint on this origin.' } });
  });
  await go(page, '/limits');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Limits');
  const window = page.getByRole('region', { name: 'Limits' });
  await expect(window.getByRole('status')).toContainText("This isn't available on this server yet.");
  await expect(window.getByRole('link', { name: 'Config → Models' })).toHaveAttribute('href', '/config?section=models');
  await expect(page.getByText('Loading the limits…')).toHaveCount(0);
  await expect(page.locator('[aria-busy="true"]')).toHaveCount(0);
  await page.waitForTimeout(5500);
  expect(asked).toBe(1);
  expect(errors).toEqual([]);
});

test('history: seeded timeline, strict revert, conflicts and force', async ({ page }) => {
  await go(page, '/history');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 changes');
  const latest = page.getByRole('listitem').filter({ hasText: '#9' }).first();
  await expect(latest).toContainText('reverts #8');
  await expect(latest).toContainText('rollback');
  // Records carry reminder rows; a move's re-placed cards fold into one line.
  const moved = page.getByRole('listitem').filter({ hasText: '#3' }).first();
  await expect(moved).toContainText('XKalos: 2 reminders re-placed');
  // `admin:discord:1001` reads as the staff member's name.
  await expect(moved.locator('strong').first()).toHaveText('Asahi');
  await page.getByLabel('Who').selectOption({ label: 'Asahi (as admin)' });
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 change');
  await page.getByLabel('Who').selectOption({ label: 'Admin (token)' });
  await expect(page.getByRole('listitem').filter({ hasText: '#9' }).first().locator('strong').first()).toHaveText('Admin (token)');
  await page.getByLabel('Who').selectOption({ label: 'everyone' });
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('9 changes');

  // Tsubame's answer (#2) was followed by an admin moving that run (#3): conflict.
  await page.locator('[data-history="2"]').click();
  await page.getByRole('complementary', { name: 'Change details' }).getByRole('button', { name: 'Revert…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Revert #2?' });
  await expect(dialog.getByRole('alert')).toContainText('Changed again since');
  await expect(dialog.getByRole('alert')).toContainText('#2: XKalos');
  // A strict refusal plans no rows; the dialog shows what forcing would do instead.
  await expect(dialog.getByRole('heading', { name: 'Would change', exact: true })).toHaveCount(0);
  await expect(dialog.getByRole('heading', { name: 'Forcing it would change' })).toBeVisible();
  await expect(dialog.getByText('XKalos: at risk → unconfirmed')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Force revert' })).toBeDisabled();
  await dialog.getByRole('checkbox', { name: /Force it/ }).check();
  await dialog.getByRole('button', { name: 'Force revert' }).click();
  await expect(toast(page, 'Reverted as #10.')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('10 changes');

  // #7 (Seren cancelled) has no later change: a strict revert applies.
  await page.locator('[data-history="7"]').click();
  await page.getByRole('complementary', { name: 'Change details' }).getByRole('button', { name: 'Revert…' }).click();
  const strict = page.getByRole('dialog', { name: 'Revert #7?' });
  await expect(strict.getByText('HSeren: cancelled → unconfirmed')).toBeVisible();
  await strict.getByRole('button', { name: 'Revert', exact: true }).click();
  await expect(toast(page, 'Reverted as #11.')).toBeVisible();

  await page.getByRole('tab', { name: 'Checkpoints' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Chain verified' })).toHaveText(/Chain verified: \d+ records, head #\d+\./);
  await expect(page.getByRole('tab', { name: 'Checkpoints' })).toHaveAccessibleName('Checkpoints 3');
  const backups = page.getByRole('table', { name: /Backups/ });
  await expect(backups.locator('tbody tr')).toHaveCount(3);
  // One row per anchor state, each named in words, not colour alone.
  await expect(backups.getByRole('row', { name: /matches — the history still holds this head/ })).toHaveCount(1);
  await expect(backups.getByRole('row', { name: /older schema — restore it with the image of that schema/ })).toHaveCount(1);
  await expect(backups.getByRole('row', { name: /mismatch — the history no longer holds this head/ })).toHaveCount(1);
  await expect(page.getByText(/checked just now/)).toBeVisible();

  // Verify again refetches the endpoint (read only) and announces the result.
  const refetch = page.waitForRequest(/\/api\/admin\/history\/checkpoints$/);
  const again = page.getByRole('button', { name: 'Verify again' });
  await again.click();
  await refetch;
  await expect(again).toBeEnabled();
  await expect(page.getByRole('status').filter({ hasText: 'Chain verified' })).toHaveText(/Chain verified: \d+ records/);
});

test('history: restore a week to a point, revert a member, and blame in the run sheet', async ({ page }) => {
  await go(page, '/history');
  await page.locator('[data-history="3"]').click();
  await page.getByRole('complementary', { name: 'Change details' }).getByRole('button', { name: 'Restore week to here…' }).click();
  const restore = page.getByRole('dialog', { name: /^Restore the week of/ });
  await expect(restore.getByText(/HCarling \+ HStar roster: −Ren \(2\)/)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(restore).toBeHidden();

  await page.getByText("Revert a member's changes…").click();
  await page.getByLabel('Member').selectOption({ label: 'Rin' });
  await page.getByRole('button', { name: 'Preview' }).click();
  const byMember = page.getByRole('dialog', { name: /^Revert everything Rin changed/ });
  await expect(byMember.getByText('HSeren: cancelled → unconfirmed')).toBeVisible();
  await byMember.getByRole('button', { name: 'Revert', exact: true }).click();
  await expect(toast(page, /Reverted as #\d+\./)).toBeVisible();

  await page.getByRole('link', { name: 'Week' }).click();
  await page.locator('[data-run="r-kalos"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'XKalos' });
  await sheet.getByText('Who changed this').click();
  await expect(sheet.getByRole('row', { name: /^Day and time/ })).toContainText('via extraction approval');
  await expect(sheet.getByRole('row', { name: /^Day and time/ })).toContainText('Fri 25 22:00');
  await expect(sheet.getByRole('row', { name: /^Day and time/ })).toContainText('Asahi');
  await expect(sheet.getByRole('row', { name: /Tsubame's answer/ })).toContainText('out');
});

test('config: pings, watching, chatbot, persona catalog, models, self-service, portal gate, notifications save', async ({ page }) => {
  await go(page, '/config');
  // Sections mount on first visit and stay mounted (unsaved edits survive
  // tab switches), so in-section selectors scope to the visible panel.
  const panel = page.locator('.settings__panel:not([hidden])');

  // Section deep-link.
  await expect(page.getByRole('tab', { name: 'Pings' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('tablist', { name: 'Settings sections' })).toHaveAttribute('aria-orientation', 'vertical');
  await page.getByRole('tab', { name: 'Weekly digest' }).click();
  await expect(page).toHaveURL(`${ADMIN}/config?section=digest`);
  await page.goto(`${ADMIN}/config?section=chatbot&sw=off`);
  await expect(page.getByRole('tab', { name: 'Chatbot' })).toHaveAttribute('aria-selected', 'true');

  // Pings: bad time refused inline, fields kept; good values toast and stay.
  await page.getByRole('tab', { name: 'Pings' }).click();
  const time = panel.getByRole('textbox', { name: 'Morning ping' });
  await time.fill('whenever');
  await panel.getByRole('button', { name: 'Save pings', exact: true }).click();
  await expect(panel.getByRole('alert')).toContainText('HH:MM');
  await expect(time).toHaveValue('whenever');
  await expect(time).toHaveAttribute('aria-invalid', 'true');
  await expect(panel.getByRole('textbox', { name: 'Countdowns (minutes)' })).toHaveAttribute('aria-invalid', 'false');
  await time.fill('08:30');
  await panel.getByRole('textbox', { name: 'Countdowns (minutes)' }).fill('45, 10');
  await panel.getByRole('button', { name: 'Save pings', exact: true }).click();
  // The server applies pings on restart; the toast says so rather than claiming a re-place.
  await expect(toast(page, 'Pings saved; they take effect when the bot restarts.')).toBeVisible();
  await expect(panel.getByRole('textbox', { name: 'Morning ping' })).toHaveValue('08:30');

  // Watching switches apply at once; turning one off asks first.
  await page.getByRole('tab', { name: 'Chat watching' }).click();
  const watching = panel.getByRole('switch', { name: /^Watching/ });
  await watching.click();
  await page.getByRole('dialog', { name: 'Pause watching?' }).getByRole('button', { name: 'Pause watching' }).click();
  await expect(toast(page, 'Watching paused.')).toBeVisible();
  await expect(watching).toHaveAttribute('aria-checked', 'false');
  await expect(panel.getByText('paused', { exact: true })).toBeVisible();
  await watching.click();
  await expect(toast(page, 'Watching resumed.')).toBeVisible();
  await expect(watching).toHaveAttribute('aria-checked', 'true');
  expect(((await (await page.request.get(`${ADMIN}/api/admin/config`)).json()) as { watching: { paused: boolean } }).watching.paused).toBe(false);

  // Chatbot rate save.
  await page.getByRole('tab', { name: 'Chatbot' }).click();
  await panel.getByRole('spinbutton', { name: 'Answers per person' }).fill('5');
  await expect(panel.getByText(/1 unsaved change · Per person answers/)).toBeVisible();
  await panel.getByRole('button', { name: 'Save chatbot', exact: true }).click();
  await expect(toast(page, /Answer limits saved/)).toBeVisible();

  // Persona catalog, profile visibility, reload, role order.
  await page.getByRole('tab', { name: /^Persona/ }).click();
  await expect(panel.getByText(/Effective:.*Kanade/)).toBeVisible();
  await panel.getByRole('combobox', { name: 'Active persona' }).selectOption('plain');
  await panel.getByRole('button', { name: 'Use this persona', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: /^Use (?!this)/ }).click();
  await expect(toast(page, /Plain/)).toBeVisible();
  await panel.getByRole('combobox', { name: 'Active persona' }).selectOption('kanade');
  await panel.getByRole('button', { name: 'Use this persona', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Use Kanade' }).click();
  await expect(panel.getByText(/Effective:.*Kanade/)).toBeVisible();
  // Reply profiles: label, voice as written, a one-line plain-text prompt preview, visibility.
  const profiles = panel.getByRole('table', { name: 'Reply profiles' });
  await expect(profiles.getByRole('columnheader')).toHaveText(['', 'Profile', 'Voice', 'Prompt', 'Visibility', 'Change visibility']);
  const kanade = profiles.getByRole('row', { name: /^Select Kanade\b/ });
  // Cells after the selection box: voice first.
  await expect(kanade.getByRole('cell').nth(1)).toHaveText('comedy.');
  await expect(kanade).toContainText('Kanade Teases lightly');
  await expect(kanade).not.toContainText('**');
  await expect(profiles).not.toContainText('config/personas/profiles/kanade');
  // Visibility is editable and profile text stays file-backed.
  const sparkly = profiles.getByRole('row', { name: /^Select Sparkly\b/ });
  await expect(sparkly).toContainText('private');
  await expect(sparkly.getByRole('button', { name: 'Publish' })).toBeVisible();
  // Role assignments use the current named guild directory; IDs are not picker input.
  await panel.getByRole('tab', { name: /^Role overrides/ }).click();
  const roleAssignments = panel.getByRole('list', { name: 'Role assignments in precedence order' });
  await expect(roleAssignments.getByRole('listitem')).toHaveCount(2);
  await expect(roleAssignments.getByRole('listitem').nth(0)).toContainText('@staff');
  await expect(panel.getByRole('combobox', { name: 'New assignment role' })).toBeEnabled();
  await expect(panel.getByText(/The first matching role.*overrides a member's saved reply style/)).toBeVisible();
  const published = await page.request.patch(`${ADMIN}/api/admin/config`, {
    headers: await csrf(page.request),
    data: { persona: { visibility: [{ key: 'sparkly', public: true }] } },
  });
  const publishedView = (await published.json()) as { persona: { profiles: { key: string; public: boolean }[] } };
  const restored = await page.request.patch(`${ADMIN}/api/admin/config`, {
    headers: await csrf(page.request),
    data: { persona: { visibility: [{ key: 'sparkly', public: false }] } },
  });
  const restoredView = (await restored.json()) as typeof publishedView;
  const publicState = (view: typeof publishedView) => view.persona.profiles.find((profile) => profile.key === 'sparkly')?.public;
  expect([published.status(), publicState(publishedView)]).toEqual([200, true]);
  expect([restored.status(), publicState(restoredView)]).toEqual([200, false]);
  // Reload re-reads the files.
  await panel.getByRole('tab', { name: 'Active persona & profiles' }).click();
  await panel.getByRole('button', { name: 'Reload profiles' }).click();
  await expect(toast(page, /Reloaded 4 reply profiles/)).toBeVisible();

  // Models: external and unknown routes state that raw member data leaves.
  await page.getByRole('tab', { name: 'Models' }).click();
  // The server's own startup check is shown; the saved seed passes it.
  await expect(page.getByRole('tab', { name: 'Models' }).locator('.settings__flag')).toHaveCount(0);
  await panel.getByRole('tab', { name: 'Capacity' }).click();
  await expect(panel.locator('.models__groups .tone--danger')).toHaveCount(0);
  await expect(panel.getByRole('row', { name: /gateway/ }).locator('.models__check .tone').first()).toBeVisible();
  await panel.getByRole('tab', { name: 'Roles' }).click();
  const models = panel.getByRole('combobox', { name: /^Model/ });
  const reasonings = panel.getByRole('combobox', { name: /^Reasoning/ });
  await models.first().selectOption('kanata/chat-cloud');
  await expect(panel.getByText(/go to an external provider/)).toBeVisible();
  await expect(panel.getByText(/raw member names, IDs, messages, and URLs leave the homelab/i)).toBeVisible();
  await expect(panel).not.toContainText(/pseudonym|provider testing|ALLOW_EXTERNAL_UNMASKED|PSEUDONYMIZE/i);
  await models.first().selectOption('kanata/extract');
  // An unknown trust zone is treated as external with the raw-data warning.
  await models.nth(2).selectOption('kanata/legacy');
  await expect(panel.getByText(/publishes no trust zone/)).toBeVisible();
  await models.nth(2).selectOption('kanata/rewrite-small');

  // Reasoning offers only the model's published efforts: the small rewrite
  // model publishes none, and extraction runs medium, so its picker is off only.
  await expect(reasonings.nth(2).locator('option')).toHaveText(['Off']);
  // `null` efforts: Kanata restricts nothing, so every level is offered.
  await models.nth(2).selectOption('kanata/legacy');
  await expect(reasonings.nth(2).locator('option')).toHaveText(['Same as extraction', 'Off', 'Minimal', 'Low', 'Medium', 'High', 'Xhigh', 'Max']);
  await expect(panel.getByText('reasoning: any level')).toBeVisible();
  await models.nth(2).selectOption('kanata/rewrite-small');
  // Inherit is offered only while the extraction effort fits the role model.
  await expect(reasonings.nth(1).locator('option').first()).toHaveText('Same as extraction');
  await reasonings.first().selectOption('high');
  await expect(reasonings.nth(1).locator('option').first()).toHaveText('Off');
  await models.first().selectOption('kanata/extract');
  await reasonings.first().selectOption('medium');
  // Chat needs tools: the tool-less model is offered disabled, with why.
  await expect(models.nth(1).locator('option[value="kanata/rewrite-small"]')).toHaveAttribute('disabled', '');
  await panel.getByRole('button', { name: 'Save models' }).click();
  await expect(toast(page, /Models saved; the next question uses them\./)).toBeVisible();

  // Capacity groups are read-only (kanade.toml): a table, no editor.
  await expect(panel.getByRole('button', { name: 'Save groups' })).toHaveCount(0);
  await expect(panel.getByRole('spinbutton', { name: /Permits/ })).toHaveCount(0);
  const groups = await page.request.patch(`${ADMIN}/api/admin/config`, {
    headers: await csrf(page.request),
    data: { models: { groups: [] } },
  });
  expect([groups.status(), ((await groups.json()) as { error: string }).error]).toEqual([422, 'read_only']);
  const badKey = await page.request.patch(`${ADMIN}/api/admin/config`, {
    headers: await csrf(page.request),
    data: { models: { kanata_limits: [] } },
  });
  await expect(badKey.status()).toBe(422);
  expect(await badKey.text()).toContain('Unknown or read-only');
  // An unwatched channel has nothing to re-read.
  const reread = await page.request.post(`${ADMIN}/api/admin/rescan`, { headers: await csrf(page.request), data: { channels: ['bm-trio'], window: 'week' } });
  expect(await reread.text()).toContain('not watched');

  // Notifications.
  await page.getByRole('tab', { name: 'Notifications' }).click();
  const quiet = panel.getByRole('switch', { name: /^Quiet mode/ });
  await quiet.click();
  await expect(toast(page, 'Quiet mode is on.')).toBeVisible();
  await expect(panel.getByText(/marked 🔕 in/)).toBeVisible();
  await quiet.click();
  await page.getByRole('dialog', { name: 'Turn quiet mode off?' }).getByRole('button', { name: 'Turn quiet mode off' }).click();
  await expect(quiet).toHaveAttribute('aria-checked', 'false');

  // Self-service: link-first saves, and closing the portal forces cards-only.
  await page.getByRole('tab', { name: 'Self-service' }).click();
  await expect(panel.getByText('How self-service works')).toBeVisible();
  await page.getByText('Link first', { exact: true }).click();
  await panel.getByRole('button', { name: 'Save self-service', exact: true }).click();
  await expect(toast(page, /link first/)).toBeVisible();
  // Closing applies at once (only opening asks).
  await panel.getByRole('switch', { name: /^Public portal/ }).click();
  await expect(toast(page, 'The public portal is closed.')).toBeVisible();
  await expect(panel.getByText(/serves only the app shell/)).toBeVisible();
  await expect(panel.getByText(/cards-only applies/)).toBeVisible();

  // Digest posts.
  await page.getByRole('tab', { name: 'Weekly digest' }).click();
  await panel.getByRole('combobox', { name: 'Channel' }).selectOption('fa-night');
  await panel.getByRole('button', { name: 'Post it now…' }).click();
  await page.getByRole('dialog', { name: /digest to #?fa-night/ }).getByRole('button', { name: 'Post it now' }).click();
  await expect(toast(page, /Posted this week's digest in #fa-night/)).toBeVisible();

  // Read-only table: values, reasons, and no inputs at all.
  await page.getByRole('tab', { name: 'Set in the environment' }).click();
  await expect(panel.getByRole('row', { name: /Timezone/ })).toContainText('Asia/Kuala_Lumpur');
  await expect(panel.getByRole('row', { name: /Timezone/ })).toContainText('a change needs a restart');
  await expect(panel.getByRole('row', { name: /Watched categories/ })).toContainText('KANADE_WATCH_CATEGORY_IDS');
  await expect(panel.getByRole('row', { name: /Watched categories/ })).toContainText('deliberate deploy');
  await expect(panel.getByRole('row', { name: /Model gateway/ })).not.toContainText('secret');
  await expect(panel.locator('input, select')).toHaveCount(0);
});

test('config: unreachable catalog and disconnected access render fallbacks', async ({ page }) => {
  await page.route(`${ADMIN}/api/admin/config`, async (route) => {
    const res = await route.fetch();
    const body = await res.json();
    body.models.reachable = false;
    await route.fulfill({ response: res, json: body });
  });
  await go(page, '/config?section=models');
  await expect(page.getByText(/model list is unreachable/)).toBeVisible();
  await expect(page.getByRole('combobox', { name: /^Model/ }).first()).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Save models' })).toBeDisabled();
  await page.unroute(`${ADMIN}/api/admin/config`);

  await page.route(`${ADMIN}/api/admin/access`, async (route) => {
    await route.fulfill({ json: { connected: false, checked_at: 'Tue 29 Sep 12:00', rows: [] } });
  });
  await go(page, '/config?section=access');
  await expect(page.getByText(/isn't connected to the guild/)).toBeVisible();
  await page.unroute(`${ADMIN}/api/admin/access`);
});

test('access: per-channel permissions and a recheck', async ({ page }) => {
  await go(page, '/config?section=access');
  const detail = page.locator('.settings__detail');
  await expect(detail.getByText(/^Checked /)).toBeVisible();
  const table = page.getByRole('table', { name: "The bot's permissions in each channel" });
  await expect(table.getByRole('row', { name: /#hstar-party/ })).toContainText('Post: granted');
  await expect(table.getByRole('row', { name: /#bm-trio/ })).toContainText('Post: missing');
  await expect(table.getByRole('row', { name: /#boss-schedule/ })).toContainText('digest');
  await expect(page.getByText(/1 channel.*will not get reminders/)).toBeVisible();
  await expect(page.getByText(/without it the reminders/)).toBeVisible();

  await page.getByRole('button', { name: 'Check again' }).click();
  await expect(toast(page, /Checked again at/)).toBeVisible();
  await expect(detail.getByText(/^Checked /)).toBeVisible();
});

test('week and sheet: per-channel re-read from the board and the sheet', async ({ page }) => {
  await go(page, '/');
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();

  // The board's own button (phones show it; wide clicks it directly too).
  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator('[data-run="r-carling"] .plan-card__reread').click();
  const progress = toast(page, /Re-reading #hstar-party/);
  await expect(progress).toBeVisible();
  // One re-read per channel: the button stays focusable but refuses a second.
  await expect(page.locator('[data-run="r-carling"] .plan-card__reread')).toHaveAttribute('aria-disabled', 'true');
  await expect(toast(page, /Re-read #hstar-party:.*nothing to change/)).toBeVisible({ timeout: 15_000 });
  // An unwatched channel is refused by name, and nothing keeps running.
  await page.locator('[data-run="r-bm"] .plan-card__reread').click();
  await expect(toast(page, /Couldn't re-read #bm-trio: #bm-trio is not watched/)).toBeVisible();
  await expect(page.locator('[data-run="r-bm"] .plan-card__reread')).toHaveAttribute('aria-disabled', 'false');

  // The sheet's channel button reports in the sheet itself.
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.getByRole('button', { name: 'Re-read #hstar-party from Discord and propose any changes' }).click();
  const notice = page.locator('.sheet__notice');
  await expect(notice).toContainText('Re-reading #hstar-party…');
  await expect(notice).toContainText(/nothing to change/, { timeout: 15_000 });
  await page.keyboard.press('Escape');
});

test('config on a phone: the section strip scrolls itself, never the frame', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await go(page, '/config?section=env');
  const tab = page.getByRole('tab', { name: 'Set in the environment' });
  await expect(tab).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('tablist', { name: 'Settings sections' })).toHaveAttribute('aria-orientation', 'horizontal');
  await expect(tab).toBeInViewport({ ratio: 1 });
  // The selected tab is marked by an underline as well as its fill.
  await expect(tab.locator('.row-content__compact .settings__label')).toBeVisible();
  await expect(tab.locator('.row-content__compact .settings__label')).toHaveCSS('text-decoration-line', 'underline');
  const scrolled = await page.evaluate(() => ({
    doc: document.scrollingElement!.scrollTop + document.scrollingElement!.scrollLeft,
    shell: document.querySelector('.shell')!.scrollLeft,
    strip: document.querySelector('.settings__toc')!.scrollLeft,
  }));
  expect(scrolled.doc).toBe(0);
  expect(scrolled.shell).toBe(0);
  expect(scrolled.strip).toBeGreaterThan(0);
  // The banner stays compact enough to leave the window most of the page.
  const window = (await page.locator('.settings').boundingBox())!;
  expect(window.height).toBeGreaterThan(300);
});

test('models: extraction to High resets an inheriting chat to Off, saved and resynced', async ({ page }) => {
  await go(page, '/config?section=models');
  const panel = page.locator('.settings__panel:not([hidden])');
  const reasonings = panel.getByRole('combobox', { name: /^Reasoning/ });
  // Seed: extraction medium, chat inherits.
  await expect(reasonings.nth(1)).toHaveValue('');
  await reasonings.first().selectOption('high');
  await expect(panel.getByRole('status').filter({ hasText: 'Chat reasoning reset to Off' })).toBeVisible();
  await expect(reasonings.nth(1)).toHaveValue('off');
  // A later change that resets nothing clears the note.
  await reasonings.first().selectOption('low');
  await expect(panel.getByText(/reasoning reset to Off/)).toHaveCount(0);
  await reasonings.first().selectOption('high');
  await expect(panel.getByText(/reasoning reset to Off/)).toHaveCount(0);
  await panel.getByRole('button', { name: 'Save models' }).click();
  await expect(toast(page, /Models saved; the next question uses them\./)).toBeVisible();

  type Cfg = { models: { roles: Record<string, { alias: string; reasoning: string }>; catalog: { id: string; reasoning_efforts: string[] | null }[] } };
  const cfg = (await (await page.request.get(`${ADMIN}/api/admin/config`)).json()) as Cfg;
  const { roles, catalog } = cfg.models;
  expect(roles.extraction!.reasoning).toBe('high');
  for (const role of ['chat', 'rewrite']) {
    const resolved = roles[role]!.reasoning === '' ? roles.extraction!.reasoning : roles[role]!.reasoning;
    const efforts = catalog.find((m) => m.id === roles[role]!.alias)!.reasoning_efforts;
    const legal = resolved === 'off' || (efforts === null ? ['low', 'medium', 'high'] : efforts).includes(resolved);
    expect(legal, `${role} resolves to ${resolved}`).toBe(true);
  }
  // The dropdown shows a real, selected option, never a blank box.
  const selected = await reasonings.nth(1).evaluate((s: HTMLSelectElement) => s.selectedOptions[0]?.textContent ?? '');
  expect(selected.trim()).toBe('Off');

  // The server refuses an explicit inherit that would resolve illegally.
  const refused = await page.request.patch(`${ADMIN}/api/admin/config`, { headers: await csrf(page.request), data: { models: { roles: { chat: { reasoning: '' } } } } });
  expect(refused.status()).toBe(422);
  expect(await refused.text()).toContain('inherits high');
});

test('persona: Reload profiles re-reads the config, so voice and summary follow the files', async ({ page }) => {
  await go(page, '/config?section=persona');
  const panel = page.locator('.settings__panel:not([hidden])');
  await expect(panel.getByRole('row', { name: /^Select Kanade\b/ }).getByRole('cell').nth(1)).toHaveText('comedy.');
  // After the reload the files say something new (served through the config read).
  let reloaded = false;
  await page.route(`${ADMIN}/api/admin/config`, async (route) => {
    if (!reloaded || route.request().method() !== 'GET') return route.continue();
    const res = await route.fetch();
    const body = await res.json();
    const kanade = body.persona.profiles.find((p: { key: string }) => p.key === 'kanade');
    kanade.voice = 'Freshly edited voice';
    kanade.prompt_summary = 'A summary written after the file changed.';
    await route.fulfill({ response: res, json: body });
  });
  await page.route(`${ADMIN}/api/admin/config/profiles/reload`, async (route) => {
    reloaded = true;
    await route.continue();
  });
  await panel.getByRole('button', { name: 'Reload profiles' }).click();
  await expect(toast(page, /Reloaded 4 reply profiles/)).toBeVisible();
  await expect(panel.getByText('Freshly edited voice', { exact: true })).toBeVisible();
  await expect(panel.getByText('A summary written after the file changed.')).toBeVisible();
});

test('models: capacity groups read-only, one row per group, Kanata limits in words, no variants', async ({ page }) => {
  await go(page, '/config?section=models');
  const panel = page.locator('.settings__panel:not([hidden])');
  await panel.getByRole('tab', { name: 'Capacity' }).click();
  // Default source: one gateway group over the role models.
  const groups = panel.getByRole('table', { name: /Every model shares one group of 1 permit \(models.permits in kanade.toml\)/ });
  await expect(groups.getByRole('row')).toHaveCount(2);
  await expect(groups.getByRole('columnheader')).toHaveText(['Group', 'Permits', 'Models', 'Startup check']);
  const gateway = groups.getByRole('row', { name: /gateway/ });
  await expect(gateway.locator('.capchip')).toHaveText(['kanata/extract', 'kanata/chat', 'kanata/rewrite-small']);
  await expect(panel.getByRole('button', { name: /Add a row|Save groups/ })).toHaveCount(0);
  // The server's verdict sits on its group's row, without repeating the group's name;
  // nothing spans groups, so no list under the table and no client-side warnings.
  await expect(gateway.locator('.models__check')).toContainText("1 permits, matching Kanata's limit.");
  await expect(gateway.locator('.models__check')).not.toContainText('Group gateway');
  await expect(gateway.locator('.models__check .tone--success')).toHaveCount(1);
  await expect(panel.getByRole('list', { name: 'Startup checks across groups' })).toHaveCount(0);
  await expect(panel).not.toContainText('size its limits for both');
  // Mixed Kanata limits: the models in use, a disclosure for the rest, never a `model:level` variant.
  const inUse = panel.getByRole('table', { name: 'What Kanata admits for the models in use' });
  await expect(inUse.getByRole('rowheader')).toHaveText(['kanata/extract', 'kanata/chat', 'kanata/rewrite-small']);
  await panel.getByText('Show all models').click();
  await expect(panel.getByRole('table', { name: 'What Kanata admits for every listed model' })).not.toContainText(':');
  await expect(panel.getByText("Kanata publishes no limit for this key; it is shared with the owner's other clients.")).toHaveCount(1);
  // The picker lists base models only; `off` is hidden where reasoning is required.
  await panel.getByRole('tab', { name: 'Roles' }).click();
  const models = panel.getByRole('combobox', { name: /^Model/ });
  await expect(models.first().locator('option[value*=":"]')).toHaveCount(0);
  await models.first().selectOption('kanata/think');
  await expect(panel.getByRole('combobox', { name: /^Reasoning/ }).first().locator('option')).toHaveText(['Low', 'High']);
});

test('models: config-declared groups, uniform limits, a stored variant, an unset role', async ({ page }) => {
  await page.route(`${ADMIN}/api/admin/config`, async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const res = await route.fetch();
    const body = await res.json();
    body.models.groups_source = 'config';
    body.models.groups = [
      { model: 'kanata/extract', group: 'local', permits: 1 },
      { model: 'kanata/chat', group: 'local', permits: 1 },
      { model: 'kanata/chat:high', group: 'local', permits: 1 },
    ];
    body.models.capacity_check = [
      { level: 'warning', message: 'The rewrite model kanata/rewrite-small is in no capacity group; its calls are refused.', group: null },
      { level: 'ok', message: "Group local: 1 permits, matching Kanata's limit.", group: 'local' },
    ];
    for (const limit of body.models.alias_limits) limit.max_in_flight = 3;
    body.models.roles.chat = { alias: 'kanata/chat:high', reasoning: '', variant_of: 'kanata/chat', fixed_effort: 'high' };
    body.models.roles.rewrite = { alias: '', reasoning: 'off' };
    await route.fulfill({ response: res, json: body });
  });
  await go(page, '/config?section=models');
  const panel = page.locator('.settings__panel:not([hidden])');
  await panel.getByRole('tab', { name: 'Capacity' }).click();
  const groups = panel.getByRole('table', { name: 'Set in kanade.toml under [[models.groups]]; restart to apply.' });
  const local = groups.getByRole('row', { name: /local/ });
  await expect(local.locator('.capchip')).toHaveText(['kanata/extract', 'kanata/chat']);
  await expect(local.locator('.models__check')).toContainText("1 permits, matching Kanata's limit.");
  // A role in no group is about no one row: it stays listed under the table.
  const across = panel.getByRole('list', { name: 'Startup checks across groups' });
  await expect(across.getByRole('listitem')).toHaveText([/kanata\/rewrite-small is in no capacity group/]);
  await expect(across.locator('.tone--warning')).toHaveCount(1);
  await expect(panel.getByText('Kanata admits 3 calls at a time per model.')).toBeVisible();
  await expect(panel.getByRole('table', { name: /models in use/ })).toHaveCount(0);
  await panel.getByRole('tab', { name: 'Roles' }).click();
  const models = panel.getByRole('combobox', { name: /^Model/ });
  // The stored variant shows as "<base> (fixed: <level>)"; no other variant is offered.
  await expect(models.nth(1).locator('option:checked')).toHaveText('kanata/chat (fixed: high)');
  await expect(models.nth(1).locator('option', { hasText: 'fixed' })).toHaveCount(1);
  await expect(models.nth(1).locator('option[value="kanata/chat:low"]')).toHaveCount(0);
  await expect(panel.getByRole('combobox', { name: /^Reasoning/ }).nth(1)).toBeDisabled();
  // An unset role reads "Not configured".
  await expect(models.nth(2).locator('option:checked')).toHaveText('Not configured');
});

test('persona: the default voice leads the profiles, fixed and never selectable', async ({ page }) => {
  await go(page, '/config?section=persona');
  const panel = page.locator('.settings__panel:not([hidden])');
  const profiles = panel.getByRole('table', { name: 'Reply profiles' });
  const rows = profiles.locator('tbody tr');
  const first = rows.first();
  await expect(first.getByRole('rowheader')).toHaveText('Default voice');
  await expect(first).toContainText('Kanade as written');
  await expect(first).toContainText("(the persona's own voice)");
  await expect(first).toContainText('public');
  await expect(first.locator('.cap')).toHaveText('default');
  // Synthesized from the active persona: no selection box, no visibility action, no prompt to open.
  await expect(first.getByRole('checkbox')).toHaveCount(0);
  await expect(first.getByRole('button')).toHaveCount(0);
  // Selecting the page selects real profiles only.
  const real = (await page.request.get(`${ADMIN}/api/admin/config`).then((r) => r.json())) as { persona: { profiles: unknown[] } };
  await panel.getByRole('checkbox', { name: 'Select all reply profiles on this page' }).check();
  const n = Math.min(real.persona.profiles.length, 10);
  await expect(panel.getByText(`${n} profile${n === 1 ? '' : 's'} selected.`)).toBeVisible();
  await panel.getByRole('button', { name: 'Clear' }).click();
  // It reads as public: the Private filter hides it, a search for it keeps it alone.
  await panel.getByRole('button', { name: /^Private/ }).click();
  await expect(profiles.getByRole('rowheader', { name: /^Default voice/ })).toHaveCount(0);
  await panel.getByRole('button', { name: /^All/ }).click();
  await panel.getByRole('searchbox', { name: 'Search' }).fill('default voice');
  await expect(rows).toHaveCount(1);
  await expect(first.getByRole('rowheader')).toHaveText('Default voice');
});

test('digest: the last posted card follows the API, and is omitted without one', async ({ page }) => {
  type Last = { posted_at: string; this_week: boolean; channel_id: string; channel_name: string | null; url: string | null };
  const view = (await page.request.get(`${ADMIN}/api/admin/config`).then((r) => r.json())) as { last_digest: Last | null };
  const last = view.last_digest!;
  expect(last).toBeTruthy();
  await go(page, '/config?section=digest');
  const card = page.getByRole('region', { name: 'Last posted' });
  // Guild time straight from the offset-carrying instant: "Thu 24 Sep 00:15".
  await expect(card.locator('b')).toHaveText(new RegExp(`^(Mon|Tue|Wed|Thu|Fri|Sat|Sun) \\d{1,2} [A-Z][a-z]{2} ${last.posted_at.slice(11, 16)}$`));
  await expect(card).toContainText(last.this_week ? '· this week ·' : '· week of ');
  await expect(card).toContainText(last.channel_name ?? last.channel_id);
  const link = card.getByRole('link', { name: /open in Discord/ });
  await expect(link).toHaveAttribute('href', last.url!);
  await expect(link).toHaveAttribute('target', '_blank');
  await expect(link).toHaveAttribute('rel', /noopener/);

  // An unknown channel name falls back to its id; no link without a URL; an older week says which.
  await page.route(`${ADMIN}/api/admin/config`, async (route) => {
    const res = await route.fetch();
    const body = await res.json();
    body.last_digest = { ...body.last_digest, channel_name: null, url: null, this_week: false };
    await route.fulfill({ response: res, json: body });
  });
  await go(page, '/config?section=digest');
  await expect(card).toContainText(last.channel_id);
  await expect(card).toContainText('· week of ');
  await expect(card.getByRole('link')).toHaveCount(0);
  await page.unroute(`${ADMIN}/api/admin/config`);

  await page.route(`${ADMIN}/api/admin/config`, async (route) => {
    const res = await route.fetch();
    const body = await res.json();
    body.last_digest = null;
    await route.fulfill({ response: res, json: body });
  });
  await go(page, '/config?section=digest');
  await expect(page.getByRole('button', { name: 'Post it now…' })).toBeVisible();
  await expect(card).toHaveCount(0);
  await page.unroute(`${ADMIN}/api/admin/config`);
});

test('env: copy buttons write the raw env value; none where it is unset', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  await go(page, '/config?section=env');
  const panel = page.locator('.settings__panel:not([hidden])');
  // The row shows the human value; the copy is the env form.
  await expect(panel.getByRole('row', { name: /Boss week starts/ })).toContainText('Thu 00:00');
  await panel.getByRole('button', { name: 'Copy KANADE_BOSS_WEEK_RESET_WEEKDAY' }).click();
  await expect(toast(page, 'Copied KANADE_BOSS_WEEK_RESET_WEEKDAY.')).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('thu');
  await panel.getByRole('button', { name: 'Copy KANADE_WATCH_CHANNEL_IDS' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toMatch(/^[^,\s]+(,[^,\s]+)+$/);
  // Unset values have nothing to copy.
  await expect(panel.getByRole('row', { name: /Watched categories/ }).getByRole('button')).toHaveCount(0);
  await expect(panel.getByRole('button', { name: 'Copy KANADE_WATCH_CATEGORY_IDS' })).toHaveCount(0);

  // Without a clipboard the toast says the value instead.
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: () => Promise.reject(new Error('denied')) }, configurable: true });
  });
  await panel.getByRole('button', { name: 'Copy KANADE_TIMEZONE' }).click();
  await expect(toast(page, "Couldn't copy here; KANADE_TIMEZONE is Asia/Kuala_Lumpur")).toBeVisible();
});
