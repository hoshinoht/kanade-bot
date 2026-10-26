import { mkdirSync } from 'node:fs';
import type { Page } from '@playwright/test';
import { ADMIN, choose, expect, moreNamed, settle, SETUP_LEFT, test, toast, useLook } from './support';

// Admin Setup (`/setup`, `GET /api/admin/setup`): the default seed's real
// answer. The harness's gateway never connects (Discord connection: Missing)
// and one watched or digest channel lacks a permission (Bot permissions:
// Missing); the fake application has both intents and the six-permission
// invite; every optional item is on, backups without age recipients.

const REQUIRED = ['discord_gateway', 'discord_intents', 'bot_access', 'digest_channel', 'watched_channels', 'model_gateway', 'extraction_model'];
const OPTIONAL = ['chat', 'persona', 'admin_sign_in', 'public_portal', 'roster', 'character_data', 'guides', 'scouter', 'backups'];
const INVITE = 'https://discord.com/oauth2/authorize?client_id=3000000000000000001&scope=bot+applications.commands&permissions=93248';

const row = (page: Page, id: string) => page.locator(`.setup-row[data-item="${id}"]`);
const state = (page: Page, id: string) => row(page, id).locator('.status-chip');

async function open(page: Page, path = '/setup') {
  await page.goto(`${ADMIN}${path}${path.includes('?') ? '&' : '?'}sw=off`);
}

