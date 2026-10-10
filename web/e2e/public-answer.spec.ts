import type { Page } from '@playwright/test';
import { ADMIN, PUBLIC, csrf, expect, signInPublic, test } from './support';

// The member's answer on their own run (member-writes contract Phase A,
// boards Week-RunMine, ConfirmAnswer, PhoneRun) against the mock's member
// (Asahi, 1001) on HCarling + HStar (Tue 29 22:00, In): shown at once,
// rolled back when refused, kept across the fresh sign-in, and a stale week
// rolls back with what changed. Every test ends with zero CSP/TT reports.

test.describe.configure({ mode: 'parallel' });

const ASAHI = '100000000000001001';
const toast = (page: Page) => page.getByRole('group', { name: 'Notification' });
const pane = (page: Page) => page.getByRole('complementary', { name: /^Your run · .*Carling/ });
const answers = (page: Page) => pane(page).getByRole('group', { name: /^Your answer/ });
const choice = (page: Page, name: 'In' | 'Maybe' | 'Out') => answers(page).getByRole('button', { name: new RegExp(`^${name}`) });

async function open(page: Page, query = 'run=r-carling'): Promise<void> {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/?${query}&sw=off`);
  await expect(choice(page, 'In')).toHaveAttribute('aria-pressed', 'true');
}

/** Asahi's answer on a run as the server holds it now. */
async function saved(page: Page, run = 'r-carling'): Promise<string | undefined> {
  const week = (await (await page.request.get(`${PUBLIC}/api/public/week`)).json()) as { runs: { id: string; participants: { id: string; answer: string }[] }[] };
  return week.runs.find((r) => r.id === run)?.participants.find((p) => p.id === ASAHI)?.answer;
}

test('an answer shows at once, then saves; the tally follows', async ({ page }) => {
  await open(page);
  // Hold the write: the press already shows.
  let release: () => void = () => {};
  const held = new Promise<void>((resolve) => (release = resolve));
  await page.route('**/api/public/runs/r-carling/answer', async (route) => {
    await held;
    await route.continue();
  });
  await choice(page, 'Maybe').click();
  await expect(choice(page, 'Maybe')).toHaveAttribute('aria-pressed', 'true');
  await expect(choice(page, 'Maybe')).toHaveText('Maybe ✓');
  await expect(answers(page)).toHaveAttribute('aria-busy', 'true');
  release();
  await expect(answers(page)).toHaveAttribute('aria-busy', 'false');
  await expect(answers(page)).toHaveAccessibleName('Your answer: Maybe');
  expect(await saved(page)).toBe('maybe');
  // Out puts the live run at risk, as the admin's RSVP does.
  await choice(page, 'Out').click();
  await expect(answers(page)).toHaveAccessibleName('Your answer: Out');
  await expect(pane(page)).toContainText('at risk');
  expect(await saved(page)).toBe('no');
});

test('a refused answer rolls back and the toast says why', async ({ page }) => {
  await open(page);
  await page.route('**/api/public/runs/r-carling/answer', (route) =>
    route.fulfill({ status: 403, contentType: 'application/json', body: JSON.stringify({ error: 'not_in_run', message: "You're not in this run." }) }),
  );
  await choice(page, 'Out').click();
  await expect(toast(page).filter({ hasText: "Couldn't save your answer: You're not in this run." })).toBeVisible();
  await expect(choice(page, 'In')).toHaveAttribute('aria-pressed', 'true');
  await expect(choice(page, 'Out')).toHaveAttribute('aria-pressed', 'false');
  expect(await saved(page)).toBe('yes');
});

test("an answer older than the fresh window: Confirm it's you, back with the choice kept, one more press saves", async ({ page }) => {
  // Aged before the page reads the session and devices, so both carry the same sign-in time.
  await signInPublic(page);
  expect((await page.request.post(`${PUBLIC}/__mock/public/unfresh`)).ok()).toBe(true);
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  await expect(choice(page, 'In')).toHaveAttribute('aria-pressed', 'true');
  await choice(page, 'Maybe').click();
  const confirm = page.getByRole('dialog', { name: "Confirm it's you" });
  await expect(confirm).toBeVisible();
  await expect(confirm).toContainText('Changing your answer needs a Discord sign-in from the last 15 minutes.');
  await expect(confirm).toContainText(
    "Not saved yet: Maybe for HCarling + HStar, Tue 29 Sep 22:00. After signing in you're back on this run with Maybe picked; press it once more to save.",
  );
  // Nothing was saved, and the press is rolled back behind the dialog.
  await expect(choice(page, 'In')).toHaveAttribute('aria-pressed', 'true');
  expect(await saved(page)).toBe('yes');
  const signIn = confirm.getByRole('link', { name: 'Sign in again' });
  await expect(signIn).toHaveAttribute('href', `/api/public/auth/discord/start?next=${encodeURIComponent('/?run=r-carling&answer=maybe')}`);

  // Through the Discord stand-in and back: the run is open with Maybe picked, not saved yet.
  await signIn.click();
  await expect(page).toHaveURL(/\/\?run=r-carling$/);
  await expect(pane(page).getByRole('status')).toHaveText('Not saved yet: Maybe. Press it once more to save.');
  expect(await saved(page)).toBe('yes');
  await choice(page, 'Maybe').click();
  await expect(answers(page)).toHaveAccessibleName('Your answer: Maybe');
  await expect(pane(page).getByRole('status')).toHaveCount(0);
  expect(await saved(page)).toBe('maybe');
});

test('a stale answer rolls back to what the server has and says what changed', async ({ page }) => {
  await open(page);
  // An admin marks Asahi out after the page read the week (the admin RSVP takes yes, no or clear).
  const week = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number };
  // The admin API names members by the mock's short ids.
  const changed = await page.request.post(`${ADMIN}/api/admin/runs/r-carling/rsvp`, { headers: await csrf(page.request), data: { member_id: '1001', answer: 'no', version: week.version } });
  expect(changed.ok()).toBe(true);
  await choice(page, 'Maybe').click();
  await expect(
    toast(page).filter({ hasText: 'HCarling + HStar changed meanwhile: your answer is now Out, it is now at risk. Your answer (Maybe) was not saved.' }),
  ).toBeVisible();
  await expect(answers(page)).toHaveAccessibleName('Your answer: Out');
  expect(await saved(page)).toBe('no');
});

test('someone else’s run has no answer or move, only the ask to join', async ({ page }) => {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/?run=r-limbo&sw=off`);
  const other = page.getByRole('complementary', { name: /^View only · .*Limbo/ });
  await expect(other).toContainText("You're not in this run, so you can't answer or move it.");
  await expect(other.getByRole('group', { name: /^Your answer/ })).toHaveCount(0);
  await expect(other.getByRole('region', { name: /^Move/ })).toHaveCount(0);
  await expect(other.getByRole('button', { name: /^(In|Maybe|Out|Move)/ })).toHaveCount(0);
  await expect(other.getByRole('link', { name: 'Ask to join…' })).toHaveAttribute('href', '/requests/new?run=r-limbo');
  // The server refuses it too.
  const session = await page.request.get(`${PUBLIC}/api/public/session`);
  const refused = await page.request.put(`${PUBLIC}/api/public/runs/r-limbo/answer`, {
    headers: { 'X-Kanade-CSRF': session.headers()['x-kanade-csrf'] ?? '', 'Idempotency-Key': 'e2e-other-answer' },
    data: { answer: 'yes', version: 0 },
  });
  expect(refused.status()).toBe(403);
});

test('phone: the run sheet answers with 44 px choices', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signInPublic(page);
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  const group = page.getByRole('group', { name: /^Your answer/ });
  await expect(group).toBeVisible();
  for (const button of await group.getByRole('button').all()) expect(Math.round((await button.boundingBox())!.height)).toBeGreaterThanOrEqual(44);
  await group.getByRole('button', { name: /^Maybe/ }).click();
  await expect(group).toHaveAccessibleName('Your answer: Maybe');
  expect(await saved(page)).toBe('maybe');
});
