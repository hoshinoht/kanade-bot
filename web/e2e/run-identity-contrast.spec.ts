import { mkdirSync, writeFileSync } from 'node:fs';
import type { Locator, Page } from '@playwright/test';
import type { Week } from '@kanade/api-types';
import { COLORWAYS } from '../packages/tokens/src/colorways';
import { ADMIN, PUBLIC, REAL_ART, expect, settle, signInPublic, test, unconditional, useLook } from './support';

// Text contrast over the identity card's entry art, measured from pixels: every
// text element's colour against the worst rendered background pixel inside its
// content box -- the real composite of the card, the art spanning behind the
// bosses and chips, and the plates under the words -- with the words hidden
// (CSSOM, no inline style attribute) so the glyphs do not count. Normal text
// needs 4.5:1, the hero clock 3:1, in every colourway and face. The same
// measurement without the art is the card's own baseline. KANADE_REAL_ART=1
// measures the real pictures; the table goes to e2e/.captures/{real,synthetic}/.

const OUT = REAL_ART ? 'e2e/.captures/real' : 'e2e/.captures/synthetic';

const TEXT = [
  '.week-pane__time',
  '.week-pane__day',
  '.member-pane__time',
  '.member-pane__day',
  '.runsheet__time',
  '.runsheet__day',
  '.boss__name',
  '.boss__lv',
  '.pill',
  '.tone',
  '.runlink',
  '.chanmark',
  '.id',
  '.statusbar__note',
  '.btn',
  '.seg__btn',
].join(', ');

interface Item {
  name: string;
  large: boolean;
  color: [number, number, number];
  rect: { x: number; y: number; width: number; height: number };
}

async function fourBosses(page: Page) {
  await page.route(`${ADMIN}/api/admin/week*`, async (route) => {
    const response = await route.fetch(unconditional(route));
    const week = (await response.json()) as Week;
    const pick = (id: string) => week.runs.find((r) => r.id === id)?.bosses ?? [];
    const runs = week.runs.map((r) => (r.id === 'r-carling' ? { ...r, bosses: [...r.bosses, ...pick('r-kalos'), ...pick('r-bm')] } : r));
    await route.fulfill({ response, json: { ...week, runs } });
  });
}

/** The text elements of a card: colour, size class and content box. */
async function items(card: Locator): Promise<Item[]> {
  return card.evaluate((root, selector) => {
    const parse = (value: string): [number, number, number] => {
      const srgb = /color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)/.exec(value);
      if (srgb) return [Number(srgb[1]) * 255, Number(srgb[2]) * 255, Number(srgb[3]) * 255];
      const rgb = /rgba?\(([\d.]+),? ([\d.]+),? ([\d.]+)/.exec(value)!;
      return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
    };
    return [...root.querySelectorAll<HTMLElement>(selector)]
      .filter((el) => el.offsetParent !== null && el.textContent!.trim() && !el.closest('.vh'))
      // Nested matches (a pill inside a boss row) are measured on their own.
      .filter((el) => !el.querySelector(selector) || el.matches('.tone, .btn'))
      .map((el) => {
        const cs = getComputedStyle(el);
        // The text's own line boxes (a Range over its contents), not the element's padding or ring.
        const range = document.createRange();
        range.selectNodeContents(el);
        const lines = [...range.getClientRects()].filter((r) => r.width > 0 && r.height > 0);
        const x = Math.min(...lines.map((r) => r.left));
        const y = Math.min(...lines.map((r) => r.top));
        const size = parseFloat(cs.fontSize);
        const bold = Number(cs.fontWeight) >= 700;
        return {
          name: `${el.className.toString().split(' ')[0]} "${el.textContent!.trim().replace(/\s+/g, ' ').slice(0, 24)}"`,
          large: size >= 24 || (bold && size >= 18.66),
          color: parse(cs.color),
          rect: { x, y, width: Math.max(...lines.map((r) => r.right)) - x, height: Math.max(...lines.map((r) => r.bottom)) - y },
        };
      })
      .filter((i) => i.rect.width >= 2 && i.rect.height >= 2);
  }, TEXT);
}

/** Hides every word and icon in the card (CSSOM), optionally the art too. */
async function blank(card: Locator, art: boolean) {
  await card.evaluate((root, hideArt) => {
    for (const el of [root, ...root.querySelectorAll<HTMLElement | SVGElement>('*')]) {
      (el as HTMLElement).style.setProperty('color', 'transparent', 'important');
      (el as HTMLElement).style.setProperty('-webkit-text-fill-color', 'transparent', 'important');
      (el as HTMLElement).style.setProperty('text-shadow', 'none', 'important');
    }
    const arts = root.querySelector<HTMLElement>('.run__arts');
    if (arts) arts.style.setProperty('visibility', hideArt ? 'hidden' : 'visible');
  }, !art);
}

/** Worst contrast of each item's colour against the pixels in its box. */
async function worst(scratch: Page, png: Buffer, origin: { x: number; y: number }, list: Item[]): Promise<number[]> {
  return scratch.evaluate(
    async ({ data, origin, list }) => {
      const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: 'image/png' }));
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const ctx = canvas.getContext('2d')!;
      ctx.drawImage(bitmap, 0, 0);
      const lin = (c: number) => {
        const s = c / 255;
        return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      };
      const lum = (r: number, g: number, b: number) => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
      return list.map(({ color, rect }) => {
        const text = lum(...color);
        // Whole pixels inside the line box only: rounding out would sample a
        // chip's own outline ring at a fractional edge, which no glyph sits on.
        const x = Math.max(0, Math.ceil(rect.x - origin.x));
        const y = Math.max(0, Math.ceil(rect.y - origin.y));
        const w = Math.min(bitmap.width, Math.floor(rect.x + rect.width - origin.x)) - x;
        const h = Math.min(bitmap.height, Math.floor(rect.y + rect.height - origin.y)) - y;
        const px = ctx.getImageData(x, y, Math.max(1, w), Math.max(1, h)).data;
        let min = 21;
        for (let i = 0; i < px.length; i += 4) {
          const bg = lum(px[i]!, px[i + 1]!, px[i + 2]!);
          const ratio = (Math.max(bg, text) + 0.05) / (Math.min(bg, text) + 0.05);
          if (ratio < min) min = ratio;
        }
        return Math.round(min * 100) / 100;
      });
    },
    { data: png.toString('base64'), origin, list },
  );
}

