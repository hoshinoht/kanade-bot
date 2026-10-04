import type { Page } from '@playwright/test';
import { ADMIN, expect, test } from './support';

// Phone clipping regressions: the Inbox heading's boss tags wrap as whole
// units, and the Members name cell keeps its name, aliases and chip inside it.

const SIZES = [{ width: 390, height: 844 }, { width: 360, height: 780 }];

async function itemIds(page: Page, tab: string): Promise<string[]> {
  await page.goto(`${ADMIN}/inbox?tab=${tab}&sw=off`);
  await expect(page.locator('[data-item]').first()).toBeVisible();
  return page.locator('[data-item]').evaluateAll((items) => items.map((item) => item.getAttribute('data-item')!));
}

/** Overlaps inside the open item's heading, as readable strings (empty when it is clean). */
function headingFaults(page: Page) {
  return page.locator('.proposal__title').evaluate((title) => {
    const meets = (a: DOMRect, b: DOMRect) => a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;
    const bound = title.getBoundingClientRect();
    const tags = [...title.querySelectorAll<HTMLElement>(':scope > .boss')];
    const text = [...title.childNodes].filter((node) => node.nodeType === Node.TEXT_NODE && node.textContent!.trim()).flatMap((node) => {
      const range = document.createRange();
      range.selectNodeContents(node);
      return [...range.getClientRects()];
    });
    const faults: string[] = [];
    tags.forEach((tag, i) => {
      const box = tag.getBoundingClientRect();
      const label = tag.textContent!.trim();
      if (box.right > bound.right + 0.5 || box.left < bound.left - 0.5) faults.push(`${label} leaves the heading`);
      for (const other of tags.slice(i + 1)) if (meets(box, other.getBoundingClientRect())) faults.push(`${label} meets ${other.textContent!.trim()}`);
      if (text.some((rect) => meets(box, rect))) faults.push(`${label} meets the heading text`);
      const name = tag.querySelector('.boss__name')!;
      const pill = tag.querySelector('.pill')!;
      if (meets(name.getBoundingClientRect(), pill.getBoundingClientRect())) faults.push(`${label}: the name runs under its pill`);
      const line = parseFloat(getComputedStyle(name).lineHeight) || parseFloat(getComputedStyle(name).fontSize) * 1.2;
      if (name.getBoundingClientRect().height > line * 1.5) faults.push(`${label}: the name wraps inside its tag`);
    });
    return { tags: tags.length, faults };
  });
}

for (const size of [...SIZES, { width: 1280, height: 800 }]) {
  test(`Inbox: heading boss tags wrap whole, never overlapping, at ${size.width}×${size.height}`, async ({ page }) => {
    await page.setViewportSize(size);
    let checked = 0;
    for (const tab of ['self_service', 'extractor']) {
      for (const id of await itemIds(page, tab)) {
        await page.goto(`${ADMIN}/inbox?tab=${tab}&item=${id}&sw=off`);
        await expect(page.locator('.proposal__title')).toBeVisible();
        await page.evaluate(() => document.fonts.ready);
        const { tags, faults } = await headingFaults(page);
        expect(faults, `${tab}/${id}`).toEqual([]);
        checked += tags > 1 ? 1 : 0;
      }
    }
    // The pwa-mock's multi-boss member request must be among them.
    expect(checked).toBeGreaterThan(0);
  });
}

