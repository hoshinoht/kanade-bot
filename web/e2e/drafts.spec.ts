import type { Page } from '@playwright/test';
import { ADMIN, choose, csrf, expect, test } from './support';

// Drafts (boards docs/research/2026-10-11-drafts-mockups) over the mock's seed:
// "Weekend reshuffle" (up to date, 2 staged on XKalos), "Carling retime" (base 3
// behind: a both-changed slot on HCarling + HStar and NLimbo cancelled since),
// and closed "Tuesday Kalos" (merged) and "Scratch" (discarded). Inbox request
// editing uses `p-kalos-fixed` (Tsubame: XKalos Fri 21:30 → Sat 22:30, + Yuzu).

const toast = (page: Page, text: string | RegExp) => page.locator('.toast:not(.is-leaving)').filter({ hasText: text });

async function drafts(page: Page, query = '') {
  await page.goto(`${ADMIN}/drafts${query}${query ? '&' : '?'}sw=off`);
  await expect(page.getByRole('listbox', { name: /drafts$/ })).toBeVisible();
}

async function showWeekEnd(page: Page) {
  await page.locator('.board').evaluate((board) => (board.scrollLeft = board.scrollWidth));
  await expect(page.locator('.board.scroll-more-end')).toHaveCount(0);
}

async function openCarlingStagePicker(page: Page) {
  await page.goto(`${ADMIN}/?sw=off`);
  await expect(page.locator('[data-run="r-carling"]')).toBeVisible();
  await showWeekEnd(page);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const pane = page.getByRole('complementary', { name: 'HCarling + HStar' });
  await pane.getByRole('button', { name: 'Stage in draft…' }).click();
  const picker = pane.getByRole('dialog', { name: 'Stage in draft' });
  await expect(picker.getByRole('radio', { name: /Weekend reshuffle/ })).toBeChecked();
  return { pane, picker };
}