test('setup: the seed’s checklist, grouped, worded and counted', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${SETUP_LEFT} required steps left`);
  await expect(page).toHaveTitle(/^Setup — /);

  // Required first, then Optional, each in the API's order.
  const items = page.getByRole('region', { name: 'Setup items' });
  const required = items.getByRole('region', { name: /^Required/ });
  const optional = items.getByRole('region', { name: /^Optional/ });
  await expect(required.getByRole('heading', { level: 3 })).toHaveText('Required 5 of 7 ready');
  await expect(optional.getByRole('heading', { level: 3 })).toHaveText('Optional 9 of 9 ready');
  expect(await required.locator('.setup-row').evaluateAll((rows) => rows.map((r) => (r as HTMLElement).dataset.item))).toEqual(REQUIRED);
  expect(await optional.locator('.setup-row').evaluateAll((rows) => rows.map((r) => (r as HTMLElement).dataset.item))).toEqual(OPTIONAL);

  // States are words with a glyph, never colour alone.
  await expect(state(page, 'discord_gateway')).toHaveText('Missing');
  await expect(state(page, 'discord_intents')).toHaveText('Ready');
  await expect(state(page, 'bot_access')).toHaveText('Missing');
  for (const id of REQUIRED.slice(3)) await expect(state(page, id)).toHaveText('Ready');
  for (const id of OPTIONAL) await expect(state(page, id)).toHaveText('Ready');
  await expect(state(page, 'discord_gateway').locator('[data-icon="x"]')).toBeVisible();

  // One line of the app's own copy per item; the source when the API names one.
  await expect(row(page, 'discord_gateway')).toContainText('The bot isn\'t running on this server: add "bot" to runtime.components in kanade.toml and restart.');
  await expect(row(page, 'discord_gateway').locator('.setup-row__runbook')).toHaveText('Set in kanade.toml / the deploy runbook');
  await expect(row(page, 'discord_intents')).toContainText('Server Members Intent and Message Content Intent are on.');
  await expect(row(page, 'bot_access')).toContainText('1 channel lacks a permission the bot needs');
  await expect(row(page, 'digest_channel')).toContainText('The weekly digest posts in #boss-schedule.');
  await expect(row(page, 'digest_channel').locator('.setup-row__source')).toHaveText(/source:\sEnvironment$/);
  await expect(row(page, 'watched_channels')).toContainText('Reading 8 watched channels and categories.');
  await expect(row(page, 'persona').locator('.setup-row__source')).toHaveText(/source:\sSaved in Config$/);
  await expect(row(page, 'backups')).toContainText('Backups are on but not encrypted (the latest is from Tue 29 Sep 09:00); add age recipients.');
  await expect(row(page, 'model_gateway').locator('.setup-row__source')).toHaveCount(0);

  // The invite asks for the six permissions and opens Discord in a new tab.
  const invite = row(page, 'bot_access').getByRole('link', { name: /Invite the bot/ });
  await expect(invite).toHaveAttribute('href', INVITE);
  await expect(invite).toHaveAttribute('target', '_blank');
  await expect(invite).toHaveAttribute('rel', 'noopener');
  await expect(invite).toHaveAccessibleName('Invite the bot (opens Discord in a new tab)');

  // Refresh asks again and says what it found.
  await page.getByRole('button', { name: 'Refresh' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Checked again.' })).toHaveText(`Checked again. ${SETUP_LEFT} required steps left.`);
});

test('setup: the nav badge counts the required steps left', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page, '/');
  const nav = page.locator('.navrail').getByRole('navigation', { name: 'Sections' });
  const more = nav.getByRole('button', { name: `More, ${SETUP_LEFT} waiting`, exact: true });
  // The badge's hidden " waiting" loses its space to Svelte's trimming; the item's name keeps it (aria-label).
  await expect(more.locator('.navlist__badge')).toHaveText(new RegExp(`^${SETUP_LEFT}\\s*waiting$`));
  await more.click();
  const item = page.getByRole('menu', { name: 'More destinations' }).getByRole('menuitem', { name: `Setup ${SETUP_LEFT} waiting`, exact: true });
  await expect(item.locator('.navmore__badge')).toHaveText(new RegExp(`^${SETUP_LEFT}\\s*waiting$`));
  await item.click();
  await expect(page).toHaveURL(`${ADMIN}/setup`);
  await expect(nav.getByRole('button', { name: moreNamed('Setup') })).toHaveAccessibleName(`More, current page Setup, ${SETUP_LEFT} waiting`);
  await expect(page.locator('#main')).toBeFocused();
});

test('setup: the digest channel is edited in Config › Channels and Setup follows', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page, '/config?section=channels');
  const panel = page.locator('.settings__panel:not([hidden])');
  const card = panel.locator('form').filter({ has: page.getByRole('heading', { name: 'Digest channel' }) });
  await expect(card.locator('.digestchannel__source')).toHaveText('From KANADE_POST_CHANNEL_ID');
  const picker = card.getByRole('combobox', { name: 'Digest channel' });
  await expect(picker).toContainText('#boss-schedule');

  // Clearing it: one section, `null`, then the env seed is ignored.
  await choose(picker, { label: 'None (no digest)' });
  const cleared = page.waitForRequest((r) => r.url().endsWith('/api/admin/config') && r.method() === 'PATCH');
  await card.getByRole('button', { name: 'Save digest channel' }).click();
  expect((await cleared).postDataJSON()).toEqual({ posting: { channel_id: null } });
  await expect(toast(page, 'Digest channel cleared; no digest is posted.')).toBeVisible();
  await expect(card.locator('.digestchannel__source')).toHaveText('Saved here KANADE_POST_CHANNEL_ID is ignored');
  // The save re-reads Setup: the badge follows without a visit.
  const nav = page.locator('.navrail').getByRole('navigation', { name: 'Sections' });
  await expect(nav.getByRole('button', { name: moreNamed() })).toHaveAccessibleName(`More, ${SETUP_LEFT + 1} waiting`);

  // On Setup the step is Missing, with the Config section that fixes it.
  await nav.getByRole('button', { name: moreNamed() }).click();
  await page.getByRole('menuitem', { name: /^Setup/ }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${SETUP_LEFT + 1} required steps left`);
  await expect(state(page, 'digest_channel')).toHaveText('Missing');
  await expect(row(page, 'digest_channel')).toContainText('Pick the channel the weekly digest posts in.');
  await expect(row(page, 'digest_channel').locator('.setup-row__source')).toHaveText(/source:\sSaved in Config$/);
  await row(page, 'digest_channel').getByRole('link', { name: 'Config › Channels' }).click();
  await expect(page).toHaveURL(`${ADMIN}/config?section=channels`);
  await expect(panel.getByRole('heading', { name: 'Channels', exact: true })).toBeVisible();

  // Picking a channel again: saved toast, and Setup shows it.
  await choose(card.getByRole('combobox', { name: 'Digest channel' }), { label: '#fa-night' });
  const saved = page.waitForRequest((r) => r.url().endsWith('/api/admin/config') && r.method() === 'PATCH');
  await card.getByRole('button', { name: 'Save digest channel' }).click();
  expect((await saved).postDataJSON()).toEqual({ posting: { channel_id: '120000000000000004' } });
  await expect(toast(page, 'Digest channel saved (#fa-night).')).toBeVisible();
  await open(page);
  await expect(state(page, 'digest_channel')).toHaveText('Ready');
  await expect(row(page, 'digest_channel')).toContainText('The weekly digest posts in #fa-night.');
});

