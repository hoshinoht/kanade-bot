import AxeBuilder from '@axe-core/playwright';
import type { Locator, Page } from '@playwright/test';
import { PUBLIC, choose, expect, settle, signInPublic, test } from './support';

// The member's requests to the admins (member-writes contract Phase B,
// boards RequestForm, RequestForm-Confirm, RequestForm-Limit, Requests,
// Requests-Expired, PhoneRequest, PhoneRequests) and the Move page's axe
// pass, against the mock's member (Asahi): one waiting weekly change on
// XKalos, then approved, declined, withdrawn and expired ones; 3 may wait
// at once. Every test ends with zero CSP/TT reports.

test.describe.configure({ mode: 'parallel' });

const toast = (page: Page) => page.getByRole('group', { name: 'Notification' });
const sendKey = (page: Page) => page.getByRole('button', { name: /^(Send request|Sending…)$/ });
const note = (page: Page) => page.getByRole('textbox', { name: 'Note for the admins · optional' });
const subject = (page: Page, text: string) => page.locator('label.req-run', { hasText: text });

async function form(page: Page, query = ''): Promise<void> {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/requests/new?${query}${query ? '&' : ''}sw=off`);
  await expect(page.getByRole('radiogroup', { name: 'Request type' })).toBeVisible();
  await expect(page.locator('.req-count')).toHaveText('1 of 3 open · 1 of 6 today');
}

/** Sends a request straight to the API, as another tab would. */
async function sendElsewhere(page: Page, body: Record<string, unknown>, key: string): Promise<number> {
  const session = await page.request.get(`${PUBLIC}/api/public/session`);
  const sent = await page.request.post(`${PUBLIC}/api/public/requests`, { headers: { 'X-Kanade-CSRF': session.headers()['x-kanade-csrf'] ?? '', 'Idempotency-Key': key }, data: body });
  return sent.status();
}

async function sent(page: Page, title: string): Promise<void> {
  await expect(toast(page).filter({ hasText: `Sent: ${title}. The admins review it; Discord tells you when one decides.` })).toBeVisible();
  await expect(page).toHaveURL(/\/requests\?open=req-/);
  const detail = page.locator('.req-detail');
  await expect(detail.getByRole('heading', { level: 2 })).toContainText(title);
  await expect(detail.locator('.req-chip')).toHaveText('waiting');
  await expect(page.locator('.req-count')).toHaveText('2 of 3 open · 2 of 6 today');
}

const KINDS: { kind: string; title: string; fill: (page: Page) => Promise<void> }[] = [
  {
    kind: 'Join a run',
    title: 'Join HLimbo',
    fill: async (page) => {
      // Join lists only runs Asahi is not in.
      await expect(subject(page, 'HCarling + HStar')).toHaveCount(0);
      await subject(page, 'HLimbo').click();
    },
  },
  {
    kind: 'Leave a run',
    title: 'Leave HCarling + HStar',
    fill: async (page) => {
      await subject(page, 'HCarling + HStar').first().click();
      await note(page).fill('Something came up on Tuesday evening.');
    },
  },
  {
    kind: 'Swap my place',
    title: 'Swap my place on HCarling + HStar',
    fill: async (page) => {
      await subject(page, 'HCarling + HStar').first().click();
      await choose(page.getByRole('combobox', { name: 'Who takes your place' }), { label: 'Kaito' });
      await expect(page.locator('.req-aside__line')).toContainText('Kaito takes your place');
    },
  },
  {
    kind: 'New weekly run',
    title: 'New weekly HLimbo',
    fill: async (page) => {
      await page.getByRole('textbox', { name: 'Bosses' }).fill('HLimbo');
      await page.getByRole('radiogroup', { name: 'Day' }).getByRole('radio', { name: 'Sat' }).click();
      await page.getByRole('spinbutton', { name: 'Time' }).fill('21:00');
      await choose(page.getByRole('combobox', { name: 'Party channel' }), { label: '#limbo-trio' });
    },
  },
  {
    kind: 'Change a weekly run',
    title: 'Change weekly NBaldrix',
    fill: async (page) => {
      await subject(page, 'NBaldrix').click();
      await page.getByRole('spinbutton', { name: 'Time' }).fill('22:00');
      // Only the time changes: the day is not repeated.
      await expect(page.locator('.req-aside__line')).toHaveText('Thu 21:30 → 22:00');
    },
  },
];

for (const { kind, title, fill } of KINDS) {
  test(`${kind}: the form sends it and My requests opens on it, waiting`, async ({ page }) => {
    await form(page);
    await page.getByRole('radio', { name: kind }).check();
    await expect(page.getByRole('radio', { name: kind })).toBeChecked();
    await fill(page);
    await expect(page.getByRole('heading', { level: 2, name: title })).toBeVisible();
    await expect(sendKey(page)).not.toHaveAttribute('aria-disabled', 'true');
    await sendKey(page).click();
    await sent(page, title);
  });
}

test('a run named by the address picks Leave for your own run and Join for someone else’s', async ({ page }) => {
  await form(page, 'run=r-carling');
  await expect(page.getByRole('radio', { name: 'Leave a run' })).toBeChecked();
  await expect(subject(page, 'HCarling + HStar').first().getByRole('radio')).toBeChecked();
  await page.goto(`${PUBLIC}/requests/new?run=r-limbo&sw=off`);
  await expect(page.getByRole('radio', { name: 'Join a run' })).toBeChecked();
  await expect(subject(page, 'HLimbo').getByRole('radio')).toBeChecked();
  // A link asking to leave a run she's not in: the run stays, blocked, saying which kind can.
  await page.goto(`${PUBLIC}/requests/new?run=r-limbo&kind=leave&sw=off`);
  await expect(page.getByRole('radio', { name: 'Leave a run' })).toBeChecked();
  await expect(subject(page, 'HLimbo')).toContainText('not in this run · use Join');
  await expect(subject(page, 'HLimbo').getByRole('radio')).toBeDisabled();
});

test("Discord's weekly-change link prefills the timing, day and time", async ({ page }) => {
  await form(page, 'fixed=f-kalos&change=edit&day=thu&time=21:00');
  await expect(page.getByRole('radio', { name: 'Change a weekly run' })).toBeChecked();
  await expect(page.locator('input[value="fixed:f-kalos"]')).toBeChecked();
  await expect(page.getByRole('radiogroup', { name: 'Day' }).getByRole('radio', { name: 'Thu' })).toHaveAttribute('aria-checked', 'true');
  await expect(page.getByRole('spinbutton', { name: 'Time' })).toHaveValue('21:00');
  await expect(page.locator('.req-aside__line')).toHaveText('Fri 21:30 → Thu 21:00');
  // change=remove asks to leave the weekly timing instead.
  await page.goto(`${PUBLIC}/requests/new?fixed=f-kalos&change=remove&sw=off`);
  await expect(page.getByRole('radio', { name: 'Leave a run' })).toBeChecked();
  await expect(page.locator('input[value="fixed:f-kalos"]')).toBeChecked();
});

test("a send older than the fresh window: Confirm it's you, back with the form kept, Send again", async ({ page }) => {
  // Aged before the page reads the session and devices, so both carry the same sign-in time.
  await signInPublic(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/unfresh`)).ok()).toBe(true);
  await page.goto(`${PUBLIC}/requests/new?run=r-carling&sw=off`);
  await expect(page.getByRole('radiogroup', { name: 'Request type' })).toBeVisible();
  await note(page).fill('Something came up on Tuesday evening.');
  await sendKey(page).click();
  const confirm = page.getByRole('dialog', { name: "Confirm it's you" });
  await expect(confirm).toContainText('Sending a request needs a Discord sign-in from the last 15 minutes.');
  await expect(confirm).toContainText(
    'Your request is kept: Leave HCarling + HStar, Tue 29 Sep 22:00 · #hstar-party, with your note. You come back to this page and press Send once more.',
  );
  await confirm.getByRole('link', { name: 'Sign in again' }).click();
  await expect(page).toHaveURL(/\/requests\/new\?kind=leave&run=r-carling&note=/);
  await expect(page.getByRole('radio', { name: 'Leave a run' })).toBeChecked();
  await expect(note(page)).toHaveValue('Something came up on Tuesday evening.');
  await sendKey(page).click();
  await sent(page, 'Leave HCarling + HStar');
  await expect(page.locator('.req-detail__note')).toHaveText('“Something came up on Tuesday evening.”');
});