// The default text size, and phones that scale text up (Android's font size, 125%).
for (const size of SIZES) {
  for (const scale of ['', '125%']) {
    test(`Members: every name cell keeps its parts inside it at ${size.width}×${size.height}${scale ? ` with ${scale} text` : ''}`, async ({ page }) => {
      await page.setViewportSize(size);
      await page.goto(`${ADMIN}/members?sw=off`);
      await expect(page.locator('[data-member="1014"]')).toBeAttached();
      await page.evaluate(async (fontSize) => {
        await document.fonts.ready;
        if (fontSize) document.documentElement.style.fontSize = fontSize;
      }, scale);
      const faults = await page.locator('.memberlist__row').evaluateAll((rows) => rows.flatMap((row) => {
        const who = row.querySelector('strong')!.textContent!;
        const cell = row.querySelector('.row-content')!.getBoundingClientRect();
        const stat = row.querySelector('.memberlist__stat')!.getBoundingClientRect();
        const name = row.querySelector('.memberlist__name')!;
        const out: string[] = [];
        for (const part of name.querySelectorAll<HTMLElement>('strong, .id, .chip')) {
          const box = part.getBoundingClientRect();
          if (box.width === 0) continue;
          const label = `${who}: ${part.textContent!.trim()}`;
          if (box.left < cell.left - 0.5 || box.right > cell.right + 0.5 || box.top < cell.top - 0.5 || box.bottom > cell.bottom + 0.5) out.push(`${label} leaves its cell`);
          if (box.right > stat.left - 0.5) out.push(`${label} reaches THIS WK`);
        }
        const strong = row.querySelector<HTMLElement>('.memberlist__name strong')!;
        if (strong.scrollWidth > strong.clientWidth) out.push(`${who}: the name is cut`);
        const chip = name.querySelector<HTMLElement>('.chip');
        if (chip && chip.scrollWidth > chip.clientWidth) out.push(`${who}: the chip is cut`);
        return out;
      }));
      expect(faults).toEqual([]);
      await expect(page.getByRole('button', { name: /^Kohane/ }).locator('.memberlist__name .chip')).toHaveText('chat only');
    });
  }
}

// The sheet's alias chips and their × stay inside the full-screen dialog, with a
// finger-sized × where the pointer is coarse.
for (const touch of [false, true]) {
  test.describe(`Members sheet aliases at 390×844${touch ? ' on touch' : ''}`, () => {
    test.use({ viewport: { width: 390, height: 844 }, hasTouch: touch });
    test('chips and × buttons fit the dialog, never clipped', async ({ page }) => {
      await page.goto(`${ADMIN}/members?sw=off`);
      await page.getByRole('button', { name: /^Mika/ }).click();
      const sheet = page.getByRole('dialog');
      const input = sheet.getByRole('textbox', { name: 'New alias for Mika' });
      // The longest alias the server takes (32 characters).
      await input.fill('quitealongaliasthatfillsthechip1');
      await sheet.getByRole('button', { name: 'Add' }).click();
      await expect(sheet.getByRole('button', { name: 'Remove alias quitealongaliasthatfillsthechip1 from Mika' })).toBeVisible();
      const faults = await sheet.locator('.membersheet__aliases').evaluate((list) => {
        const bound = list.getBoundingClientRect();
        const out: string[] = [];
        if (list.scrollWidth > list.clientWidth) out.push('the chip row scrolls sideways');
        if (document.documentElement.scrollWidth > window.innerWidth) out.push('the page scrolls sideways');
        for (const chip of list.querySelectorAll<HTMLElement>('.membersheet__alias')) {
          const box = chip.getBoundingClientRect();
          const button = chip.querySelector('button')!.getBoundingClientRect();
          const name = chip.textContent!.trim();
          if (box.left < bound.left - 0.5 || box.right > bound.right + 0.5) out.push(`${name} leaves the row`);
          if (button.right > box.right + 0.5 || button.top < box.top - 0.5 || button.bottom > box.bottom + 0.5) out.push(`${name}: the × leaves its chip`);
          if (chip.scrollWidth > chip.clientWidth) out.push(`${name}: the chip is cut`);
          if (button.width < 24 || button.height < 24) out.push(`${name}: the × is under 24px`);
        }
        return out;
      });
      expect(faults).toEqual([]);
      const size = (await sheet.locator('.membersheet__alias-remove').first().boundingBox())!;
      expect(Math.min(size.width, size.height)).toBeGreaterThanOrEqual(touch ? 44 : 24);
    });
  });
}

