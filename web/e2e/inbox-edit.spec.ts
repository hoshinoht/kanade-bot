import type { Page } from '@playwright/test';
import { ADMIN, expect, setClock, test } from './support';

// Inbox request editing (boards docs/research/2026-10-11-drafts-mockups B_InboxEdit*)
// over the seed's `p-kalos-fixed` (Tsubame: XKalos Fri 21:30 → Sat 22:30, + Yuzu).

const toast = (page: Page, text: string | RegExp) => page.locator('.toast:not(.is-leaving)').filter({ hasText: text });

/** Tsubame's request in the Inbox list (Asahi's own `req-kalos-weekly` is a weekly timing change too). */
const kalosOption = (page: Page) => page.getByRole('option', { name: /^Weekly timing change — XKalos Tsubame/ });

/**
 * Tsubame's request, on Fri 25 12:00 guild time: XKalos (Fri 22:00, amended) has not
 * happened yet, so approving offers Keep or Update for it (an ended run is never a choice).
 */
async function kalosRequest(page: Page) {
  await setClock('2026-09-25T04:00:00Z');
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
  await expect(kalosOption(page)).toContainText('editing');
  await page.getByRole('radio', { name: 'Keep as it is' }).check();
  await page.getByRole('button', { name: 'Approve edited' }).click();
  await expect(toast(page, /^Approved with changes/)).toBeVisible();
  const fixed = await (await page.request.get(`${ADMIN}/api/admin/fixed`)).json();
  const kalos = (Array.isArray(fixed) ? fixed : fixed.rows).find((f: { id: string }) => f.id === 'f-kalos');
  expect(JSON.stringify(kalos)).toContain('22:00');
});

test('inbox: an edited approval refused after the request changed reloads read-only with Edit again', async ({ page }) => {
  await kalosRequest(page);
  await page.getByRole('button', { name: 'Edit before approving' }).click();
  await page.getByRole('button', { name: 'Drop change 2: the party change' }).click();
  // Another admin edits it to Sunday meanwhile (requesters never edit on the real server).
  const changed = await page.request.post(`${ADMIN}/__e2e/inbox/kalos-v4`);
  expect(changed.status(), await changed.text()).toBe(200);
  const body: unknown = await changed.json();
  const version = String(body !== null && typeof body === 'object' && 'version' in body ? body.version : undefined);
  expect(version, 'the request version after the edit').toMatch(/^\d+$/);
  await page.getByRole('radio', { name: 'Keep as it is' }).check();
  await page.getByRole('button', { name: 'Approve edited' }).click();
  const refused = toast(page, 'Couldn’t approve your edit.');
  await expect(refused).toContainText(`This request was changed meanwhile (now version ${version}), so your edits were cleared.`);
  // Persistent (an error), the editor closed, the request at its new version and flagged.
  await expect(page.getByText('Editing · not saved')).toHaveCount(0);
  await expect(page.getByText(`changed in version ${version}`)).toBeVisible();
  await expect(kalosOption(page)).toContainText(`updated · version ${version}`);
  await expect(page.getByRole('button', { name: 'Approve', exact: true })).toBeVisible();
  await refused.getByRole('button', { name: 'Edit again' }).click();
  await expect(page.getByText('Editing · not saved')).toBeVisible();
  await expect(page.getByRole('radiogroup', { name: 'Day for change 1' }).getByRole('radio', { name: 'Sunday (Tsubame asked)' })).toBeChecked();
  await page.getByRole('radio', { name: 'Keep as it is' }).check();
  await page.getByRole('button', { name: 'Approve edited' }).click();
  await expect(toast(page, /^Approved/)).toBeVisible();
});