test('at the open limit: the toast says nothing was sent, the key is off with the reason', async ({ page }) => {
  await form(page, 'run=r-carling');
  // Two more sent from another tab fill the three that may wait.
  expect(await sendElsewhere(page, { kind: 'join', run_id: 'r-limbo' }, 'e2e-limit-1')).toBe(201);
  expect(await sendElsewhere(page, { kind: 'join', run_id: 'r-fa' }, 'e2e-limit-2')).toBe(201);
  await sendKey(page).click();
  const refused = toast(page).filter({ hasText: 'Limit reached: 3 requests waiting. Nothing was sent.' });
  await expect(refused).toBeVisible();
  await expect(sendKey(page)).toHaveAttribute('aria-disabled', 'true');
  await expect(sendKey(page)).toHaveAccessibleDescription('3 requests are already waiting. Withdraw one in My requests, or send this once an admin decides.');
  await expect(page.locator('.req-count')).toHaveText('3 of 3 open · 3 of 6 today');
  // The key stays put: another press sends nothing.
  let posts = 0;
  page.on('request', (request) => request.method() === 'POST' && request.url().endsWith('/api/public/requests') && posts++);
  // aria-disabled keeps it focusable with its reason; a forced press still sends nothing.
  await sendKey(page).click({ force: true });
  expect(posts).toBe(0);
  await refused.getByRole('button', { name: 'My requests' }).click();
  await expect(page).toHaveURL(`${PUBLIC}/requests`);
});

