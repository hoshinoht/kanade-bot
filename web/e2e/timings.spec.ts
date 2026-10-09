import type { Page } from '@playwright/test';
import { PUBLIC, expect, settle, signInPublic, test } from './support';

// My runs › Weekly timings (boards MyRuns-Timings-Owner, PhoneMyRuns-Timings-Owner)
// against the mock's member (Asahi, 1001): she owns Baldrix (Thu) and Carling
// (Tue, Mika asks to own it), Ren owns Kalos (Fri), Minato owns Jupiter (Mon).
// Every write goes to the mock; the fresh-sign-in refusal and a party change
// the mock cannot make are answered by `page.route`.

test.describe.configure({ mode: 'parallel' });

const SHOTS = process.env.KANADE_TIMINGS_SHOTS;
async function shot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  await settle(page);
  await page.screenshot({ path: `${SHOTS}/${name}.png` });
}

async function open(page: Page, size: 'desktop' | 'phone' = 'desktop'): Promise<void> {
  await page.setViewportSize(size === 'phone' ? { width: 390, height: 844 } : { width: 1280, height: 800 });
  await signInPublic(page);
  await page.goto(`${PUBLIC}/mine?week=timings&sw=off`);
  await expect(page.locator('[data-timing]')).toHaveCount(4);
}

const row = (page: Page, id: string) => page.locator(`[data-timing="${id}"]`);
const toast = (page: Page) => page.getByRole('group', { name: 'Notification' });