// The weekly timing sheet's Day / Time / Owner stack one per line on phones:
// each box stays inside the dialog, none meets another, and the owner's name
// is not cut.
for (const size of SIZES) {
  test(`Fixed editor: Day, Time and Owner fit without overlap at ${size.width}×${size.height}`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${ADMIN}/fixed?open=f-bm&sw=off`);
    const sheet = page.getByRole('dialog', { name: 'Tuesday 23:30 — XBM' });
    await expect(sheet.getByLabel('Owner')).toHaveValue('1012');
    const faults = await sheet.locator('.fixedsheet__fields').evaluate((grid) => {
      const meets = (a: DOMRect, b: DOMRect) => a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;
      const bound = grid.closest('dialog')!.getBoundingClientRect();
      const out: string[] = [];
      if (document.documentElement.scrollWidth > window.innerWidth) out.push('the page scrolls sideways');
      if (grid.scrollWidth > grid.clientWidth) out.push('the fields scroll sideways');
      const boxes = [...grid.querySelectorAll<HTMLElement>('.field')].map((field) => ({
        name: field.querySelector('span')!.textContent!.trim(),
        box: field.querySelector('select, input')!.getBoundingClientRect(),
      }));
      boxes.forEach(({ name, box }, i) => {
        if (box.left < bound.left - 0.5 || box.right > bound.right + 0.5) out.push(`${name} leaves the sheet`);
        if (box.width < 120) out.push(`${name} is only ${Math.round(box.width)}px wide`);
        for (const other of boxes.slice(i + 1)) if (meets(box, other.box)) out.push(`${name} meets ${other.name}`);
      });
      const owner = grid.querySelector<HTMLSelectElement>('.field:nth-child(3) select')!;
      const text = document.createElement('span');
      text.textContent = owner.selectedOptions[0]!.textContent;
      text.style.font = getComputedStyle(owner).font;
      document.body.append(text);
      const needed = text.getBoundingClientRect().width;
      text.remove();
      if (needed > owner.clientWidth - 24) out.push('the owner name is cut');
      return { count: boxes.length, out };
    });
    expect(faults.count).toBe(3);
    expect(faults.out).toEqual([]);
  });
}

// The decision's optional consequence line (VarRail2): the API's words under the
// change, nothing at all when it sends null, wrapped (never cut) on a phone.
const CONSEQUENCE = 'Party unchanged · 3 reminders will move';

test('Inbox: the consequence line sits between the change and Approve, and is absent when null', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${ADMIN}/inbox?tab=extractor&item=p-bm-move&sw=off`);
  const card = page.getByRole('complementary', { name: 'Decide this change' });
  const line = card.locator('.proposal__consequence');
  await expect(line).toHaveText(CONSEQUENCE);
  const [change, said, approve] = await Promise.all([card.locator('.proposal__would'), line, card.locator('.decision__approve')].map((l) => l.boundingBox()));
  expect(said!.y).toBeGreaterThanOrEqual(change!.y + change!.height - 0.5);
  expect(approve!.y).toBeGreaterThanOrEqual(said!.y + said!.height - 0.5);
  for (const id of ['p-fa-request', 'p-kalos-expired', 'p-limbo-new']) {
    // All three are member requests (conflicted, expired, nothing to say).
    await page.goto(`${ADMIN}/inbox?tab=self_service&item=${id}&sw=off`);
    await expect(page.locator(`[data-item="${id}"]`)).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('.proposal__title')).toBeVisible();
    await expect(page.locator('.proposal__consequence')).toHaveCount(0);
  }
});

