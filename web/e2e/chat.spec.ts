import { ADMIN, expect, test } from './support';

// Chat as one list-detail window (B_Chat, gate G5): the list beside the open
// turn on wide screens, one at a time on phones; only the list and the
// turn's tab panel scroll.

test('chat list-detail: the first turn opens beside the list; arrows move the open turn', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/chat?sw=off`);
  const list = page.getByRole('listbox', { name: /Chatbot interactions/ });
  const options = list.getByRole('option');
  await expect(options.first()).toHaveAttribute('aria-selected', 'true');
  const first = (await options.first().getAttribute('data-item'))!;
  const second = (await options.nth(1).getAttribute('data-item'))!;
  const detail = page.locator('.chat__detail');
  await expect(detail.getByRole('button', { name: 'Copy transcript' })).toBeVisible();
  await expect(page).toHaveURL(`${ADMIN}/chat?sw=off`);

  // Selection follows the arrows; the URL names the open turn without new history entries; focus stays on the list.
  await list.focus();
  await page.keyboard.press('ArrowDown');
  await expect(page).toHaveURL(`${ADMIN}/chat/${second}?sw=off`);
  await expect(options.nth(1)).toHaveAttribute('aria-selected', 'true');
  await expect(list).toBeFocused();
  await page.keyboard.press('ArrowUp');
  await expect(page).toHaveURL(`${ADMIN}/chat/${first}?sw=off`);
  await expect(list).toBeFocused();

  // The open turn's tab survives picking another turn.
  await page.getByRole('tab', { name: /^Model trace/ }).click();
  await options.nth(1).click();
  await expect(page.getByRole('tab', { name: /^Model trace/ })).toHaveAttribute('aria-selected', 'true');

  // Only the list and the panel scroll; the window, header and pill tabs stay put.
  const tabs = page.getByRole('tablist', { name: 'Sections of this interaction' });
  const before = (await tabs.boundingBox())!.y;
  const got = await page.evaluate(() => {
    const doc = document.scrollingElement!;
    window.scrollTo(0, 400);
    const pane = document.querySelector<HTMLElement>('.list-pane')!;
    pane.scrollTop = pane.scrollHeight;
    return { doc: doc.scrollTop, docFits: doc.scrollHeight <= doc.clientHeight + 1, list: getComputedStyle(pane).overflowY };
  });
  expect(got).toEqual({ doc: 0, docFits: true, list: 'auto' });
  expect(await page.locator('.chat-turn__panel').evaluate((el) => getComputedStyle(el).overflowY)).toBe('auto');
  expect(Math.abs((await tabs.boundingBox())!.y - before)).toBeLessThanOrEqual(1);
});

test('chat on a phone: the list, then the turn with "‹ Chat" in the top bar; Back returns to the row', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${ADMIN}/chat?sw=off`);
  const list = page.getByRole('listbox', { name: /Chatbot interactions/ });
  const option = list.getByRole('option', { name: /^can you move bm to wed/ });
  await expect(option).toBeVisible();
  await expect(page.getByRole('button', { name: 'Copy transcript' })).toHaveCount(0);

  await option.click();
  await expect(page).toHaveURL(/\/chat\/c-move\?sw=off$/);
  await expect(list).toBeHidden();
  await expect(page.getByRole('button', { name: 'Copy transcript' })).toBeVisible();
  // The phone frame's open turn: no window title bar; the top bar carries the way back.
  await expect(page.getByRole('heading', { level: 2, name: 'Interactions', exact: true })).toBeHidden();
  const back = page.getByRole('button', { name: 'Back to the list (Chat)' });
  await expect(back).toBeVisible();
  expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight + 1)).toBe(true);

  await back.click();
  await expect(page).toHaveURL(`${ADMIN}/chat?sw=off`);
  await expect(option).toBeVisible();
  await expect(list).toBeFocused();
  await expect(list).toHaveAttribute('aria-activedescendant', (await option.getAttribute('id'))!);

  // A deep link opens the turn alone; its back step goes to the list.
  await page.goto(`${ADMIN}/chat/c-when?sw=off`);
  await expect(page.getByRole('heading', { level: 2, name: 'Mon 28 Sep · 00:00' })).toBeVisible();
  await page.getByRole('button', { name: 'Back to the list (Chat)' }).click();
  await expect(page).toHaveURL(`${ADMIN}/chat?sw=off`);
  await expect(list).toBeVisible();
});

test('chat Raw tab: the turn as stored, in its own scrolling panel', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/chat/c-move?sw=off`);
  await page.getByRole('tab', { name: 'Raw' }).click();
  await expect(page.getByRole('tab', { name: 'Raw' })).toHaveAttribute('aria-selected', 'true');
  const raw = page.locator('.chat__detail pre.chat-raw');
  await expect(raw).toBeVisible();
  expect((await raw.textContent())!.trim().length).toBeGreaterThan(0);
});