test('the note: one line, 200 characters at most, counted as typed; the server refuses more', async ({ page }) => {
  await form(page, 'run=r-carling');
  await note(page).fill('x'.repeat(200));
  await expect(page.locator('.req-note__hint')).toContainText('200/200');
  await expect(sendKey(page)).not.toHaveAttribute('aria-disabled', 'true');
  await note(page).fill('x'.repeat(201));
  await expect(page.locator('.req-note__hint')).toContainText('201/200');
  await expect(page.locator('.req-note__hint')).toHaveClass(/req-note__hint--over/);
  await expect(sendKey(page)).toHaveAttribute('aria-disabled', 'true');
  await expect(sendKey(page)).toHaveAccessibleDescription('Keep the note to 200 characters.');
  expect(await sendElsewhere(page, { kind: 'leave', run_id: 'r-carling', note: 'x'.repeat(201) }, 'e2e-note-201')).toBe(422);
  expect(await sendElsewhere(page, { kind: 'leave', run_id: 'r-carling', note: 'x'.repeat(200) }, 'e2e-note-200')).toBe(201);
});

test('My requests: newest first, each state on its chip; the open one says where it stands', async ({ page }) => {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/requests?sw=off`);
  const rows = page.getByRole('navigation', { name: 'Your requests' }).getByRole('link');
  await expect(rows).toHaveCount(5);
  const chips = await rows.evaluateAll((links) => links.map((a) => [(a as HTMLElement).dataset.request, a.querySelector('.req-chip')?.textContent?.trim()]));
  expect(chips).toEqual([
    ['req-kalos-weekly', 'waiting'],
    ['req-jupiter-join', 'approved'],
    ['req-bellona-join', 'declined'],
    ['req-jupiter-new', 'withdrawn'],
    ['req-carling-swap', 'expired'],
  ]);
  await expect(page.locator('.pageline__context')).toHaveText('· 1 waiting · 2 decided');
  // The first is open by default: waiting, with Withdraw….
  await expect(rows.first()).toHaveAttribute('aria-current', 'page');
  await expect(page.locator('.req-detail')).toContainText('Waiting on the admins. The weekly timing stays Fri 21:30 until one decides');
  await expect(page.getByRole('button', { name: 'Withdraw…' })).toBeVisible();
  // Declined: the admin's reason.
  await rows.nth(2).click();
  await expect(page).toHaveURL(/open=req-bellona-join/);
  await expect(page.locator('.req-detail')).toContainText('Declined by Ren');
  await expect(page.locator('.req-detail')).toContainText('“party is full” Nothing changed.');
  await expect(page.getByRole('button', { name: 'Withdraw…' })).toHaveCount(0);
  // Approved: where the change shows.
  await rows.nth(1).click();
  await expect(page.locator('.req-detail')).toContainText('Approved by Ren');
  await expect(page.locator('.req-detail').getByRole('link', { name: 'Open the run' })).toBeVisible();
  // Expired (board Requests-Expired): nothing changed, and Ask again for this week's run.
  await rows.nth(4).click();
  await expect(page.locator('.req-detail')).toContainText('No admin decided before the boss week ended, so this request expired');
  await expect(page.locator('.req-detail').getByRole('link', { name: 'Ask again for this week…' })).toHaveAttribute('href', '/requests/new?run=r-carling&kind=swap');
  // Withdrawn.
  await rows.nth(3).click();
  await expect(page.locator('.req-detail')).toContainText('You withdrew this request');
});

test('Withdraw… confirms first; Keep it changes nothing, Withdraw closes the request', async ({ page }) => {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/requests?sw=off`);
  await page.getByRole('button', { name: 'Withdraw…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Withdraw this request?' });
  await expect(dialog).toContainText("Change weekly XKalos stops waiting on the admins and nothing changes. It still counts toward today's 6.");
  await dialog.getByRole('button', { name: 'Keep it' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.locator('[data-request="req-kalos-weekly"] .req-chip')).toHaveText('waiting');
  await page.getByRole('button', { name: 'Withdraw…' }).click();
  await dialog.getByRole('button', { name: 'Withdraw', exact: true }).click();
  await expect(toast(page).filter({ hasText: 'Withdrawn: Change weekly XKalos. Nothing changed.' })).toBeVisible();
  await expect(page.locator('[data-request="req-kalos-weekly"] .req-chip')).toHaveText('withdrawn');
  await expect(page.locator('[data-request="req-kalos-weekly"]')).toBeFocused();
  await expect(page.locator('.req-count')).toHaveText('0 of 3 open · 1 of 6 today');
  // The masthead's count follows.
  await expect(page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: /^Requests/ })).not.toContainText('1');
});

