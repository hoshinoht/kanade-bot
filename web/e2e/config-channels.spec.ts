import type { Page } from '@playwright/test';
import { ADMIN, expect, test, toggleOptions } from './support';

// Config → Channels: three lists, each saved on its own (an explicit list
// save); a list nobody saved follows its env seed.
const panel = (page: Page) => page.locator('.settings__panel:not([hidden])');
const card = (page: Page, title: string) => panel(page).locator('form').filter({ has: page.getByRole('heading', { name: title }) });

type Lists = {
  watching: { channel_ids: string[]; channel_ids_source: string; category_ids: string[]; category_ids_source: string };
  chatbot: { category_ids: string[]; category_ids_source: string };
};
async function lists(page: Page): Promise<Lists> {
  return (await (await page.request.get(`${ADMIN}/api/admin/config`)).json()) as Lists;
}
const patched = (page: Page) => page.waitForRequest((r) => r.url().endsWith('/api/admin/config') && r.method() === 'PATCH');

test('channels: each list saves alone and then overrides its env seed', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=channels&sw=off`);
  await expect(panel(page).getByRole('heading', { name: 'Channels', exact: true })).toBeVisible();
  await expect(panel(page)).toContainText('The environment variables are only the starting values');
  const categories = card(page, 'Watched categories');
  await expect(categories.locator('.idlist__source')).toHaveText('From KANADE_WATCH_CATEGORY_IDS');

  // Typed ids become chips; Save sends only this list.
  const add = categories.getByRole('textbox', { name: 'Add categories by id' });
  await add.fill('21, 22');
  await add.press('Enter');
  await expect(categories.getByRole('list', { name: 'Watched categories' }).getByRole('listitem')).toHaveText([/^21/, /^22/]);
  const request = patched(page);
  const saveButton = categories.getByRole('button', { name: 'Save watched categories' });
  await saveButton.focus();
  await page.keyboard.press('Enter');
  expect((await request).postDataJSON()).toEqual({ watching: { category_ids: ['21', '22'] } });
  await expect(page.getByText('Watched categories saved (2 categories).')).toBeVisible();
  // The card stays mounted: focus stays on Save, now inert until the next edit.
  await expect(saveButton).toBeFocused();
  await expect(saveButton).toHaveAttribute('aria-disabled', 'true');
  await expect(page.locator('.settings__dirty')).toHaveCount(0);
  await expect(card(page, 'Watched categories').locator('.idlist__source')).toHaveText('Saved here KANADE_WATCH_CATEGORY_IDS is ignored');
  const after = await lists(page);
  expect(after.watching.category_ids).toEqual(['21', '22']);
  expect(after.watching.category_ids_source).toBe('saved');
  expect(after.watching.channel_ids_source).toBe('env');
  expect(after.chatbot.category_ids_source).toBe('env');

  // Watched channels come from the guild's channel picker.
  const channels = card(page, 'Watched channels');
  const before = after.watching.channel_ids;
  expect(before).toContain('kalos-four');
  await toggleOptions(channels.getByRole('combobox', { name: 'Watched channels to keep' }), ['#kalos-four']);
  const save = patched(page);
  await channels.getByRole('button', { name: 'Save watched channels' }).click();
  expect((await save).postDataJSON()).toEqual({ watching: { channel_ids: before.filter((id) => id !== 'kalos-four') } });
  await expect.poll(async () => (await lists(page)).watching.channel_ids_source).toBe('saved');
});

test('channels: a refused list keeps the draft and shows the error inline', async ({ page }) => {
  await page.goto(`${ADMIN}/config?section=channels&sw=off`);
  const chat = card(page, 'Chat categories');
  const before = await lists(page);
  const add = chat.getByRole('textbox', { name: 'Add categories by id' });
  // Not a number: refused before anything is sent.
  await add.fill('general');
  await add.press('Enter');
  await expect(chat.getByRole('alert')).toHaveText('Ids are whole numbers, separated by commas or spaces.');
  // A number that is not a Discord id: the server refuses it.
  await add.fill('0');
  await chat.getByRole('button', { name: 'Save chat categories' }).click();
  await expect(chat.getByRole('alert')).toHaveText(/canonical positive Discord ids/);
  await expect(chat.getByRole('list', { name: 'Chat categories' }).getByRole('listitem').last()).toHaveText(/^0/);
  await expect(page.locator('.settings__dirty')).toHaveCount(1);
  expect(await lists(page)).toEqual(before);
  await chat.getByRole('button', { name: 'Discard' }).click();
  await expect(chat.getByRole('alert')).toHaveCount(0);
  await expect(page.locator('.settings__dirty')).toHaveCount(0);
});
