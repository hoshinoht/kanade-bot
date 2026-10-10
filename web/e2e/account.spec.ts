import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { ADMIN, expect, settle, test } from './support';

// Account (boards AdminAccount / AdminPhone, direction B): one window with
// Profile / Sessions / This browser tabs beside a fixed identity pane. The
// mock's Discord session is Asahi (staff, on the roster, saved style Kanade;
// the staff role sets Terse in Config › Persona); token and Tailscale
// sessions are neutral. The fixture resets the mock per test.

async function axe(page: Page, label: string) {
  await settle(page);
  const result = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice']).analyze();
  const bad = result.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${label}: ${v.id} ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`)).toEqual([]);
}

const tab = (page: Page, name: RegExp) => page.getByRole('tablist', { name: 'Account' }).getByRole('tab', { name });
const panel = (page: Page) => page.getByRole('tabpanel');
const noDocumentScroll = (page: Page) => page.evaluate(() => document.scrollingElement!.scrollHeight <= window.innerHeight);

test('a Discord session opens its account from the menu: identity, access, roles, allowance and reply style', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/?sw=off`);
  await page.getByRole('button', { name: /^Account: Asahi/ }).click();
  await page.getByRole('menu', { name: 'Account' }).getByRole('menuitem', { name: 'Your account' }).click();
  await expect(page).toHaveURL(`${ADMIN}/account`);
  await expect(page).toHaveTitle(/^Account — /);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Account');
  await expect(page.locator('.pageline__context')).toHaveText('Asahi · signed in with Discord');

  const you = page.getByRole('complementary', { name: 'You' });
  await expect(you.getByRole('heading', { name: 'Asahi' })).toBeVisible();
  await expect(you).toContainText('signed in with Discord');
  await expect(you.locator('dd').first()).toHaveText('1001');
  await expect(you).toContainText('Tue 29 Sep 12:00');
  await expect(you.getByRole('button', { name: 'Sign out' })).toBeVisible();

  await expect(tab(page, /^Profile/)).toHaveAttribute('aria-selected', 'true');
  const profile = panel(page);
  await expect(profile).toContainText('Chatbot access');
  await expect(profile).toContainText('Exempt from chatbot budgets');
  await expect(profile.getByRole('list', { name: 'Server roles' }).getByRole('listitem')).toHaveText(['staff', 'bossers']);
  // Names only: no raw role ids on the page.
  await expect(profile).not.toContainText('300001');
  await expect(profile).toContainText('Exempt (staff)');
  await expect(profile.getByRole('progressbar')).toHaveCount(0);
  // The staff role's style beats the saved one; both are shown.
  await expect(profile).toContainText('In effect');
  await expect(profile).toContainText('set by your staff role');
  await expect(profile).toContainText('Your saved style: Kanade');
  await expect(profile.getByRole('link', { name: /Open my member profile/ })).toHaveAttribute('href', '/members?open=1001');
  await expect(profile.getByRole('link', { name: /See my changes/ })).toHaveAttribute('href', '/history?actor=admin%3Adiscord%3A1001');

  await profile.getByRole('button', { name: 'Recheck my access' }).click();
  await expect(profile).toContainText('Checked just now: Staff');
  await axe(page, 'account profile');
});

test('tabs: arrow keys move between them, the choice is in the URL and survives a reload', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/account?sw=off`);
  await tab(page, /^Profile/).focus();
  await page.keyboard.press('ArrowRight');
  await expect(tab(page, /^Sessions/)).toHaveAttribute('aria-selected', 'true');
  await expect(tab(page, /^Sessions/)).toBeFocused();
  await expect(page).toHaveURL(`${ADMIN}/account?tab=sessions`);
  await page.keyboard.press('ArrowRight');
  await expect(tab(page, /^This browser/)).toHaveAttribute('aria-selected', 'true');
  await page.reload();
  await expect(tab(page, /^This browser/)).toHaveAttribute('aria-selected', 'true');
  await expect(panel(page)).toContainText('Saved in this browser only');
  await tab(page, /^This browser/).focus();
  await page.keyboard.press('Home');
  await expect(page).toHaveURL(`${ADMIN}/account`);
});

test('the account links open my member sheet and History filtered to me', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/account?sw=off`);
  await panel(page).getByRole('link', { name: /Open my member profile/ }).click();
  await expect(page).toHaveURL(`${ADMIN}/members?open=1001`);
  await expect(page.locator('.side-pane, dialog[open]').first()).toContainText('Asahi');
  await page.goto(`${ADMIN}/account?sw=off`);
  await panel(page).getByRole('link', { name: /See my changes/ }).click();
  await expect(page).toHaveURL(/\/history\?actor=admin%3Adiscord%3A1001$/);
  await expect(page.getByRole('combobox', { name: 'Who' })).not.toContainText('everyone');
});

