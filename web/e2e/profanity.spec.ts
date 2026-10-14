import { ADMIN, expect, test } from './support';

// The chat profanity guardrail (user decision 2026-10-05): Config → Profanity
// saves its lists and line with the bar and its two checks at once; Chat
// filters profanity turns and names the side, the word and the line sent.
// Words are the mock's invented placeholders (blarg, drat, frak, gorram, smeg, zounds).

const LINE = "Ochitsuite! Let's keep it clean in here. Ask me again nicely and I'll help.";

test('Config Profanity: words and the line save whole; refusals stay inline; the checks apply at once', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/config?section=profanity&sw=off`);
  const panel = page.getByRole('tabpanel', { name: 'Profanity' });
  await expect(panel.getByRole('heading', { name: 'Profanity' })).toBeVisible();
  await expect(panel.getByRole('switch', { name: 'Check questions' })).toHaveAttribute('aria-checked', 'true');
  await expect(panel.getByRole('switch', { name: 'Check replies' })).toHaveAttribute('aria-checked', 'true');
  const line = panel.getByRole('textbox', { name: 'Sent instead' });
  await expect(line).toHaveValue(LINE);
  await expect(panel.getByText('Reminder headings and nudge lead-ins are checked against the same list.', { exact: false })).toBeVisible();

  // An extra word: typed, then Enter makes a chip. A built-in word is refused before anything is sent.
  const add = panel.getByRole('textbox', { name: 'Add blocked words' });
  await add.fill('Heck');
  await add.press('Enter');
  const extra = panel.getByRole('list', { name: 'Extra blocked words' });
  await expect(extra.getByRole('listitem')).toHaveText(['heck×']);
  await add.fill('drat');
  await add.press('Enter');
  await expect(panel.getByRole('alert')).toHaveText('“drat” is already on the built-in list.');
  await expect(add).toHaveAttribute('aria-invalid', 'true');
  await add.fill('');

  // A built-in word allowed again: found by typing, picked with Enter; the whole list is never shown.
  const find = panel.getByRole('combobox', { name: 'Find a built-in word to allow again' });
  await expect(panel.getByRole('listbox', { name: 'Built-in words' })).toBeHidden();
  await find.fill('me');
  await expect(panel.getByRole('listbox', { name: 'Built-in words' }).getByRole('option')).toHaveText(['smeg']);
  await expect(find).toHaveAttribute('aria-activedescendant', /.+/);
  await find.press('Enter');
  await expect(find).toHaveValue('');
  await expect(panel.getByRole('list', { name: 'Built-in words allowed again' }).getByRole('listitem')).toHaveText(['smeg×']);
  await expect(page.locator('.savebar__summary')).toContainText('2 unsaved changes');

  // The server refuses a line that uses a listed word: inline, the edits kept.
  await line.fill('Oi, no frak talk here.');
  await panel.getByRole('button', { name: 'Save profanity' }).click();
  await expect(panel.getByRole('alert')).toContainText('frak');
  await expect(line).toHaveAttribute('aria-invalid', 'true');
  await expect(line).toHaveValue('Oi, no frak talk here.');
  await expect(extra.getByRole('listitem')).toHaveCount(1);

  // Saved for real: one section, the lists whole and the line trimmed.
  await line.fill('  Oi, keep it clean in here.  ');
  const sent = page.waitForRequest((r) => r.method() === 'PATCH' && r.url().endsWith('/api/admin/config'));
  await panel.getByRole('button', { name: 'Save profanity' }).click();
  expect((await sent).postDataJSON()).toEqual({
    profanity: { extra_words: ['heck'], allowed_words: ['smeg'], deflection_line: 'Oi, keep it clean in here.' },
  });
  await expect(page.getByText('Profanity words saved; the next question uses them.')).toBeVisible();
  await expect(panel.getByRole('alert')).toHaveCount(0);
  await expect(line).toHaveValue('Oi, keep it clean in here.');
  await expect(page.locator('.savebar__summary')).toContainText('All changes saved');

  // Removing a chip keeps focus in the list's way in.
  await panel.getByRole('button', { name: 'Remove smeg' }).click();
  await expect(find).toBeFocused();

  // A switch applies at once: turning one off asks first, then offers Undo.
  await panel.getByRole('switch', { name: 'Check replies' }).click();
  const confirm = page.getByRole('dialog', { name: 'Stop checking replies?' });
  await confirm.getByRole('button', { name: 'Stop checking replies' }).click();
  await expect(panel.getByRole('switch', { name: 'Check replies' })).toHaveAttribute('aria-checked', 'false');
  await expect(page.getByText('No longer checking replies.')).toBeVisible();
  const config = await (await page.request.get(`${ADMIN}/api/admin/config`)).json();
  expect(config.profanity).toMatchObject({ check_questions: true, check_replies: false, extra_words: ['heck'], allowed_words: ['smeg'] });

  // The contents list summarises the section; a reload shows what was saved (the removed chip was never saved).
  await expect(page.getByRole('tab', { name: /Profanity/ })).toContainText('questions');
  await page.reload();
  await expect(panel.getByRole('list', { name: 'Built-in words allowed again' }).getByRole('listitem')).toHaveText(['smeg×']);
});

test('Chat: the profanity filter lists the three hits; each turn names its side, word and what was sent', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/chat?sw=off`);
  await page.getByRole('button', { name: 'Filters (0)' }).click();
  await page.getByRole('group', { name: 'Filters' }).getByRole('checkbox', { name: 'profanity' }).check();
  await expect(page).toHaveURL(/outcome=profanity/);
  const list = page.getByRole('listbox', { name: /Chatbot interactions/ });
  await expect(list.getByRole('option')).toHaveCount(3);
  await page.keyboard.press('Escape');

  const cases = [
    { id: 'c-deflected', side: 'question', word: 'blarg', sent: `“${LINE}”`, label: 'Sent instead' },
    { id: 'c-safe-line', side: 'reply', word: 'frak', sent: `“${LINE}”`, label: 'Sent instead' },
    { id: 'c-recovered', side: 'reply', word: 'smeg', sent: 'Retry answered cleanly', label: 'Outcome' },
  ];
  for (const c of cases) {
    await page.goto(`${ADMIN}/chat/${c.id}?outcome=profanity&sw=off`);
    const guard = page.getByRole('region', { name: `Profanity in the ${c.side}` });
    await expect(guard).toBeVisible();
    await expect(guard.locator('dt')).toHaveText(['Matched', c.label]);
    await expect(guard.locator('dd')).toHaveText([c.word, c.sent]);
    await expect(page.locator('.chat-turn__meta')).toContainText('profanity');
  }
  // An ordinary turn has no profanity block.
  await page.goto(`${ADMIN}/chat/c-when?sw=off`);
  await expect(page.getByRole('heading', { level: 2, name: 'Mon 28 Sep · 00:00' })).toBeVisible();
  await expect(page.locator('.chat-guard')).toHaveCount(0);
});