for (const size of ['desktop', 'phone'] as const) {
  test(`weekly timings (${size}): boss-week order, the owner crowned, the action each member has`, async ({ page }) => {
    await open(page, size);
    const tab = page.getByRole('tab', { name: /^(Weekly timings|Timings)/ });
    await expect(tab).toHaveAttribute('aria-selected', 'true');
    await expect(tab.locator('.tabs__count')).toHaveText('4');
    // The boss week starts Thursday.
    expect(await page.locator('[data-timing]').evaluateAll((rows) => rows.map((r) => (r as HTMLElement).dataset.timing))).toEqual(['f-baldrix', 'f-kalos', 'f-jupiter', 'f-carling']);
    await expect(row(page, 'f-baldrix')).toContainText('Thu');
    await expect(row(page, 'f-baldrix').locator('.owner')).toHaveText('You, owner');
    await expect(row(page, 'f-kalos').locator('.owner')).toHaveText('Ren, owner');
    await expect(row(page, 'f-baldrix').getByRole('button', { name: 'Hand Thu 21:30 to another party member' })).toBeVisible();
    await expect(row(page, 'f-kalos').getByRole('button', { name: 'Ask to own Fri 21:30' })).toBeVisible();
    // Mika's open ask on Asahi's Carling, with its time left by the server clock.
    const ask = row(page, 'f-carling').getByRole('group', { name: 'Mika asks to own this timing' });
    await expect(ask).toContainText('expires in 4 h');
    // Nothing the API does not serve yet: no Always in, attendance or suggestion.
    await expect(page.getByRole('switch')).toHaveCount(0);
    await expect(page.getByText('Always in')).toHaveCount(0);
    if (size === 'phone') {
      // Every control is a 44 px target, and nothing scrolls sideways.
      for (const button of await page.locator('[data-timing] button').all()) expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
      const panel = page.locator('#mine-panel');
      expect(await panel.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
    }
    await shot(page, `${size}-1-list`);
    // This and Next week still work beside it.
    await page.getByRole('tab', { name: /^(This week)/ }).click();
    await expect(page).toHaveURL(/\/mine\?sw=off$/);
    await expect(page.locator('[data-timing]')).toHaveCount(0);
  });
}

test('hand off: pick a party member, the owner changes, the toast says it as Discord does', async ({ page }) => {
  await open(page);
  const hand = row(page, 'f-carling').getByRole('button', { name: /^Hand Tue 22:00/ });
  await hand.click();
  const picker = page.getByRole('dialog', { name: 'Hand Tue 22:00 Carling + Radiant Malefic Star to…' });
  await expect(picker).toBeVisible();
  await expect(picker.getByRole('radio')).toHaveCount(5);
  await expect(picker.getByRole('radio', { name: 'Ren' })).toBeChecked();
  // Mika's ask is marked in the list.
  await expect(picker.locator('label', { hasText: 'Mika' })).toContainText('asked to own');
  await picker.getByRole('radio', { name: 'Mika' }).check();
  await expect(picker).toContainText("You → Mika. Discord posts “👑 Mika now owns weekly timing HCarling + HStar · Tue 22:00”. Mika's ask closes: its owner changed.");
  await shot(page, 'desktop-3-picker');
  await picker.getByRole('button', { name: 'Hand to Mika' }).click();
  await expect(picker).toBeHidden();
  await expect(toast(page).filter({ hasText: '👑 Mika now owns weekly timing HCarling + HStar · Tue 22:00.' })).toBeVisible();
  await expect(row(page, 'f-carling').locator('.owner')).toHaveText('Mika, owner');
  await expect(row(page, 'f-carling').getByRole('button', { name: 'Ask to own Tue 22:00' })).toBeVisible();
  await expect(row(page, 'f-carling').getByRole('group')).toHaveCount(0);
  await shot(page, 'desktop-5-handed');
});

test('ask to own: the pending chip with Withdraw, then withdrawn', async ({ page }) => {
  await open(page);
  await row(page, 'f-kalos').getByRole('button', { name: 'Ask to own Fri 21:30' }).click();
  const pending = row(page, 'f-kalos').locator('.timing__chip');
  await expect(pending).toHaveText('Asked Ren · expires in 24 h');
  await expect(page.locator('main [role="status"]').filter({ hasText: 'Asked Ren to own XKalos · Fri 21:30.' })).toHaveCount(1);
  const withdraw = row(page, 'f-kalos').getByRole('button', { name: 'Withdraw your ask to own Fri 21:30' });
  // Focus follows the row once Ask became Withdraw.
  await expect(withdraw).toBeFocused();
  await shot(page, 'desktop-2-pending');
  await withdraw.click();
  await expect(pending).toHaveCount(0);
  await expect(row(page, 'f-kalos').getByRole('button', { name: 'Ask to own Fri 21:30' })).toBeVisible();
});

test("an ask on your timing: Decline closes it and you stay the owner", async ({ page }) => {
  await open(page);
  const ask = row(page, 'f-carling').getByRole('group', { name: 'Mika asks to own this timing' });
  await ask.getByRole('button', { name: "Decline Mika's ask" }).click();
  await expect(ask).toHaveCount(0);
  await expect(row(page, 'f-carling').locator('.owner')).toHaveText('You, owner');
  await expect(page.locator('main [role="status"]').filter({ hasText: "Declined Mika's ask to own HCarling + HStar · Tue 22:00." })).toHaveCount(1);
});

test('an ask on your timing: Accept makes the asker the owner', async ({ page }) => {
  await open(page);
  await row(page, 'f-carling').getByRole('button', { name: "Accept Mika's ask" }).click();
  await expect(toast(page).filter({ hasText: '👑 Mika now owns weekly timing HCarling + HStar · Tue 22:00.' })).toBeVisible();
  await expect(row(page, 'f-carling').locator('.owner')).toHaveText('Mika, owner');
  await expect(row(page, 'f-carling').getByRole('group')).toHaveCount(0);
});

test("an owner change older than the fresh window: Confirm it's you, back with the pick, then handed", async ({ page }) => {
  await open(page);
  // The mock's clock stands still, so its sign-in never ages: answer as the server does after 15 min.
  await page.route('**/api/public/timings/*/owner', (route) =>
    route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify({ error: 'reauth_required', message: 'Sign in with Discord again to make this change.' }) }),
  );
  await row(page, 'f-baldrix').getByRole('button', { name: /^Hand Thu 21:30/ }).click();
  const picker = page.getByRole('dialog', { name: /^Hand Thu 21:30/ });
  await picker.getByRole('radio', { name: 'Mika' }).check();
  await picker.getByRole('button', { name: 'Hand to Mika' }).click();
  const confirm = page.getByRole('dialog', { name: "Confirm it's you" });
  await expect(confirm).toBeVisible();
  await expect(confirm).toContainText('Handing over a weekly timing needs a Discord sign-in from the last 15 minutes. Yours was at');
  await expect(confirm).toContainText("Not saved yet: Mika as owner of NBaldrix, Thu 21:30. After signing in you're back on Weekly timings with Mika picked; press Hand to Mika once more.");
  await shot(page, 'desktop-4-confirm');
  const signIn = confirm.getByRole('link', { name: 'Sign in again' });
  await expect(signIn).toHaveAttribute('href', `/api/public/auth/discord/start?next=${encodeURIComponent('/mine?week=timings&hand=f-baldrix&to=100000000000001003')}`);
  // Not now keeps everything as it was.
  await confirm.getByRole('button', { name: 'Not now' }).click();
  await expect(confirm).toBeHidden();
  await expect(row(page, 'f-baldrix').locator('.owner')).toHaveText('You, owner');

  // Through the Discord stand-in and back: the picker is open again with Mika picked.
  await page.unroute('**/api/public/timings/*/owner');
  await row(page, 'f-baldrix').getByRole('button', { name: /^Hand Thu 21:30/ }).click();
  await picker.getByRole('radio', { name: 'Mika' }).check();
  await page.route('**/api/public/timings/*/owner', (route) =>
    route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify({ error: 'reauth_required', message: 'Sign in with Discord again to make this change.' }) }),
    { times: 1 },
  );
  await picker.getByRole('button', { name: 'Hand to Mika' }).click();
  await confirm.getByRole('link', { name: 'Sign in again' }).click();
  await expect(page).toHaveURL(/\/mine\?week=timings$/);
  const back = page.getByRole('dialog', { name: /^Hand Thu 21:30/ });
  await expect(back.getByRole('radio', { name: 'Mika' })).toBeChecked();
  await back.getByRole('button', { name: 'Hand to Mika' }).click();
  await expect(row(page, 'f-baldrix').locator('.owner')).toHaveText('Mika, owner');
});