/** Every visible button and link in `scope` is at least 44 px tall (rounded like a device pixel). */
async function tall(scope: Locator): Promise<void> {
  for (const control of await scope.locator('button:visible, a.btn:visible, a.req-row__head:visible').all())
    expect(Math.round((await control.boundingBox())!.height), await control.innerText()).toBeGreaterThanOrEqual(44);
}

test('phone: the request form is rows with a bottom Send, every key 44 px', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signInPublic(page);
  await page.goto(`${PUBLIC}/requests/new?run=r-carling&sw=off`);
  const kinds = page.getByRole('radiogroup', { name: 'Request type' });
  await expect(kinds).toBeVisible();
  for (const row of await kinds.locator('label').all()) expect(Math.round((await row.boundingBox())!.height)).toBeGreaterThanOrEqual(44);
  await tall(page.locator('.req-form__foot'));
  const foot = (await page.locator('.req-form__foot').boundingBox())!;
  expect(Math.round(foot.y + foot.height)).toBeLessThanOrEqual(844);
  expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(0);
  await sendKey(page).click();
  await expect(page).toHaveURL(/\/requests\?open=req-/);
});

test('phone: My requests unfolds the open row in place with a 44 px Withdraw…', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signInPublic(page);
  await page.goto(`${PUBLIC}/requests?sw=off`);
  const row = page.locator('[data-request="req-kalos-weekly"]');
  await expect(row).toHaveAttribute('aria-expanded', 'false');
  await row.click();
  await expect(row).toHaveAttribute('aria-expanded', 'true');
  await tall(page.locator('.req-list'));
  expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(0);
});

// Axe on the new pages in both faces of the default colourway, as a11y.spec.ts walks them.
async function serious(page: Page, label: string): Promise<void> {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

for (const theme of ['light', 'dark'] as const) {
  for (const size of [
    { width: 1280, height: 800 },
    { width: 390, height: 844 },
  ]) {
    test(`axe: Move, the request form, Confirm it's you and My requests, ${theme} ${size.width}`, async ({ page }) => {
      test.setTimeout(60_000);
      await page.setViewportSize(size);
      await page.addInitScript((t) => {
        localStorage.setItem('colorway', 'marigold');
        localStorage.setItem('theme', t);
      }, theme);
      await signInPublic(page);
      await page.goto(`${PUBLIC}/runs/r-carling?move_to=${encodeURIComponent('2026-09-30T13:30:00Z')}&sw=off`);
      await expect(page.locator('.movepick__result')).toContainText('Wed 30 21:30');
      await serious(page, 'move');
      await page.goto(`${PUBLIC}/runs/p-kalos?move_to=${encodeURIComponent('2026-09-18T14:00:00Z')}&sw=off`);
      await expect(page.locator('.move-page__notice')).toBeVisible();
      await serious(page, 'move week over');
      await page.goto(`${PUBLIC}/requests/new?run=r-carling&sw=off`);
      await expect(note(page)).toBeVisible();
      await serious(page, 'request form');
      await page.request.post(`${PUBLIC}/__mock/public/unfresh`);
      await sendKey(page).click();
      await expect(page.getByRole('dialog', { name: "Confirm it's you" })).toBeVisible();
      await serious(page, 'confirm');
      await page.goto(`${PUBLIC}/requests?open=req-carling-swap&sw=off`);
      await expect(page.locator('[data-request="req-carling-swap"]')).toBeVisible();
      await serious(page, 'requests');
    });
  }
}
