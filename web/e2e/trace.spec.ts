import AxeBuilder from '@axe-core/playwright';
import { ADMIN, expect, test } from './support';

// Chat turn: one-line tool trace with a full-text viewer, and Copy transcript.

test('tool trace: one line per call; long text opens in a keyboard-reachable modal', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  await page.goto(`${ADMIN}/chat/c-guide?sw=off`);
  await page.getByRole('tab', { name: /Tool trace/ }).click();
  const row = page.getByRole('row', { name: /knowledge\.read/ });
  // The multi-line result stays one line, and "12 ms" never wraps.
  const heights = await row.evaluate((tr) => [...tr.children].map((c) => (c as HTMLElement).getBoundingClientRect().height));
  expect(Math.max(...heights)).toBeLessThan(60);
  const took = row.getByRole('cell', { name: '12 ms' });
  expect(await took.evaluate((td) => td.scrollHeight <= td.clientHeight + 1 && getComputedStyle(td).whiteSpace)).toBe('nowrap');

  const open = row.getByRole('button', { name: /open the full return/ });
  await open.focus();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'knowledge.read: return' });
  await expect(dialog).toBeVisible();
  await expect(dialog.locator('pre')).toContainText('"tips": [');
  await expect(dialog.locator('pre')).toContainText('… [truncated, 12034 bytes]');
  expect(await dialog.locator('pre').evaluate((el) => getComputedStyle(el).whiteSpace)).toBe('pre-wrap');
  // Contrast is judged once the open animation has settled.
  await dialog.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
  const axe = await new AxeBuilder({ page }).include('dialog[open]').analyze();
  expect(axe.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => v.id)).toEqual([]);

  await dialog.getByRole('button', { name: 'Copy' }).click();
  await expect(page.getByText('Copied.')).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('[truncated, 12034 bytes]');

  // Esc closes and returns focus to the preview that opened it.
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(open).toBeFocused();

  // A backdrop click closes too.
  await row.getByRole('button', { name: /open the full arguments/ }).click();
  const args = page.getByRole('dialog', { name: 'knowledge.read: arguments' });
  await expect(args).toBeVisible();
  await page.mouse.click(5, 5);
  await expect(args).toBeHidden();

  // A text-selection drag from the panel that ends on the backdrop keeps it open.
  await row.getByRole('button', { name: /open the full return/ }).click();
  await expect(dialog).toBeVisible();
  // Chrome sends that click to the <dialog> (the common ancestor); dispatched here, as a synthetic drag varies by engine.
  await dialog.evaluate((el) => {
    el.querySelector('pre')!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  });
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  // The preview is short, so a screen reader is not read the whole result.
  const name = await open.evaluate((el) => el.textContent ?? '');
  expect(name.length).toBeLessThan(200);
  expect(name).not.toContain('\n');
});

test('tool trace: a withheld turn shows the placeholder, with nothing to open', async ({ page }) => {
  await page.goto(`${ADMIN}/chat/c-withheld?sw=off`);
  await page.getByRole('tab', { name: /Tool trace/ }).click();
  const row = page.getByRole('row', { name: /schedule\.read/ });
  await expect(row.getByText('[message withheld]')).toHaveCount(2);
  await expect(row.getByRole('button')).toHaveCount(0);
});

test('copy transcript: Markdown by default, JSON on request, names not ids', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  await page.goto(`${ADMIN}/chat/c-guide?sw=off`);
  await page.getByRole('button', { name: 'Copy transcript' }).click();
  await expect(page.getByText('Transcript copied as Markdown.')).toBeVisible();
  const md = await page.evaluate(() => navigator.clipboard.readText());
  expect(md).toMatch(/^# Chat turn c-guide/);
  expect(md).toContain('## Question\n\n```\nany tips for hard limbo\n```');
  expect(md).toContain('### knowledge.read — ok, 12 ms');
  expect(md).toContain('… [truncated, 12034 bytes]');
  expect(md).toContain('- Channel: #limbo-trio');
  expect(md).not.toMatch(/Who: \d+/);

  await page.getByRole('combobox', { name: 'Transcript format' }).selectOption('json');
  await page.getByRole('button', { name: 'Copy transcript' }).click();
  await expect(page.getByText('Transcript copied as JSON.')).toBeVisible();
  const json = JSON.parse(await page.evaluate(() => navigator.clipboard.readText()));
  expect(json.id).toBe('c-guide');
  expect(json.rounds[0].calls[0].name).toBe('knowledge.read');
  expect(json.rounds[0].calls[0].result).toContain('truncated');
});

test('copy transcript: a withheld turn stays redacted', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: ADMIN });
  await page.goto(`${ADMIN}/chat/c-withheld?sw=off`);
  await page.getByRole('button', { name: 'Copy transcript' }).click();
  const md = await page.evaluate(() => navigator.clipboard.readText());
  expect(md).toContain('## Question\n\n```\n[message withheld]\n```');
  expect(md).not.toContain('admin token');
  expect(md).not.toContain('secret');
});

test('copy transcript: without a clipboard the text is shown selected', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'clipboard', { value: undefined }));
  await page.goto(`${ADMIN}/chat/c-guide?sw=off`);
  await page.getByRole('button', { name: 'Copy transcript' }).click();
  const dialog = page.getByRole('dialog', { name: 'Transcript' });
  await expect(dialog.locator('pre')).toContainText('# Chat turn c-guide');
  await dialog.getByRole('button', { name: 'Copy' }).click();
  const area = dialog.getByRole('textbox');
  await expect(area).toBeFocused();
  expect(await area.evaluate((el: HTMLTextAreaElement) => el.selectionEnd - el.selectionStart)).toBeGreaterThan(100);
});
