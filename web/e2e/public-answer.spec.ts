import type { Locator, Page } from '@playwright/test';
import { ADMIN, PUBLIC, csrf, expect, signInPublic, test } from './support';

// The member's answer on their own run (member-writes contract Phase A,
// boards Week-RunMine, ConfirmAnswer, PhoneRun) as the real server's member
// (Asahi) on HCarling + HMaleficStar (Tue 29 22:00, In): shown at once,
// rolled back when refused, kept across the fresh sign-in, and a stale week
// rolls back with what changed. Every test ends with zero CSP/TT reports.

test.describe.configure({ mode: 'parallel' });

const ASAHI = '100000000000001001';
const toast = (page: Page) => page.getByRole('group', { name: 'Notification' });
// The pane is named with the bosses' display names (the real catalog's Karling).
const pane = (page: Page) => page.getByRole('complementary', { name: /^Your run · .*Karling/ });
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
  await expect.poll(() => saved(page)).toBe('maybe');
  // Out puts the live run at risk, as the admin's RSVP does.
  await choice(page, 'Out').click();
  await expect(answers(page)).toHaveAccessibleName('Your answer: Out');
  await expect(pane(page)).toContainText('at risk');
  await expect.poll(() => saved(page)).toBe('no');
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
  // Signed in before the fresh window, so the session and devices reads carry the same sign-in time.
  await signInPublic(page, { stale: true });
  await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
  await expect(choice(page, 'In')).toHaveAttribute('aria-pressed', 'true');
  await choice(page, 'Maybe').click();
  const confirm = page.getByRole('dialog', { name: "Confirm it's you" });
  await expect(confirm).toBeVisible();
  await expect(confirm).toContainText('Changing your answer needs a Discord sign-in from the last 15 minutes.');
  await expect(confirm).toContainText(
    "Not saved yet: Maybe for HCarling + HMaleficStar, Tue 29 Sep 22:00. After signing in you're back on this run with Maybe picked; press it once more to save.",
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
  // The press shows at once; the real write lands a moment later.
  await expect(answers(page)).toHaveAttribute('aria-busy', 'false');
  await expect.poll(() => saved(page)).toBe('maybe');
});

test('a stale answer rolls back to what the server has and says what changed', async ({ page }) => {
  // The real server pushes a change hint the moment the admin writes, and the
  // page would re-read before the press; with the stream held the page stays
  // on the week it read, the case this test is about.
  await page.route('**/api/public/events', () => {});
  await open(page);
  // An admin marks Asahi out after the page read the week (the admin RSVP takes yes, no or clear).
  const week = (await (await page.request.get(`${ADMIN}/api/admin/week`)).json()) as { version: number };
  // The admin API names members by their Discord ids.
  const changed = await page.request.post(`${ADMIN}/api/admin/runs/r-carling/rsvp`, { headers: await csrf(page.request), data: { member_id: ASAHI, answer: 'no', version: week.version } });
  expect(changed.ok()).toBe(true);
  await choice(page, 'Maybe').click();
  await expect(
    toast(page).filter({ hasText: 'HCarling + HMaleficStar changed meanwhile: your answer is now Out, it is now at risk. Your answer (Maybe) was not saved.' }),
  ).toBeVisible();
  await expect(answers(page)).toHaveAccessibleName('Your answer: Out');
  await expect.poll(() => saved(page)).toBe('no');
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
    headers: { 'X-Kanade-CSRF': session.headers()['x-kanade-csrf'] ?? '', 'Idempotency-Key': 'e2e-other-answer', Origin: PUBLIC },
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
  await expect.poll(() => saved(page)).toBe('maybe');
});

// One press, one morph (the user's "the animation plays twice"): a choice's
// shape follows only the choice (the chosen pill), so the press does not
// also step its corners down first. Counted from the press until the spring
// has settled: corner transitions on the pressed choice, and any CSS
// animation on the page (an own answer is no arrival: no mark, no pulse).
async function motionOf(page: Page, button: Locator, press: () => Promise<void>): Promise<{ morphs: number; animations: string[] }> {
  const seen = await button.evaluateHandle((el) => {
    const seen = { morphs: 0, animations: [] as string[] };
    el.addEventListener('transitionstart', (event) => {
      if (event.target === el && event.propertyName === 'border-top-left-radius') seen.morphs += 1;
    });
    document.addEventListener('animationstart', (event) => seen.animations.push(event.animationName));
    return seen;
  });
  await press();
  await expect(button).toHaveAttribute('aria-pressed', 'true');
  // Past the tap's lingering :active and the 250 ms spring.
  await page.waitForTimeout(800);
  return seen.jsonValue();
}

/** A deliberate mouse press: held 120 ms, as a hand does. */
async function hold(page: Page, button: Locator): Promise<void> {
  const box = (await button.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(120);
  await page.mouse.up();
}

test('one press on an answer morphs it once and marks nothing: run pane', async ({ page }) => {
  await open(page);
  const maybe = choice(page, 'Maybe');
  expect(await motionOf(page, maybe, () => hold(page, maybe))).toEqual({ morphs: 1, animations: [] });
  await expect.poll(() => saved(page)).toBe('maybe');
});

test('one press on an answer morphs it once and marks nothing: My runs', async ({ page }) => {
  await signInPublic(page);
  await page.goto(`${PUBLIC}/mine?sw=off`);
  // My runs names each group with the run's boss tokens.
  const maybe = page.getByRole('group', { name: /^Your answer, .* HCarling \+ HMaleficStar$/ }).getByRole('button', { name: /^Maybe/ });
  await expect(maybe).toHaveAttribute('aria-pressed', 'false');
  expect(await motionOf(page, maybe, () => hold(page, maybe))).toEqual({ morphs: 1, animations: [] });
  await expect.poll(() => saved(page)).toBe('maybe');
});

test.describe('touch', () => {
  test.use({ hasTouch: true, isMobile: true, viewport: { width: 390, height: 844 } });

  test('one tap on an answer morphs it once and marks nothing: phone run', async ({ page }) => {
    await signInPublic(page);
    await page.goto(`${PUBLIC}/?run=r-carling&sw=off`);
    const maybe = page.getByRole('group', { name: /^Your answer/ }).getByRole('button', { name: /^Maybe/ });
    await expect(maybe).toHaveAttribute('aria-pressed', 'false');
    expect(await motionOf(page, maybe, () => maybe.tap())).toEqual({ morphs: 1, animations: [] });
    await expect.poll(() => saved(page)).toBe('maybe');
  });
});