test('sessions: this one first and marked; sign out one, then everywhere else', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/account?tab=sessions&sw=off`);
  const list = panel(page).getByRole('list', { name: 'Active sessions' });
  await expect(list.getByRole('listitem')).toHaveCount(3);
  await expect(tab(page, /^Sessions/)).toContainText('3');
  const first = list.getByRole('listitem').first();
  await expect(first).toContainText('Chrome · macOS');
  await expect(first).toContainText('This one');
  await expect(first).toContainText('last seen now');
  await expect(first.getByRole('button')).toHaveCount(0);
  await expect(list).toContainText('Safari · iPhone');
  await expect(list).toContainText('40 min ago');
  await axe(page, 'account sessions');

  await list.getByRole('button', { name: /^Sign out Safari · iPhone/ }).click();
  await expect(page.getByText('Signed out Safari · iPhone.')).toBeVisible();
  await expect(list.getByRole('listitem')).toHaveCount(2);
  await expect(list).not.toContainText('Safari · iPhone');
  await expect(tab(page, /^Sessions/)).toContainText('2');

  await panel(page).getByRole('button', { name: 'Sign out everywhere else' }).click();
  await expect(page.getByText('Signed out 1 other session.')).toBeVisible();
  await expect(list.getByRole('listitem')).toHaveCount(1);
  await expect(panel(page).getByRole('button', { name: 'Sign out everywhere else' })).toHaveCount(0);
  await expect(panel(page)).toContainText('No other sessions');
});

test('reply style: the picker searches public styles, moves by keyboard, warns about the role and saves', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/account?sw=off`);
  const change = panel(page).getByRole('button', { name: 'Change…' });
  await change.click();
  const dialog = page.getByRole('dialog', { name: 'Reply style' });
  await expect(dialog).toBeVisible();
  const search = dialog.getByRole('searchbox', { name: 'Find a style' });
  await expect(search).toBeFocused();
  await expect(dialog.getByRole('note')).toContainText('Your staff role sets Terse, and role settings come first.');
  const radios = dialog.getByRole('radio');
  // The default voice, then the public styles A to Z; the private one is never offered.
  await expect(dialog.locator('.reply-picker__option .account-row__title')).toHaveText(['Default voice', 'Kanade· saved', 'Terse']);
  await expect(dialog).not.toContainText('Sparkly');
  await expect(radios.nth(1)).toBeChecked();
  await expect(dialog.getByRole('button', { name: 'Saved' })).toHaveAttribute('aria-disabled', 'true');

  await search.fill('blunt');
  await expect(radios).toHaveCount(1);
  await expect(dialog).toContainText('Short to the point of blunt');
  await search.fill('zzz');
  await expect(dialog).toContainText('No style matches “zzz”.');
  await search.fill('');
  await expect(radios).toHaveCount(3);

  // Down from the search lands on the checked style; arrows pick another.
  await search.press('ArrowDown');
  await expect(radios.nth(1)).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(radios.nth(2)).toBeChecked();
  await expect(dialog.getByRole('button', { name: 'Save Terse' })).toBeEnabled();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(change).toBeFocused();

  await change.click();
  await expect(radios.nth(1)).toBeChecked();
  await radios.nth(0).check();
  await dialog.getByRole('button', { name: 'Save Default voice' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText('Saved Default voice as your reply style. Your staff role still sets Terse.')).toBeVisible();
  await expect(panel(page)).toContainText('Your saved style: Default voice');
  await axe(page, 'account reply picker closed');
});

test('reply style: the picker passes axe while open', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/account?sw=off`);
  await panel(page).getByRole('button', { name: 'Change…' }).click();
  await expect(page.getByRole('dialog', { name: 'Reply style' }).getByRole('radio').first()).toBeVisible();
  await axe(page, 'account reply picker');
});

test('this browser: look, Reduce motion, Discord links, Experiments and the shortcuts', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/account?tab=browser&sw=off`);
  const p = panel(page);
  await expect(p.getByRole('radio', { name: 'Otonose' })).toBeVisible();
  // Off by default (the device decides); reduce-motion.spec covers what it does.
  await expect(p.getByRole('switch', { name: 'Reduce motion' })).toHaveAttribute('aria-checked', 'false');
  await expect(p.getByRole('switch', { name: 'Reduce motion' })).toHaveAccessibleDescription(/even if your system allows motion/);
  await expect(p.getByRole('switch', { name: 'Open Discord links in the app' })).toHaveAttribute('aria-checked', 'true');
  const exp = p.getByRole('switch', { name: 'Experiments' });
  const before = await exp.getAttribute('aria-checked');
  await exp.click();
  await expect(exp).toHaveAttribute('aria-checked', before === 'true' ? 'false' : 'true');
  await expect(p.getByRole('list').last().getByRole('listitem')).toHaveCount(7);
  await expect(p).toContainText('Open commands');
  await axe(page, 'account browser');
});