for (const size of SIZES) {
  for (const text of [CONSEQUENCE, 'Party unchanged · 2 reminders will move, 1 will be added · Supercalifragilisticexpialidocious replaces Bobby']) {
    test(`Inbox: the consequence line fits at ${size.width}×${size.height}${text === CONSEQUENCE ? '' : ' when long'}`, async ({ page }) => {
      await page.setViewportSize(size);
      if (text !== CONSEQUENCE) {
        await page.route(/\/api\/admin\/inbox(\?.*)?$/, async (route) => {
          const res = await route.fetch();
          const items = (await res.json()) as { id: string; consequence: string | null }[];
          await route.fulfill({ response: res, json: items.map((item) => (item.id === 'p-bm-move' ? { ...item, consequence: text } : item)) });
        });
      }
      await page.goto(`${ADMIN}/inbox?tab=extractor&item=p-bm-move&sw=off`);
      const line = page.locator('.proposal__consequence');
      await expect(line).toHaveText(text);
      await page.evaluate(() => document.fonts.ready);
      await line.scrollIntoViewIfNeeded();
      const faults = await line.evaluate((el) => {
        const out: string[] = [];
        const box = el.getBoundingClientRect();
        const pane = el.parentElement!.getBoundingClientRect();
        if (document.documentElement.scrollWidth > window.innerWidth) out.push('the page scrolls sideways');
        if (el.scrollWidth > el.clientWidth + 0.5) out.push('the line is cut');
        if (box.left < pane.left - 0.5 || box.right > pane.right + 0.5) out.push('the line leaves its panel');
        if (box.left < 0 || box.right > window.innerWidth) out.push('the line leaves the screen');
        // It stacks under the change, never squeezing it into a column beside it.
        const change = el.parentElement!.querySelector('.proposal__changes');
        if (change && box.top < change.getBoundingClientRect().bottom - 0.5) out.push('the line sits beside the change');
        const bar = document.querySelector('.decision-card')!.getBoundingClientRect();
        if (box.bottom > bar.top + 0.5) out.push('the line sits under the action bar');
        return out;
      });
      expect(faults).toEqual([]);
    });
  }
}

/** Children of `selector` that are cut or stick out of their parent's box. */
function cut(page: Page, selector: string, parts: string) {
  return page.locator(selector).evaluateAll((els, parts) => {
    const out: string[] = [];
    for (const el of els) {
      const box = el.getBoundingClientRect();
      for (const part of el.querySelectorAll<HTMLElement>(parts)) {
        const p = part.getBoundingClientRect();
        if (!p.width) continue;
        const name = `${part.className.split(' ')[0]} "${part.textContent!.trim().slice(0, 20)}"`;
        if (p.right > box.right + 0.5 || p.left < box.left - 0.5) out.push(`${name} leaves its row`);
        if (part.scrollWidth > part.clientWidth + 0.5) out.push(`${name} is cut`);
      }
    }
    return out;
  }, parts);
}

for (const size of [...SIZES, { width: 1082, height: 736 }]) {
  test(`Chat rows keep their model and outcome whole at ${size.width} px`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${ADMIN}/chat?sw=off`);
    await expect(page.locator('.chat-row').first()).toBeVisible();
    await page.evaluate(() => document.fonts.ready);
    expect(await cut(page, '.chat-row', '.chat-row__model, .chat-row__outcome')).toEqual([]);
  });
}

for (const size of SIZES) {
  test(`Extractions title bar and prompt bar stay on screen at ${size.width} px`, async ({ page }) => {
    await page.setViewportSize(size);
    await page.goto(`${ADMIN}/extractions?sw=off`);
    const reread = page.getByRole('button', { name: 'Re-read channels' });
    await expect(reread).toBeVisible();
    const head = await page.locator('.extract-window__head').boundingBox();
    const btn = await reread.boundingBox();
    expect(head!.y + head!.height - (btn!.y + btn!.height)).toBeGreaterThanOrEqual(6);
    await page.locator('[data-call]').first().click();
    await page.getByRole('tab', { name: 'Prompt' }).click();
    await expect(page.locator('.extract-code__bar')).toBeVisible();
    expect(await cut(page, '.extract-code__bar', ':scope > *')).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}
