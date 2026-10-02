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
