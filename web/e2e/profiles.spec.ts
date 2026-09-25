import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

type Profile = { key: string; name: string; public: boolean; voice: string; prompt_summary: string };

/** A long catalog: the mock's four profiles plus 20 numbered ones. */
async function manyProfiles(page: Page) {
  await page.route(/\/api\/admin\/config$/, async (route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const res = await route.fetch();
    const body = (await res.json()) as { persona: { profiles: Profile[] } };
    for (let i = 1; i <= 20; i++)
      body.persona.profiles.push({ key: `extra-${i}`, name: `Extra ${String(i).padStart(2, '0')}`, public: i % 2 === 0, voice: 'Numbered', prompt_summary: `Filler prompt number ${i}.` });
    await route.fulfill({ response: res, json: body });
  });
}

async function persona(page: Page) {
  await page.goto(`${ADMIN}/config?sw=off`);
  await page.getByRole('tab', { name: 'Persona' }).click();
  return page.getByRole('table', { name: 'Reply profiles' });
}

test('reply profiles: search, visibility and pages, kept across Reload', async ({ page }) => {
  await manyProfiles(page);
  const table = await persona(page);
  await expect(table.locator('tbody tr')).toHaveCount(10);
  await expect(page.getByText('1–10 of 24 profiles')).toBeVisible();
  await page.getByRole('button', { name: 'Next →' }).click();
  await expect(page.getByText('11–20 of 24 profiles')).toBeVisible();

  const find = page.getByRole('searchbox', { name: 'Search' });
  await find.fill('numbered');
  await expect(page.getByText('1–10 of 20 profiles')).toBeVisible();
  await page.getByRole('combobox', { name: 'Visibility' }).selectOption('private');
  // One page: the pager steps aside.
  await expect(table.locator('tbody tr')).toHaveCount(10);
  await expect(page.getByRole('button', { name: 'Next →' })).toHaveCount(0);
  for (const row of await table.locator('tbody tr').all()) await expect(row).toContainText('private');
  // Prompt text is searched too.
  await find.fill('number 7.');
  await expect(table.locator('tbody tr')).toHaveCount(1);
  await expect(table).toContainText('Extra 07');

  await page.getByRole('button', { name: 'Reload profiles' }).click();
  await expect(page.getByText(/Reloaded 4 reply profiles/)).toBeVisible();
  await expect(find).toHaveValue('number 7.');
  await expect(page.getByRole('combobox', { name: 'Visibility' })).toHaveValue('private');

  await find.fill('nothing like this');
  await expect(table).toContainText('No profile matches.');
});

test('reply profiles: a one-line prompt preview opens the whole prompt', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  const table = await persona(page);
  await expect(table.getByRole('button', { name: /Make (public|private)/ })).toHaveCount(0);
  const kanade = table.getByRole('row', { name: /^Kanade/ });
  const open = kanade.getByRole('button', { name: /read Kanade's whole prompt/ });
  const shown = (await open.evaluate((b) => b.childNodes[0]!.textContent ?? '')).trim();
  expect(shown.length).toBeLessThanOrEqual(61);
  await open.click();
  const dialog = page.getByRole('dialog', { name: 'Kanade: prompt' });
  await expect(dialog.locator('pre')).toContainText('keeps every schedule fact exact');
  await expect(dialog.locator('pre')).not.toContainText('**');
  await dialog.getByRole('button', { name: 'Copy' }).click();
  await expect(page.getByText('Prompt copied.')).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('schedule fact');
  await page.keyboard.press('Escape');
  await expect(open).toBeFocused();
});
