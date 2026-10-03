import type { Locator, Page } from '@playwright/test';
import { ADMIN, PUBLIC, csrf, expect, test } from './support';

type Profile = { key: string; name: string; public: boolean; voice: string; prompt_summary: string };
type ConfigResponse = { persona: { profiles: Profile[] } };
type VisibilityPatch = { persona: { visibility: { key: string; public: boolean }[] } };

/** A long catalog: the mock's four profiles plus 20 numbered ones. */
async function manyProfiles(page: Page, patches?: VisibilityPatch[]) {
  let latest: ConfigResponse | null = null;
  await page.route(/\/api\/admin\/config$/, async (route) => {
    if (route.request().method() === 'PATCH' && patches) {
      const body = route.request().postDataJSON() as VisibilityPatch;
      patches.push(body);
      if (!latest) throw new Error('A profile visibility patch arrived before config loaded.');
      for (const delta of body.persona.visibility) {
        const profile = latest.persona.profiles.find((p) => p.key === delta.key);
        if (!profile) throw new Error(`Unexpected profile key in delta: ${delta.key}`);
        profile.public = delta.public;
      }
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(latest) });
      return;
    }
    if (route.request().method() !== 'GET') return route.continue();
    const res = await route.fetch();
    const body = (await res.json()) as ConfigResponse;
    const keys = new Set(body.persona.profiles.map((p) => p.key));
    for (let i = 1; i <= 20; i++) {
      const key = `extra-${i}`;
      if (keys.has(key)) continue;
      body.persona.profiles.push({ key, name: `Extra ${String(i).padStart(2, '0')}`, public: i % 2 === 0, voice: 'Numbered', prompt_summary: `Filler prompt number ${i}.` });
    }
    latest = structuredClone(body);
    await route.fulfill({ response: res, json: body });
  });
}

async function recordPatches(page: Page, patches: unknown[]) {
  await page.route(/\/api\/admin\/config$/, async (route) => {
    if (route.request().method() === 'PATCH') patches.push(route.request().postDataJSON());
    await route.continue();
  });
}

async function persona(page: Page) {
  await page.goto(`${ADMIN}/config?sw=off`);
  await page.getByRole('tab', { name: /^Persona/ }).click();
  return page.getByRole('table', { name: 'Reply profiles' });
}

/** Persona's second pill tab holds the role overrides. */
async function overrides(page: Page) {
  await persona(page);
  await page.getByRole('tab', { name: /^Role overrides/ }).click();
}

async function roleEditor(page: Page) {
  await overrides(page);
  await expect(page.getByRole('status').filter({ hasText: 'Current guild roles are loaded.' })).toBeVisible();
  return page.getByRole('list', { name: 'Role assignments in precedence order' });
}

function roleRows(list: Locator) {
  return list.locator('.role-profile__row');
}

function profileRow(table: Locator, name: string) {
  return table.getByRole('rowheader').filter({ hasText: name }).locator('xpath=..');
}

function profileState(row: Locator, state: 'public' | 'private') {
  return row.locator('.profile__state-narrow:visible, .profile__state-wide:visible').filter({ hasText: state });
}

test('reply profiles: search, visibility and pages, kept across Reload', async ({ page }) => {
  await manyProfiles(page);
  const table = await persona(page);
  // Ten profiles a page, after the synthesized default voice on page 1.
  await expect(table.locator('tbody tr')).toHaveCount(11);
  await expect(table.getByRole('rowheader').first()).toHaveText('Default voice');
  await expect(page.getByText('1–10 of 24 profiles')).toBeVisible();
  await page.getByRole('button', { name: 'Next →' }).click();
  await expect(page.getByText('11–20 of 24 profiles')).toBeVisible();

  const find = page.getByRole('searchbox', { name: 'Search', exact: true });
  await find.fill('numbered');
  await expect(page.getByText('1–10 of 20 profiles')).toBeVisible();
  await page.getByRole('group', { name: 'Visibility' }).getByRole('button', { name: /^Private/ }).click();
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
  await expect(page.getByRole('group', { name: 'Visibility' }).getByRole('button', { name: /^Private/ })).toHaveAttribute('aria-pressed', 'true');

  await find.fill('nothing like this');
  await expect(table).toContainText('No profile matches.');
});