for (const [width, height] of [
  [1280, 800],
  [390, 844],
] as const) {
  test(`profanity text fits at ${width}×${height}: nothing clipped in the section or the turn`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    // The verification.md audit for these regions: own text clipped by its box (clip-x/clip-y), or its
    // text range cut by a non-scrolling clipping ancestor (cut); visually hidden text is skipped.
    const clipped = () =>
      page.evaluate(() => {
        const out: string[] = [];
        for (const el of document.querySelectorAll<HTMLElement>('.settings__panel:not([hidden]) *, .chat-guard, .chat-guard *')) {
          const texts = [...el.childNodes].filter((n) => n.nodeType === Node.TEXT_NODE && n.textContent!.trim());
          const box = el.getBoundingClientRect();
          if (!texts.length || box.width <= 1 || box.height <= 1 || el.tagName === 'TEXTAREA') continue;
          const style = getComputedStyle(el);
          const what = `${el.tagName.toLowerCase()}.${el.className} “${el.textContent!.trim().slice(0, 30)}”`;
          if (style.overflowX !== 'visible' && el.scrollWidth > el.clientWidth + 1) out.push(`clip-x ${what}`);
          if (style.overflowY !== 'visible' && style.overflowY !== 'auto' && el.scrollHeight > el.clientHeight + 1) out.push(`clip-y ${what}`);
          let clip: HTMLElement | null = el.parentElement;
          while (clip && !['hidden', 'clip'].includes(getComputedStyle(clip).overflowX) && !['hidden', 'clip'].includes(getComputedStyle(clip).overflowY)) clip = clip.parentElement;
          if (!clip) continue;
          const edge = clip.getBoundingClientRect();
          for (const node of texts) {
            const range = document.createRange();
            range.selectNodeContents(node);
            const r = range.getBoundingClientRect();
            if (r.left < edge.left - 1 || r.right > edge.right + 1) out.push(`cut ${what}`);
          }
        }
        return out;
      });
    await page.goto(`${ADMIN}/config?section=profanity&sw=off`);
    const find = page.getByRole('combobox', { name: 'Find a built-in word to allow again' });
    await expect(find).toBeVisible();
    await find.fill('r');
    expect(await clipped()).toEqual([]);
    for (const id of ['c-deflected', 'c-safe-line', 'c-recovered']) {
      await page.goto(`${ADMIN}/chat/${id}?sw=off`);
      await expect(page.locator('.chat-guard')).toBeVisible();
      expect(await clipped()).toEqual([]);
    }
    expect(await page.evaluate(() => document.scrollingElement!.scrollHeight <= innerHeight + 1)).toBe(true);
  });
}
