import { mkdirSync, writeFileSync } from 'node:fs';
import type { BrowserContext, Locator, Page } from '@playwright/test';
import type { Week } from '@kanade/api-types';
import { ADMIN, REAL_ART, expect, settle, test } from './support';

// Text contrast over the identity card's entry art, measured from pixels: every
// text element's colour against the worst rendered background pixel inside its
// content box, with the words hidden (CSSOM, no inline style attribute) so the
// glyphs do not count. Normal text needs 4.5:1, the hero clock 3:1. The same
// measurement without the art is the card's own baseline. KANADE_REAL_ART=1
// measures the real pictures; the table goes to e2e/.captures/{real,synthetic}/.

const OUT = REAL_ART ? 'e2e/.captures/real' : 'e2e/.captures/synthetic';

const TEXT = [
  '.week-pane__time',
  '.week-pane__day',
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
    const response = await route.fetch();
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
async function worst(context: BrowserContext, png: Buffer, origin: { x: number; y: number }, list: Item[]): Promise<number[]> {
  const scratch = await context.newPage();
  try {
    return await scratch.evaluate(
      async ({ data, origin, list }) => {
        const bitmap = await createImageBitmap(new Blob([new Uint8Array(data)], { type: 'image/png' }));
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
          const x = Math.max(0, Math.round(rect.x - origin.x));
          const y = Math.max(0, Math.round(rect.y - origin.y));
          const w = Math.min(bitmap.width - x, Math.round(rect.width));
          const h = Math.min(bitmap.height - y, Math.round(rect.height));
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
      { data: [...png], origin, list },
    );
  } finally {
    await scratch.close();
  }
}

async function measure(page: Page, card: Locator): Promise<{ name: string; need: number; art: number; plain: number }[]> {
  await page.evaluate(() => Promise.all([...document.images].map((i) => i.decode().catch(() => {}))));
  await settle(page);
  const list = await items(card);
  const box = (await card.boundingBox())!;
  const clip = { x: Math.floor(box.x), y: Math.floor(box.y), width: Math.ceil(box.width), height: Math.ceil(box.height) };
  await blank(card, true);
  const withArt = await worst(page.context(), await page.screenshot({ clip, animations: 'disabled' }), clip, list);
  await blank(card, false);
  const plain = await worst(page.context(), await page.screenshot({ clip, animations: 'disabled' }), clip, list);
  return list.map((item, i) => ({ name: item.name, need: item.large ? 3 : 4.5, art: withArt[i]!, plain: plain[i]! }));
}

const LOOKS = [
  { name: 'blossom-light', colorway: 'blossom', theme: 'light' },
  { name: 'twilight-dark', colorway: 'twilight', theme: 'dark' },
];
const SURFACES = [
  { name: 'pane', width: 1280, height: 800 },
  { name: 'sheet', width: 1280, height: 800 },
  { name: 'phone', width: 390, height: 844 },
] as const;
const RUNS = [
  { id: 'r-bm', title: 'XBM', bosses: 1 },
  { id: 'r-carling', title: 'HCarling + HStar', bosses: 2 },
  { id: 'r-carling', title: 'HCarling + HStar + XKalos + XBM', bosses: 4 },
] as const;

test.describe('identity-card contrast over the art', () => {
  test.describe.configure({ mode: 'parallel' });
  for (const look of LOOKS) {
    for (const surface of SURFACES) {
      test(`contrast ${surface.name} ${look.name}`, async ({ page }) => {
        await page.setViewportSize({ width: surface.width, height: surface.height });
        await page.addInitScript(
          ([c, t]) => {
            localStorage.setItem('colorway', c!);
            localStorage.setItem('theme', t!);
          },
          [look.colorway, look.theme],
        );
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
            const ok = m.art >= m.need || m.art >= m.plain - 0.05;
            rows.push(`| ${surface.name} | ${look.name} | ${run.bosses} | ${m.name} | ${m.need} | ${m.art} | ${m.plain} | ${m.art >= m.need ? 'pass' : ok ? 'baseline' : 'FAIL'} |`);
            if (!ok) failures.push(`${run.bosses} bosses: ${m.name} ${m.art} < ${m.need} (plain ${m.plain})`);
          }
        }
        mkdirSync(OUT, { recursive: true });
        writeFileSync(`${OUT}/contrast-${surface.name}-${look.name}.md`, rows.join('\n') + '\n');
        // Below the bar only where the card is already below it without any art.
        expect(failures).toEqual([]);
      });
    }
  }
});