test('a refused accept: the toast says why with Reload, and the list is read again', async ({ page }) => {
  await open(page);
  await page.route('**/api/public/owner-requests/*/accept', (route) =>
    route.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ error: 'not_on_party', message: 'Ownership only moves between members of the party.' }) }),
  );
  const reads: string[] = [];
  page.on('request', (request) => request.url().endsWith('/api/public/timings') && reads.push(request.url()));
  await row(page, 'f-carling').getByRole('button', { name: "Accept Mika's ask" }).click();
  const refused = toast(page).filter({ hasText: "Couldn't accept: Mika is no longer on the party. Ownership only moves between members of the party." });
  await expect(refused).toBeVisible();
  await expect(refused.getByRole('button', { name: 'Reload' })).toBeVisible();
  expect(reads.length).toBeGreaterThanOrEqual(1);
  await shot(page, 'desktop-6-refused');
});

test("phone: Hand to… and Confirm it's you are bottom sheets with 44 px commits", async ({ page }) => {
  await open(page, 'phone');
  await row(page, 'f-carling').getByRole('button', { name: /^Hand Tue 22:00/ }).click();
  const sheet = page.getByRole('dialog', { name: /^Hand Tue 22:00/ });
  await expect(sheet).toBeVisible();
  await settle(page);
  const box = (await sheet.boundingBox())!;
  expect(Math.round(box.y + box.height)).toBe(844);
  expect(Math.round(box.width)).toBe(390);
  for (const name of ['Cancel', 'Hand to Ren']) expect((await sheet.getByRole('button', { name }).boundingBox())!.height).toBeGreaterThanOrEqual(44);
  await shot(page, 'phone-3-picker');
  await page.route('**/api/public/timings/*/owner', (route) =>
    route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify({ error: 'reauth_required', message: '' }) }),
  );
  await sheet.getByRole('button', { name: 'Hand to Ren' }).click();
  const confirm = page.getByRole('dialog', { name: "Confirm it's you" });
  await expect(confirm).toBeVisible();
  await settle(page);
  const cbox = (await confirm.boundingBox())!;
  expect(Math.round(cbox.y + cbox.height)).toBe(844);
  await shot(page, 'phone-4-confirm');
  await page.unroute('**/api/public/timings/*/owner');
  await confirm.getByRole('button', { name: 'Not now' }).click();
  // Asking on a phone: the chip and Withdraw on the row's own line.
  await row(page, 'f-kalos').getByRole('button', { name: 'Ask to own Fri 21:30' }).click();
  await expect(row(page, 'f-kalos').locator('.timing__chip')).toHaveText('Asked Ren · expires in 24 h');
  await shot(page, 'phone-2-pending');
  await row(page, 'f-carling').getByRole('button', { name: "Accept Mika's ask" }).click();
  await expect(toast(page).filter({ hasText: '👑 Mika now owns' })).toBeVisible();
  await shot(page, 'phone-5-handed');
});