test('reply profiles: a one-line prompt preview opens the whole prompt', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  const table = await persona(page);
  const kanade = profileRow(table, 'Kanade');
  await expect(kanade.getByRole('button', { name: 'Make private' })).toBeVisible();
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

test('reply profiles: single Publish and Make private save and reload accurately', async ({ page }) => {
  const patches: unknown[] = [];
  await recordPatches(page, patches);
  const table = await persona(page);
  const kanade = profileRow(table, 'Kanade');

  await kanade.getByRole('button', { name: 'Make private' }).click();
  let dialog = page.getByRole('dialog', { name: 'Make 1 profile private?' });
  await expect(dialog).toContainText('Members will no longer be able to choose these reply styles');
  await expect(dialog.locator('li')).toHaveText(['Kanade']);
  await dialog.getByRole('button', { name: 'Make 1 profile private' }).click();
  await expect(profileState(kanade, 'private')).toBeVisible();
  await expect(kanade.getByRole('button', { name: 'Publish' })).toBeVisible();
  expect(patches).toEqual([{ persona: { visibility: [{ key: 'kanade', public: false }] } }]);

  await page.getByRole('button', { name: 'Reload profiles' }).click();
  await expect(page.getByText(/Reloaded 4 reply profiles/)).toBeVisible();
  await expect(profileState(kanade, 'private')).toBeVisible();

  await kanade.getByRole('button', { name: 'Publish' }).click();
  dialog = page.getByRole('dialog', { name: 'Publish 1 profile?' });
  await expect(dialog.locator('li')).toHaveText(['Kanade']);
  await dialog.getByRole('button', { name: 'Publish 1 profile' }).click();
  await expect(profileState(kanade, 'public')).toBeVisible();
  expect(patches).toEqual([
    { persona: { visibility: [{ key: 'kanade', public: false }] } },
    { persona: { visibility: [{ key: 'kanade', public: true }] } },
  ]);
});

test('reply profiles: batch actions send only selected visibility deltas', async ({ page }) => {
  const patches: unknown[] = [];
  await recordPatches(page, patches);
  const table = await persona(page);
  const names = ['Default', 'Kanade'];
  for (const name of names) await profileRow(table, name).getByRole('checkbox', { name: `Select ${name}` }).check();
  await expect(page.getByText('2 profiles selected.', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Make private selected' }).click();
  let dialog = page.getByRole('dialog', { name: 'Make 2 profiles private?' });
  await expect(dialog.locator('li')).toHaveText(names);
  await expect(dialog).toContainText('Members will no longer be able to choose these reply styles');
  await dialog.getByRole('button', { name: 'Make 2 profiles private' }).click();
  for (const name of names) await expect(profileState(profileRow(table, name), 'private')).toBeVisible();
  await expect(profileState(profileRow(table, 'Terse'), 'public')).toBeVisible();
  await expect(profileState(profileRow(table, 'Sparkly'), 'private')).toBeVisible();
  await expect(page.getByText('0 profiles selected. Made 2 reply profiles private.', { exact: true })).toBeVisible();

  for (const name of names) await profileRow(table, name).getByRole('checkbox', { name: `Select ${name}` }).check();
  await page.getByRole('button', { name: 'Publish selected' }).click();
  dialog = page.getByRole('dialog', { name: 'Publish 2 profiles?' });
  await expect(dialog.locator('li')).toHaveText(names);
  await dialog.getByRole('button', { name: 'Publish 2 profiles' }).click();
  for (const name of names) await expect(profileState(profileRow(table, name), 'public')).toBeVisible();
  expect(patches).toEqual([
    { persona: { visibility: [{ key: 'default', public: false }, { key: 'kanade', public: false }] } },
    { persona: { visibility: [{ key: 'default', public: true }, { key: 'kanade', public: true }] } },
  ]);
});

test('reply profiles: selection survives pages and filters without republishing hidden rows', async ({ page }) => {
  const patches: VisibilityPatch[] = [];
  await manyProfiles(page, patches);
  const table = await persona(page);
  await profileRow(table, 'Extra 01').getByRole('checkbox', { name: 'Select Extra 01' }).check();
  await page.getByRole('button', { name: 'Next →' }).click();
  await profileRow(table, 'Extra 12').getByRole('checkbox', { name: 'Select Extra 12' }).check();
  await expect(page.getByText('2 profiles selected.', { exact: true })).toBeVisible();

  await page.getByRole('searchbox', { name: 'Search', exact: true }).fill('numbered');
  await page.getByRole('group', { name: 'Visibility' }).getByRole('button', { name: /^Private/ }).click();
  await expect(page.getByText('2 profiles selected.', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Publish selected' }).click();
  let dialog = page.getByRole('dialog', { name: 'Publish 1 profile?' });
  await expect(dialog.locator('li')).toHaveText(['Extra 01']);
  await expect(dialog).toContainText('1 selected profile is already public and will stay unchanged.');
  await dialog.getByRole('button', { name: 'Publish 1 profile' }).click();
  await expect(page.getByRole('searchbox', { name: 'Search', exact: true })).toHaveValue('numbered');
  await expect(page.getByRole('group', { name: 'Visibility' }).getByRole('button', { name: /^Private/ })).toHaveAttribute('aria-pressed', 'true');
  await expect(profileRow(table, 'Extra 01')).toHaveCount(0);
  await expect(page.getByText('1 profile selected. Published 1 reply profile.', { exact: true })).toBeVisible();

  await page.getByRole('button', { name: 'Make private selected' }).click();
  dialog = page.getByRole('dialog', { name: 'Make 1 profile private?' });
  await expect(dialog.locator('li')).toHaveText(['Extra 12']);
  await dialog.getByRole('button', { name: 'Make 1 profile private' }).click();
  await expect(profileState(profileRow(table, 'Extra 12'), 'private')).toBeVisible();
  expect(patches).toEqual([
    { persona: { visibility: [{ key: 'extra-1', public: true }] } },
    { persona: { visibility: [{ key: 'extra-12', public: false }] } },
  ]);
});

test('reply profiles: visibility save stays busy, explains refusal and can recover', async ({ page }) => {
  let firstPatch = true;
  let release!: () => void;
  let markStarted!: () => void;
  const started = new Promise<void>((resolve) => (markStarted = resolve));
  const gate = new Promise<void>((resolve) => (release = resolve));
  await page.route(/\/api\/admin\/config$/, async (route) => {
    if (route.request().method() === 'PATCH' && firstPatch) {
      firstPatch = false;
      markStarted();
      await gate;
      await route.fulfill({
        status: 422,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'read_only', message: 'Visibility update refused; try again.' }),
      });
      return;
    }
    await route.continue();
  });
  const table = await persona(page);
  const kanade = profileRow(table, 'Kanade');
  await kanade.getByRole('button', { name: 'Make private' }).click();
  const dialog = page.getByRole('dialog', { name: 'Make 1 profile private?' });
  const confirm = dialog.getByRole('button', { name: /Make 1 profile private|Making private…/ });
  await confirm.click();
  await started;
  await expect(confirm).toHaveAttribute('aria-disabled', 'true');
  await expect(dialog.getByText('Making these reply profiles private…', { exact: true })).toBeVisible();
  const close = dialog.getByRole('button', { name: 'Close' });
  await expect(close).toBeDisabled();
  await expect(confirm).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeVisible();
  await expect(confirm).toBeFocused();
  const closeBounds = await close.boundingBox();
  if (!closeBounds) throw new Error('The modal close control has no rendered box.');
  await page.mouse.click(closeBounds.x + closeBounds.width / 2, closeBounds.y + closeBounds.height / 2);
  await expect(dialog).toBeVisible();
  await expect(confirm).toBeFocused();

  release();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('alert')).toHaveText('Visibility update refused; try again.');
  await expect(profileState(kanade, 'public')).toBeVisible();
  await expect(confirm).toBeFocused();

  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).not.toBeVisible();
  await expect(kanade.getByRole('button', { name: 'Make private' })).toBeFocused();
  await kanade.getByRole('button', { name: 'Make private' }).click();
  const retryDialog = page.getByRole('dialog', { name: 'Make 1 profile private?' });
  await retryDialog.getByRole('button', { name: 'Make 1 profile private' }).click();
  await expect(profileState(kanade, 'private')).toBeVisible();
  await expect(page.getByText('0 profiles selected. Made 1 reply profile private.', { exact: true })).toBeVisible();
});

test('role profiles: add, change, reorder and remove save one ordered ID-backed patch', async ({ page }) => {
  const list = await roleEditor(page);
  const rows = roleRows(list);
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0).locator('.role-profile__identity strong')).toHaveText('@staff');
  await expect(rows.nth(0)).not.toContainText('300001');

  const config = (await (await page.request.get(`${ADMIN}/api/admin/config`)).json()) as {
    persona: { role_profiles_digest: string };
  };
  const add = page.locator('.role-profile__add');
  await add.getByRole('searchbox', { name: 'Search current guild roles' }).fill('boss');
  const picker = add.getByRole('combobox', { name: 'New assignment role' });
  await expect(picker.locator('option')).toHaveText(['Choose a role…', '@bossers']);
  await picker.selectOption('300003');
  await add.getByRole('combobox', { name: 'Reply profile' }).selectOption('sparkly');
  await add.getByRole('button', { name: 'Add assignment' }).click();
  await expect(rows).toHaveCount(3);

  await rows.nth(0).getByRole('combobox', { name: 'Reply profile' }).selectOption('kanade');
  await rows.nth(2).getByRole('button', { name: 'Move up' }).click();
  await rows.nth(1).getByRole('button', { name: 'Move up' }).click();
  await expect(rows.nth(0).locator('.role-profile__identity strong')).toHaveText('@bossers');
  await expect(rows.nth(1).locator('.role-profile__identity strong')).toHaveText('@staff');
  await rows.nth(2).getByRole('button', { name: 'Remove @newbies assignment' }).click();
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(1).getByRole('button', { name: 'Remove @staff assignment' })).toBeFocused();

  const patch = page.waitForRequest((request) => request.method() === 'PATCH' && request.url().endsWith('/api/admin/config'));
  await page.getByRole('button', { name: 'Save role overrides' }).click();
  const body = (await patch).postDataJSON() as {
    persona: { role_profiles: { role_id: string; profile: string }[]; role_profiles_digest: string };
  };
  expect(body).toEqual({
    persona: {
      role_profiles: [
        { role_id: '300003', profile: 'sparkly' },
        { role_id: '300001', profile: 'kanade' },
      ],
      role_profiles_digest: config.persona.role_profiles_digest,
    },
  });
  await expect(page.getByRole('status').filter({ hasText: 'Role assignments saved.' })).toBeVisible();
  await expect(page.getByRole('searchbox', { name: 'Search current guild roles' })).toBeFocused();
  expect(body.persona.role_profiles.every((assignment) => !('role_name' in assignment))).toBe(true);
});