test('a metered member sees when the oldest answer frees up, on the server clock, and a due reset re-reads', async ({ page }) => {
  // A browser clock days away from the mock's, paused so only runFor moves it.
  await page.clock.install({ time: new Date('2026-10-02T21:13:00Z') });
  await page.clock.pauseAt(new Date('2026-10-02T21:13:01Z'));
  await page.request.post(`${ADMIN}/__mock/session`, { data: { method: 'discord', user: '1010' } });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/account?sw=off`);
  const allowance = panel(page).getByRole('region', { name: 'Chat allowance' });
  await expect(allowance).toContainText('7 of 20 answers used');
  await expect(allowance).toContainText('resets in 5 h 12 m');
  await expect(allowance).toContainText('Your own allowance (overridden)');

  // Ren: the guild's 5 min window, oldest answer freeing up in 2 m 12 s.
  await page.request.post(`${ADMIN}/__mock/session`, { data: { method: 'discord', user: '1002' } });
  let reads = 0;
  page.on('request', (request) => {
    if (new URL(request.url()).pathname === '/api/admin/me') reads += 1;
  });
  await page.reload();
  await expect(allowance).toContainText('2 of 4 answers used');
  await expect(allowance).toContainText('resets in 2 m 12 s');
  await page.clock.runFor(2000);
  await expect(allowance).toContainText('resets in 2 m 10 s');
  const before = reads;
  // Due: one quiet re-read brings the window as the server sees it now.
  await page.clock.runFor(131_000);
  await expect.poll(() => reads).toBe(before + 1);
  // The fresh window counts from the re-read (the rest of runFor ticks on).
  await expect(allowance).toContainText(/resets in 2 m 1[12] s/);
  await page.clock.runFor(5000);
  expect(reads).toBe(before + 1);
  await page.clock.resume();
  await axe(page, 'account metered allowance');
});

for (const [method, how, name] of [
  ['token', 'signed in with a token', 'Break-glass token'],
  ['tailscale', 'signed in with Tailscale', 'ops@example.test'],
] as const) {
  test(`a ${method} session is neutral: not a Discord member, still its own sessions`, async ({ page }) => {
    await page.request.post(`${ADMIN}/__mock/session`, { data: { method } });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${ADMIN}/account?sw=off`);
    await expect(page.locator('.pageline__context')).toHaveText(`${name} · ${how}`);
    const you = page.getByRole('complementary', { name: 'You' });
    await expect(you.getByRole('heading', { name })).toBeVisible();
    await expect(you.locator('dd').first()).toHaveText('—');
    await expect(panel(page)).toContainText('Not a Discord member.');
    for (const gone of ['Chatbot access', 'Chat allowance', 'Reply style', 'Open my member profile']) await expect(panel(page)).not.toContainText(gone);
    await axe(page, `account ${method}`);
    await tab(page, /^Sessions/).click();
    await expect(panel(page).getByRole('list', { name: 'Active sessions' }).getByRole('listitem').first()).toContainText('This one');
    await expect(panel(page).getByRole('list', { name: 'Active sessions' }).getByRole('listitem').first()).toContainText(method === 'token' ? 'Token' : 'Tailscale');
  });
}

test('phone 390×844: identity strip above the tabs’ panel, nothing scrolls but the panel, the picker is a full-screen sheet', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/account?sw=off`);
  await expect(page.locator('.account-strip')).toContainText('Asahi');
  await expect(page.getByRole('complementary', { name: 'You' })).toHaveCount(0);
  await expect(tab(page, /^Browser/)).toBeVisible();
  for (const t of [/^Profile/, /^Sessions/, /^Browser/]) {
    await tab(page, t).click();
    await settle(page);
    expect(await noDocumentScroll(page)).toBe(true);
    const box = (await page.locator('.account-window__panel').boundingBox())!;
    expect(box.x + box.width).toBeLessThanOrEqual(390);
    expect(await page.locator('.account-window__panel').evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  }
  await tab(page, /^Sessions/).click();
  await expect(panel(page).getByRole('button', { name: 'Sign out everywhere else' })).toBeVisible();
  await tab(page, /^Profile/).click();
  // Diagnostics and Sign out move into the panel on phones.
  await expect(panel(page).getByRole('button', { name: 'Sign out' })).toBeVisible();
  await panel(page).getByRole('button', { name: /^Terse/ }).click();
  const dialog = page.getByRole('dialog', { name: 'Reply style' });
  await expect(dialog).toBeVisible();
  await settle(page);
  const sheet = (await dialog.locator('.modal__panel').boundingBox())!;
  expect(sheet.width).toBeGreaterThanOrEqual(389);
  expect(sheet.height).toBeGreaterThanOrEqual(843);
  await axe(page, 'account phone picker');
});
