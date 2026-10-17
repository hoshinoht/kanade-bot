import type { Page } from '@playwright/test';
import { PUBLIC, expect, setPortal, signInPublic, test } from './support';

// `/privacy` (boards P_Privacy, P_PhonePrivacy): the character-data privacy
// page the `/character link` notice links. Public: signed out, signed in
// or closed it shows the same words, signed out it reads no member route,
// and it carries the attribution and the notice version. Every test ends
// with zero CSP/TT reports (`csp`).

test.describe.configure({ mode: 'parallel' });

const ITEMS = ['What is fetched', 'Why', 'How long', 'Who sees it', 'Your controls', 'Estimates', 'As is', 'Contact'];

/** Every member API path the page asked for (status and session aside). */
function memberReads(page: Page): string[] {
  const seen: string[] = [];
  page.on('request', (request) => {
    const { pathname } = new URL(request.url());
    if (pathname.startsWith('/api/public/') && !['/api/public/status', '/api/public/session'].includes(pathname)) seen.push(pathname);
  });
  return seen;
}

async function expectPrivacy(page: Page) {
  await expect(page.getByRole('heading', { level: 1, name: 'Character data and privacy' })).toBeVisible();
  // `data-fid` is stripped from built bundles: find the list by its markup.
  const list = page.locator('dl.privacy__list');
  await expect(list.locator('dt')).toHaveText(ITEMS);
  await expect(list).toContainText('Kanade is not affiliated with or endorsed by NEXON.');
  await expect(list).toContainText('The guild admins on Discord.');
  // Estimates are kept up to 30 days in the character-data file (estimate cache), so /character data covers only NEXON's data.
  const dd = (term: string) => list.locator('dt', { hasText: term }).locator('xpath=following-sibling::dd[1]');
  await expect(dd('Your controls')).toContainText('/character data shows everything kept about you from NEXON. /character unlink deletes it at once');
  await expect(dd('Estimates')).toContainText(
    'the answer is kept in Kanade’s character-data file for up to 30 days, never in backups or exports, and refreshed automatically for party members of upcoming runs as often as the admins set (every 7 days unless changed). It is deleted when the character is retired or removed from the roster, or when you leave the server.',
  );
  await expect(dd('Estimates')).not.toContainText('in memory');
  // D14: what reminder headers may send to the chat model, and never a name.
  await expect(dd('Estimates')).toContainText(
    'When an admin turns it on, Kanade’s chat model words reminder headers from the party’s overall estimate per boss and the top two carriers’ shares; Kanade adds character names afterwards, so they are never sent. If you allow teasing, your share of the estimate and any running-joke note an admin wrote may be sent, never your name.',
  );
  await expect(dd('Who sees it')).toContainText('never sent to chat models');
  await expect(page.getByRole('link', { name: 'developer privacy policy' })).toHaveAttribute('href', 'https://m.nexon.com/terms/1000');
  await expect(page.getByText('Data based on NEXON Open API', { exact: true })).toBeVisible();
  await expect(page.getByText('Notice 2 · 15 Oct 2026')).toBeVisible();
  await expect(page).toHaveTitle('Privacy · Kanade');
}

test('signed out, the privacy page shows and reads no member data', async ({ page }) => {
  const reads = memberReads(page);
  await page.goto(`${PUBLIC}/privacy?sw=off`);
  await expectPrivacy(page);
  expect(reads).toEqual([]);
});

test('signed in on a phone it is the same page', async ({ page }) => {
  await signInPublic(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${PUBLIC}/privacy?sw=off`);
  await expectPrivacy(page);
});

test('closed, the privacy page still shows', async ({ page, request }) => {
  await setPortal(request, false);
  await page.goto(`${PUBLIC}/privacy?sw=off`);
  await expectPrivacy(page);
});