test('setup: a failed read shows the shared failed state, and Try again recovers', async ({ page }) => {
  let fail = true;
  await page.route(`${ADMIN}/api/admin/setup`, (route) => (fail ? route.fulfill({ status: 503, json: {} }) : route.fallback()));
  await open(page);
  const alert = page.getByRole('alert').filter({ hasText: 'Couldn’t load the setup checklist' });
  await expect(alert).toContainText('HTTP 503');
  // No answer, no Setup badge on More (the Inbox keeps its own).
  await expect(page.locator('.navrail .navmore .navlist__badge')).toHaveCount(0);
  fail = false;
  await alert.getByRole('button', { name: 'Try again' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${SETUP_LEFT} required steps left`);
});

test('setup: the phone frame stacks each row and keeps every action reachable', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await open(page);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${SETUP_LEFT} required steps left`);
  await settle(page);
  // The document never scrolls; only the list does.
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight - innerHeight)).toBeLessThanOrEqual(0);
  // Stacked: the action sits under the line, inside the frame.
  const invite = row(page, 'bot_access').getByRole('link', { name: /Invite the bot/ });
  const line = (await row(page, 'bot_access').locator('.setup-row__line').boundingBox())!;
  const action = (await invite.boundingBox())!;
  expect(action.y).toBeGreaterThanOrEqual(line.y + line.height - 1);
  expect(action.x + action.width).toBeLessThanOrEqual(390);
  expect(action.height).toBeGreaterThanOrEqual(44);
  // The last row scrolls into reach.
  await row(page, 'backups').scrollIntoViewIfNeeded();
  await expect(row(page, 'backups').locator('.setup-row__runbook')).toBeInViewport();
  // The drawer opens with More open on its page: Setup's own link is current and carries the badge.
  await page.getByRole('button', { name: 'Open the navigation' }).click();
  const drawer = page.getByRole('dialog', { name: 'Navigation' });
  const setup = drawer.getByRole('link', { name: /^Setup/ });
  await expect(setup).toHaveAttribute('aria-current', 'page');
  await expect(setup.locator('.navlist__badge')).toHaveText(new RegExp(`^${SETUP_LEFT}\\s*waiting$`));
});

// Review captures for the user: `KANADE_SETUP_CAPTURES=1` (writes PNGs only).
const CAPTURES = '/private/tmp/kanade-setup/captures';
test.describe('captures', () => {
  test.skip(process.env.KANADE_SETUP_CAPTURES !== '1', 'review captures are opt-in');

  for (const frame of [
    { name: 'desk', width: 1280, height: 800 },
    { name: 'phone', width: 390, height: 844 },
  ]) {
    for (const theme of ['light', 'dark']) {
      if (frame.name === 'phone' && theme === 'dark') continue;
      test(`setup page ${frame.name} ${theme}`, async ({ page }) => {
        mkdirSync(CAPTURES, { recursive: true });
        await useLook(page, 'marigold', theme);
        await page.setViewportSize(frame);
        await open(page);
        await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${SETUP_LEFT} required steps left`);
        await settle(page);
        await page.screenshot({ path: `${CAPTURES}/setup-${frame.name}-${theme}.png` });
      });

      test(`config digest channel ${frame.name} ${theme}`, async ({ page }) => {
        mkdirSync(CAPTURES, { recursive: true });
        await useLook(page, 'marigold', theme);
        await page.setViewportSize(frame);
        await open(page, '/config?section=channels');
        const card = page.locator('.settings__panel:not([hidden]) form').filter({ has: page.getByRole('heading', { name: 'Digest channel' }) });
        await expect(card.getByRole('combobox', { name: 'Digest channel' })).toContainText('#boss-schedule');
        await card.scrollIntoViewIfNeeded();
        await settle(page);
        await page.screenshot({ path: `${CAPTURES}/config-digest-${frame.name}-${theme}.png` });
      });
    }
  }
});