async function measure(page: Page, card: Locator): Promise<{ name: string; need: number; art: number; plain: number }[]> {
  await page.evaluate(() => Promise.all([...document.images].map((i) => i.decode().catch(() => {}))));
  await settle(page);
  const list = await items(card);
  const box = (await card.boundingBox())!;
  const clip = { x: Math.floor(box.x), y: Math.floor(box.y), width: Math.ceil(box.width), height: Math.ceil(box.height) };
  // A blank page of its own does the pixel work, so the app page is left as it rendered.
  const scratch = await page.context().newPage();
  try {
    await blank(card, true);
    const withArt = await worst(scratch, await page.screenshot({ clip, animations: 'disabled' }), clip, list);
    await blank(card, false);
    const plain = await worst(scratch, await page.screenshot({ clip, animations: 'disabled' }), clip, list);
    return list.map((item, i) => ({ name: item.name, need: item.large ? 3 : 4.5, art: withArt[i]!, plain: plain[i]! }));
  } finally {
    await scratch.close();
  }
}

// Every colourway in both faces; Dynamic is marigold until an avatar palette is cached.
const LOOKS = COLORWAYS.filter((way) => way.key !== 'dynamic').flatMap((way) =>
  (['light', 'dark'] as const).map((theme) => ({ name: `${way.key}-${theme}`, colorway: way.key, theme })),
);
const SURFACES = [
  { name: 'pane', width: 1280, height: 800 },
  { name: 'sheet', width: 1280, height: 800 },
  { name: 'phone', width: 390, height: 844 },
] as const;
const RUNS = [
  { id: 'r-bm', title: 'XBM', bosses: 1 },
  { id: 'r-carling', title: 'HCarling + HMaleficStar', bosses: 2 },
  { id: 'r-carling', title: 'HCarling + HMaleficStar + XKalos + XBM', bosses: 4 },
] as const;

test.describe('identity-card contrast over the art', () => {
  test.describe.configure({ mode: 'parallel' });
  for (const look of LOOKS) {
    for (const surface of SURFACES) {
      test(`contrast ${surface.name} ${look.name}`, async ({ page }) => {
        await page.setViewportSize({ width: surface.width, height: surface.height });
        await useLook(page, look.colorway, look.theme);
        const rows: string[] = [];
        const failures: string[] = [];
        for (const run of RUNS) {
          await page.unrouteAll();
          if (run.bosses === 4) await fourBosses(page);
          await page.goto(`${ADMIN}/?sw=off`);
          await page.locator(`[data-run="${run.id}"] .plan-card__open`).click();
          let card: Locator;
          if (surface.name === 'pane') {
            card = page.getByRole('complementary', { name: run.title, exact: true }).locator('.week-pane__art');
          } else {
            if (surface.name === 'sheet')
              await page.getByRole('complementary', { name: run.title, exact: true }).getByRole('button', { name: 'Open in a larger view' }).click();
            card = page.getByRole('dialog', { name: run.title, exact: true }).locator('.runsheet__hero');
          }
          await expect(card.locator('.run__arts')).toBeAttached();
          for (const m of await measure(page, card)) {
            const ok = m.art >= m.need;
            rows.push(`| ${surface.name} | ${look.name} | ${run.bosses} | ${m.name} | ${m.need} | ${m.art} | ${m.plain} | ${ok ? 'pass' : 'FAIL'} |`);
            if (!ok) failures.push(`${run.bosses} bosses: ${m.name} ${m.art} < ${m.need} (plain ${m.plain})`);
          }
        }
        mkdirSync(OUT, { recursive: true });
        writeFileSync(`${OUT}/contrast-${surface.name}-${look.name}.md`, rows.join('\n') + '\n');
        // Every word at its bar over the art, in every colourway and face.
        expect(failures).toEqual([]);
      });
    }
    // The member portal's run pane shares the identity card's styles (`$pane: member-pane`).
    test(`contrast portal pane ${look.name}`, async ({ page }) => {
      await useLook(page, look.colorway, look.theme);
      await signInPublic(page);
      await page.goto(`${PUBLIC}/?sw=off`);
      await page.getByRole('button', { name: /, you are in$/ }).first().click();
      const card = page.getByRole('complementary', { name: /^Your run · / }).locator('.member-pane__art');
      await expect(card.locator('.run__arts')).toBeAttached();
      const rows: string[] = [];
      const failures: string[] = [];
      for (const m of await measure(page, card)) {
        const ok = m.art >= m.need;
        rows.push(`| portal | ${look.name} | 1 | ${m.name} | ${m.need} | ${m.art} | ${m.plain} | ${ok ? 'pass' : 'FAIL'} |`);
        if (!ok) failures.push(`${m.name} ${m.art} < ${m.need} (plain ${m.plain})`);
      }
      mkdirSync(OUT, { recursive: true });
      writeFileSync(`${OUT}/contrast-portal-${look.name}.md`, rows.join('\n') + '\n');
      expect(failures).toEqual([]);
    });
  }
});