test('role assignment order has keyboard controls and a searchable native role picker', async ({ page }) => {
  const list = await roleEditor(page);
  const rows = roleRows(list);
  const search = page.getByRole('searchbox', { name: 'Search current guild roles' });
  await search.fill('boss');
  const picker = page.getByRole('combobox', { name: 'New assignment role' });
  await expect(picker.locator('option')).toHaveText(['Choose a role…', '@bossers']);
  await search.fill('');

  const moveDown = rows.nth(0).getByRole('button', { name: 'Move down' });
  await moveDown.focus();
  await page.keyboard.press('Enter');
  await expect(rows.nth(0).locator('.role-profile__identity strong')).toHaveText('@newbies');
  await expect(page.getByRole('status').filter({ hasText: 'Moved @staff to position 2 of 2.' })).toBeVisible();
  const discard = page.getByRole('button', { name: 'Discard', exact: true });
  await discard.focus();
  await page.keyboard.press('Enter');
  await expect(rows.nth(0).locator('.role-profile__identity strong')).toHaveText('@staff');
});

test('role profiles: a failed role list is distinct from an empty directory and still permits reorder/remove', async ({ page }) => {
  await page.route(`${ADMIN}/api/admin/roles`, (route) =>
    route.fulfill({ status: 503, json: { error: 'unavailable', message: 'The guild role directory is unavailable.' } }),
  );
  const list = page.getByRole('list', { name: 'Role assignments in precedence order' });
  await overrides(page);
  await expect(page.getByRole('alert').filter({ hasText: "Couldn't load current guild roles" })).toContainText('temporarily unknown');
  const rows = roleRows(list);
  await expect(rows.nth(0).locator('.role-profile__identity strong')).toHaveText('Role status unknown');
  await expect(rows.nth(0)).not.toContainText('@staff');
  await expect(page.getByRole('combobox', { name: 'New assignment role' })).toBeDisabled();
  await expect(rows.nth(0).getByRole('combobox', { name: 'Reply profile' })).toBeDisabled();
  await expect(rows.nth(0).getByRole('button', { name: 'Move down' })).toBeEnabled();
  await expect(rows.nth(0).getByRole('button', { name: /Remove Role status unknown assignment/ })).toBeEnabled();

  await rows.nth(0).getByRole('button', { name: 'Move down' }).click();
  await page.getByRole('button', { name: 'Save role overrides' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Role assignments saved.' })).toBeVisible();
});

test('role profiles: an empty current directory shows unavailable roles, not stale names', async ({ page }) => {
  await page.route(`${ADMIN}/api/admin/roles`, (route) => route.fulfill({ status: 200, json: [] }));
  const list = page.getByRole('list', { name: 'Role assignments in precedence order' });
  await overrides(page);
  await expect(page.getByRole('status').filter({ hasText: 'No current guild roles are available.' })).toBeVisible();
  const rows = roleRows(list);
  await expect(rows.nth(0).locator('.role-profile__identity strong')).toHaveText('Unavailable role');
  await expect(rows.nth(0)).not.toContainText('@staff');
  await expect(page.getByRole('combobox', { name: 'New assignment role' })).toBeDisabled();
  await expect(rows.nth(0).getByRole('combobox', { name: 'Reply profile' })).toBeDisabled();
  await expect(rows.nth(0).getByRole('button', { name: 'Move down' })).toBeEnabled();
  await expect(rows.nth(0).getByRole('button', { name: /Remove Unavailable role assignment/ })).toBeEnabled();
});

test('role profiles: a second admin conflict preserves the draft until explicit reload', async ({ page }) => {
  const list = await roleEditor(page);
  const rows = roleRows(list);
  const config = (await (await page.request.get(`${ADMIN}/api/admin/config`)).json()) as {
    persona: { role_profiles: { role_id: string; profile: string }[]; role_profiles_digest: string };
  };
  const competitor = await page.request.patch(`${ADMIN}/api/admin/config`, {
    headers: await csrf(page.request),
    data: {
      persona: {
        role_profiles: [
          { role_id: '300002', profile: 'default' },
          { role_id: '300001', profile: 'sparkly' },
        ],
        role_profiles_digest: config.persona.role_profiles_digest,
      },
    },
  });
  expect(competitor.status()).toBe(200);

  await rows.nth(0).getByRole('combobox', { name: 'Reply profile' }).selectOption('kanade');
  await page.getByRole('button', { name: 'Save role overrides' }).click();
  await expect(page.getByRole('alert').filter({ hasText: 'Role assignments changed since they were loaded' })).toContainText('Role assignments changed since they were loaded');
  await expect(page.getByRole('status').filter({ hasText: 'Your draft is still here.' })).toBeVisible();
  await expect(rows.nth(0).getByRole('combobox', { name: 'Reply profile' })).toHaveValue('kanade');
  await expect(page.getByRole('button', { name: 'Save role overrides' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Reload latest assignments' })).toBeFocused();

  await page.getByRole('button', { name: 'Reload latest assignments' }).click();
  await expect(rows.nth(0).locator('.role-profile__identity strong')).toHaveText('@newbies');
  await expect(rows.nth(1).getByRole('combobox', { name: 'Reply profile' })).toHaveValue('sparkly');
  await expect(page.getByRole('status').filter({ hasText: 'Latest saved assignments loaded.' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save role overrides' })).toBeDisabled();
  await expect(page.getByRole('searchbox', { name: 'Search current guild roles' })).toBeFocused();
});

test('reply profile controls keep the fixed config shell at desktop and phone widths', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await overrides(page);
  await expect(page.getByRole('status').filter({ hasText: 'Current guild roles are loaded.' })).toBeVisible();
  const desktop = await page.evaluate(() => ({
    clientHeight: document.documentElement.clientHeight,
    scrollHeight: document.documentElement.scrollHeight,
  }));
  expect(desktop.scrollHeight).toBeLessThanOrEqual(desktop.clientHeight + 1);
  await page.locator('.role-profile').evaluate((el) => {
    const panel = el.closest<HTMLElement>('.settings__scroll')!;
    panel.scrollTop += el.getBoundingClientRect().top - panel.getBoundingClientRect().top;
  });
  await page.screenshot({ path: testInfo.outputPath('profiles-desktop.png') });

  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('tab', { name: 'Active persona & profiles' }).click();
  await expect(page.getByRole('table', { name: 'Reply profiles' })).toBeVisible();
  await page.getByRole('tab', { name: /^Role overrides/ }).click();
  await page.locator('.role-profile__add').evaluate((el) => {
    const panel = el.closest<HTMLElement>('.settings__scroll')!;
    panel.scrollTop += el.getBoundingClientRect().top - panel.getBoundingClientRect().top - 8;
  });
  const phoneForm = page.locator('.role-profile__add');
  const phoneFormControls = await phoneForm.locator('input, select, button').evaluateAll((controls) =>
    controls.map((control) => ({
      height: control.getBoundingClientRect().height,
      visible: control.getBoundingClientRect().bottom <= control.closest('.settings__scroll')!.getBoundingClientRect().bottom,
    })),
  );
  expect(phoneFormControls.length).toBe(4);
  expect(phoneFormControls.every((control) => control.visible)).toBe(true);
  expect(phoneFormControls[3]!.height).toBeGreaterThanOrEqual(44);
  const phone = await page.evaluate(() => ({
    clientHeight: document.documentElement.clientHeight,
    scrollHeight: document.documentElement.scrollHeight,
  }));
  expect(phone.scrollHeight).toBeLessThanOrEqual(phone.clientHeight + 1);
  await page.screenshot({ path: testInfo.outputPath('profiles-phone.png') });
});

test('public app does not gain reply-profile visibility controls', async ({ page }) => {
  await page.goto(`${PUBLIC}/`);
  await expect(page.getByText(/reply profiles/i)).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Publish|Make private/ })).toHaveCount(0);
});