async function stageWeekendSwap(page: Page) {
  const { pane, picker } = await openCarlingStagePicker(page);
  await picker.getByRole('button', { name: 'Start staging' }).click();
  await expect(pane.getByText('Staging into Weekend reshuffle')).toBeVisible();
  await pane.getByRole('button', { name: 'Stage swap timing with…' }).click();
  const swap = pane.getByRole('group', { name: /Swap HCarling \+ HStar's timing/ });
  await choose(pane.getByRole('combobox', { name: 'Swap with' }), { label: 'Tue 29 23:30 · XBM' });
  return { pane, swap };
}

async function refuseSecondStagedMove(page: Page, refuseRemoval = false) {
  const posts: { version: number; op: { run: string } }[] = [];
  const removals: string[] = [];
  await page.route(/\/api\/admin\/drafts\/d-weekend\/ops(?:\/\d+)?(?:\?.*)?$/, async (route) => {
    const request = route.request();
    if (request.method() === 'POST') {
      posts.push(request.postDataJSON() as { version: number; op: { run: string } });
      if (posts.length === 2) {
        await route.fulfill({
          status: 409,
          json: { error: 'stale', message: 'The draft changed or was closed since you loaded it; reload it.' },
        });
        return;
      }
    }
    if (request.method() === 'DELETE') {
      const url = new URL(request.url());
      removals.push(`${url.pathname}${url.search}`);
      if (refuseRemoval) {
        await route.fulfill({
          status: 409,
          json: { error: 'stale', message: 'The draft changed or was closed since you loaded it; reload it.' },
        });
        return;
      }
    }
    await route.continue();
  });
  return { posts, removals };
}
test('drafts: the list, a draft and its preview, and the closed tab', async ({ page }) => {
  await drafts(page);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('2 open');
  const list = page.getByRole('listbox', { name: 'Open drafts' });
  await expect(list.getByRole('option')).toHaveCount(2);
  await expect(list.getByRole('option').first()).toContainText('Weekend reshuffle');
  await expect(list.getByRole('option').first()).toContainText('Up to date');
  await expect(list.getByRole('option').nth(1)).toContainText('Base moved');
  // The first draft opens by default: not live, its staged changes, the preview and Merge.
  await expect(page.getByRole('heading', { level: 2, name: 'Weekend reshuffle' })).toBeVisible();
  await expect(page.getByText('No pings, cards or reminders until you merge.')).toBeVisible();
  await expect(page.getByRole('list', { name: 'Staged changes' }).getByRole('listitem')).toHaveCount(2);
  const pane = page.getByRole('complementary', { name: 'Preview and merge' });
  await expect(pane).toContainText('If merged now');
  await expect(pane.locator('.history-diff').first()).toContainText('changes 1, 2');
  await expect(pane.getByRole('button', { name: /^Merge 2 changes/ })).toHaveAttribute('aria-disabled', 'false');
  // Arrows move the selection (wide: it follows).
  await list.focus();
  await page.keyboard.press('ArrowDown');
  await expect(page).toHaveURL(/item=d-carling/);
  await expect(page.getByRole('heading', { level: 2, name: 'Carling retime' })).toBeVisible();

  await page.getByRole('tab', { name: 'Closed' }).click();
  const closed = page.getByRole('listbox', { name: 'Closed drafts' });
  await expect(closed.getByRole('option')).toHaveCount(2);
  await expect(closed.getByRole('option').first()).toContainText('merged as #1');
  await expect(page.getByText('Closed: merged as #1')).toBeVisible();
  // A closed draft takes no changes.
  await expect(page.getByRole('button', { name: /^Remove change/ })).toHaveCount(0);
});

test('drafts: conflicts → rebase (take one, keep one) → merge', async ({ page }) => {
  await drafts(page, '?item=d-carling');
  await expect(page.getByText('The schedule moved since you started.')).toBeVisible();
  const pane = page.getByRole('complementary', { name: 'Conflicts and merge' });
  await expect(pane.locator('.draftcfl')).toHaveCount(2);
  await expect(pane.getByText('Both changed')).toBeVisible();
  // The conflicting changes are flagged in place.
  await expect(page.getByRole('list', { name: 'Staged changes' }).locator('.draftop--bad')).toHaveCount(3);
  // Merge is off with a reason; Rebase waits for every conflict to be settled.
  await expect(pane.getByRole('button', { name: /^Merge 3 changes/ })).toHaveAttribute('aria-disabled', 'true');
  await expect(pane.getByText('Can’t merge yet:')).toBeVisible();
  const rebase = pane.getByRole('button', { name: /^Rebase onto #/ });
  await expect(rebase).toHaveAttribute('aria-disabled', 'true');
  await pane.getByRole('button', { name: /^Take Ren’s/ }).click();
  await expect(rebase).toHaveAttribute('aria-disabled', 'true');
  await pane.getByRole('button', { name: 'Revive the run' }).click();
  await expect(pane.getByText('2 conflicts · both settled')).toBeVisible();
  await rebase.click();
  await expect(toast(page, 'Draft rebased; your changes are kept.')).toBeVisible();
  // On the head and conflict-free: the preview pane, and Merge.
  const preview = page.getByRole('complementary', { name: 'Preview and merge' });
  await expect(preview).toBeVisible();
  // Take dropped the slot change; keep revived NLimbo ahead of its two changes.
  const ops = page.getByRole('list', { name: 'Staged changes' }).getByRole('listitem');
  await expect(ops).toHaveCount(3);
  await expect(ops.first()).toContainText('Revive r-limbo');
  await expect(page.getByRole('listbox', { name: 'Open drafts' }).getByRole('option', { name: /Carling retime/ })).toContainText('Up to date');
  await preview.getByRole('button', { name: /^Merge 3 changes/ }).click();
  await expect(toast(page, /^Draft merged as change #\d+\./)).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 open');
  await page.getByRole('tab', { name: 'Closed' }).click();
  await expect(page.getByRole('listbox', { name: 'Closed drafts' }).getByRole('option', { name: /Carling retime/ })).toContainText('merged as #');
});

test('drafts: remove a change, rename, discard, new draft', async ({ page }) => {
  await drafts(page, '?item=d-weekend');
  await page.getByRole('button', { name: /^Remove change 2:/ }).click();
  await expect(toast(page, 'Removed change 2 from Weekend reshuffle.')).toBeVisible();
  await expect(page.getByRole('list', { name: 'Staged changes' }).getByRole('listitem')).toHaveCount(1);
  // Focus stays in the list.
  await expect(page.getByRole('button', { name: /^Remove change 1:/ })).toBeFocused();

  await page.getByRole('button', { name: 'Rename Weekend reshuffle' }).click();
  const field = page.getByRole('textbox', { name: 'Draft name' });
  await expect(field).toBeFocused();
  await field.fill('Wednesday shuffle');
  await field.press('Enter');
  await expect(page.getByRole('heading', { level: 2, name: 'Wednesday shuffle' })).toBeVisible();
  await expect(page.getByRole('listbox', { name: 'Open drafts' }).getByRole('option').first()).toContainText('Wednesday shuffle');
  await expect(page.getByRole('button', { name: 'Rename Wednesday shuffle' })).toBeFocused();

  await page.getByRole('button', { name: 'Discard…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Discard Wednesday shuffle?' });
  await expect(dialog).toContainText('Nothing was live');
  await dialog.getByRole('button', { name: 'Discard draft' }).click();
  await expect(toast(page, 'Draft discarded; nothing was applied.')).toBeVisible();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('1 open');

  await page.getByRole('button', { name: 'New draft' }).first().click();
  const named = page.getByRole('dialog', { name: 'New draft' });
  await named.getByRole('textbox', { name: 'Name' }).fill('Bellona clean-up');
  await named.getByRole('button', { name: 'Start draft' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'Bellona clean-up' })).toBeVisible();
  await expect(page.getByText('Nothing staged yet.')).toBeVisible();
});

test('drafts: an empty Open tab shows the note and the recently closed', async ({ page }) => {
  await page.route('**/api/admin/drafts?state=open', (route) => route.fulfill({ json: { head: 9, drafts: [] } }));
  await page.goto(`${ADMIN}/drafts?sw=off`);
  await expect(page.getByRole('heading', { name: 'No open drafts' })).toBeVisible();
  await expect(page.getByRole('link', { name: /Tuesday Kalos/ })).toContainText('merged as #1');
});

test('drafts: a failed read says so and retries', async ({ page }) => {
  let fail = true;
  await page.route('**/api/admin/drafts?state=open', (route) =>
    fail ? route.fulfill({ status: 503, json: { error: 'unavailable', message: "The server didn't answer in time." } }) : route.fallback(),
  );
  await page.goto(`${ADMIN}/drafts?sw=off`);
  await expect(page.getByRole('heading', { name: 'Couldn’t load drafts' })).toBeVisible();
  fail = false;
  await page.getByRole('button', { name: 'Try again' }).click();
  await expect(page.getByRole('listbox', { name: 'Open drafts' })).toBeVisible();
});

test('drafts: staging from the run sheet persists across sheets until Stop staging', async ({ page }) => {
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  let pane = page.getByRole('complementary', { name: 'HCarling + HStar' });
  await pane.getByRole('button', { name: 'Stage in draft…' }).click();
  const picker = pane.getByRole('dialog', { name: 'Stage in draft' });
  // This boss week's open drafts, then New draft with a name.
  await expect(picker.getByRole('radio', { name: /Weekend reshuffle/ })).toBeChecked();
  await picker.getByRole('textbox', { name: 'New draft name' }).fill('Wednesday shuffle');
  await expect(picker.getByRole('radio', { name: /New draft/ })).toBeChecked();
  await picker.getByRole('button', { name: 'Start staging' }).click();
  const strip = pane.getByRole('status').filter({ hasText: 'Staging into Wednesday shuffle' });
  await expect(strip).toContainText('edits on this run go into the draft, not live');
  // Status is staged, not live: the run keeps its status on the board.
  await expect(pane.getByRole('group', { name: 'Stage status' })).toBeVisible();
  await pane.getByRole('button', { name: 'Confirmed' }).click();
  await expect(pane.getByText(/^Staged HCarling \+ HStar as confirmed in Wednesday shuffle/)).toBeVisible();
  await expect(pane.getByText(/In draft: 1 change to this run/)).toBeVisible();
  const live = await page.request.get(`${ADMIN}/api/admin/week`);
  expect((await live.json()).runs.find((r: { id: string }) => r.id === 'r-carling').status).toBe('planned');

  // Closing the sheet keeps staging: another run's sheet shows the strip too.
  await pane.getByRole('button', { name: 'Close HCarling + HStar' }).click();
  await page.locator('[data-run="r-bm"] .plan-card__open').click();
  pane = page.getByRole('complementary', { name: 'XBM' });
  await expect(pane.getByText('Staging into Wednesday shuffle')).toBeVisible();
  await expect(pane.getByText('Stage a move to', { exact: true })).toBeVisible();
  await expect(pane.getByRole('button', { name: 'Stage move', exact: true })).toBeVisible();

  // Open draft shows it with the staged change.
  await pane.getByRole('button', { name: 'Open draft' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'Wednesday shuffle' })).toBeVisible();
  await expect(page.getByRole('list', { name: 'Staged changes' }).getByRole('listitem')).toHaveCount(1);

  // Back on the Week (in the app: staging is this tab's state), Stop staging: live edits again.
  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Week' }).click();
  await page.locator('[data-run="r-bm"] .plan-card__open').click();
  pane = page.getByRole('complementary', { name: 'XBM' });
  await pane.getByRole('button', { name: 'Stop staging' }).click();
  await expect(pane.getByText('Staging into')).toHaveCount(0);
  await expect(pane.getByRole('group', { name: 'Status' })).toBeVisible();
});

test('drafts: staging refuses a draft discarded after the run-sheet picker read it', async ({ page }) => {
  const { pane, picker } = await openCarlingStagePicker(page);
  const discarded = await page.request.delete(`${ADMIN}/api/admin/drafts/d-weekend?version=3`, { headers: await csrf(page.request) });
  expect(discarded.status()).toBe(200);

  await picker.getByRole('button', { name: 'Start staging' }).click();
  await expect(picker.getByRole('alert')).toHaveText('Weekend reshuffle is closed; it can take no more changes.');
  await expect(pane.getByRole('status').filter({ hasText: 'Staging into' })).toHaveCount(0);
  const detail = await page.request.get(`${ADMIN}/api/admin/drafts/d-weekend`);
  expect((await detail.json()).status).toBe('discarded');
});

test('drafts: a refused second staged swap op rolls the first op back', async ({ page }) => {
  const intercepted = await refuseSecondStagedMove(page);
  const { pane, swap } = await stageWeekendSwap(page);
  await swap.getByRole('button', { name: 'Swap', exact: true }).click();

  const failure = toast(page, /^Couldn't stage a swap/);
  await expect(failure).toBeVisible();
  expect(intercepted.posts.map(({ version, op }) => [version, op.run])).toEqual([[3, 'r-carling'], [4, 'r-bm']]);
  expect(intercepted.removals).toEqual(['/api/admin/drafts/d-weekend/ops/2?version=4']);
  const response = await page.request.get(`${ADMIN}/api/admin/drafts/d-weekend`);
  const detail = (await response.json()) as { ops: { run_id: string | null }[] };
  expect(detail.ops).toHaveLength(2);
  expect(detail.ops.some(({ run_id }) => run_id === 'r-carling' || run_id === 'r-bm')).toBe(false);
  await expect(pane.getByRole('alert')).toContainText(/^Couldn't stage a swap/);
});

test('drafts: a refused swap rollback names the change left in the draft', async ({ page }) => {
  const intercepted = await refuseSecondStagedMove(page, true);
  const { pane, swap } = await stageWeekendSwap(page);
  await swap.getByRole('button', { name: 'Swap', exact: true }).click();

  const failure = toast(page, /^Couldn't stage a swap/);
  await expect(failure).toBeVisible();
  await expect(failure).toContainText('Change 3 stayed in Weekend reshuffle; remove it there.');
  expect(intercepted.posts.map(({ version, op }) => [version, op.run])).toEqual([[3, 'r-carling'], [4, 'r-bm']]);
  expect(intercepted.removals).toEqual(['/api/admin/drafts/d-weekend/ops/2?version=4']);
  const response = await page.request.get(`${ADMIN}/api/admin/drafts/d-weekend`);
  const detail = (await response.json()) as { ops: { ord: number; run_id: string | null }[] };
  expect(detail.ops).toHaveLength(3);
  expect(detail.ops[2]).toMatchObject({ ord: 2, run_id: 'r-carling' });
  await expect(pane.getByRole('alert')).toContainText(/^Couldn't stage a swap/);
});


test('drafts: the phone stages from a bottom sheet', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.locator('[data-run="r-carling"] .plan-card__open').click();
  const sheet = page.getByRole('dialog', { name: 'HCarling + HStar' });
  await sheet.getByRole('button', { name: 'Stage in draft…' }).click();
  const picker = page.getByRole('dialog', { name: 'Stage in draft' });
  await picker.getByRole('button', { name: 'Start staging' }).click();
  await expect(picker).toBeHidden();
  await expect(sheet.getByText('Staging into Weekend reshuffle')).toBeVisible();
  await expect(sheet.getByRole('button', { name: /^Stage a move of/ })).toBeFocused();
});

async function kalosRequest(page: Page) {
  await page.goto(`${ADMIN}/inbox?tab=self_service&item=p-kalos-fixed&sw=off`);
  await expect(page.getByRole('heading', { level: 2, name: /Weekly timing change/ })).toBeVisible();
}

test('inbox: edit a request before approving (day/time, drop) and approve edited', async ({ page }) => {
  await kalosRequest(page);
  await page.getByRole('button', { name: 'Edit before approving' }).click();
  await expect(page.getByText('Editing · not saved')).toBeVisible();
  const day = page.getByRole('radiogroup', { name: 'Day for change 1' });
  // Thursday first (the boss week's order); the member's day is marked.
  await expect(day.getByRole('radio').first()).toHaveAccessibleName('Thursday');
  await expect(day.getByRole('radio', { name: 'Saturday (Tsubame asked)' })).toBeFocused();
  await page.getByRole('button', { name: /minutes earlier$/ }).click();
  await expect(page.getByText('Your edit:')).toContainText('Sat 22:00');
  await page.getByRole('button', { name: 'Drop change 2: the party change' }).click();
  await expect(page.getByText('Dropped')).toBeVisible();
  await expect(page.getByText(/XKalos weekly moves to Sat 22:00\. The party change was not approved\./)).toBeVisible();
  // The last change in cannot be dropped: that would be a rejection.
  await expect(page.getByRole('button', { name: 'Drop change 1: the day and time change' })).toBeDisabled();
  await expect(page.getByRole('option', { name: /Weekly timing change — XKalos/ })).toContainText('editing');
  await page.getByRole('radio', { name: 'Keep as it is' }).check();
  await page.getByRole('button', { name: 'Approve edited' }).click();
  await expect(toast(page, /^Approved with changes/)).toBeVisible();
  const fixed = await (await page.request.get(`${ADMIN}/api/admin/fixed`)).json();
  const kalos = (Array.isArray(fixed) ? fixed : fixed.rows).find((f: { id: string }) => f.id === 'f-kalos');
  expect(JSON.stringify(kalos)).toContain('22:00');
});

test('inbox: an edited approval refused after the member changed it reloads read-only with Edit again', async ({ page }) => {
  await kalosRequest(page);
  await page.getByRole('button', { name: 'Edit before approving' }).click();
  await page.getByRole('button', { name: 'Drop change 2: the party change' }).click();
  expect((await page.request.post(`${ADMIN}/__mock/inbox/kalos-v4`)).status()).toBe(204);
  await page.getByRole('radio', { name: 'Keep as it is' }).check();
  await page.getByRole('button', { name: 'Approve edited' }).click();
  const refused = toast(page, 'Couldn’t approve your edit.');
  await expect(refused).toContainText('Tsubame changed this request (now version 4), so your edits were cleared.');
  // Persistent (an error), the editor closed, the request at version 4 and flagged.
  await expect(page.getByText('Editing · not saved')).toHaveCount(0);
  await expect(page.getByText('changed in version 4')).toBeVisible();
  await expect(page.getByRole('option', { name: /Weekly timing change — XKalos/ })).toContainText('updated · version 4');
  await expect(page.getByRole('button', { name: 'Approve', exact: true })).toBeVisible();
  await refused.getByRole('button', { name: 'Edit again' }).click();
  await expect(page.getByText('Editing · not saved')).toBeVisible();
  await expect(page.getByRole('radiogroup', { name: 'Day for change 1' }).getByRole('radio', { name: 'Sunday (Tsubame asked)' })).toBeChecked();
  await page.getByRole('radio', { name: 'Keep as it is' }).check();
  await page.getByRole('button', { name: 'Approve edited' }).click();
  await expect(toast(page, /^Approved/)).toBeVisible();
});
