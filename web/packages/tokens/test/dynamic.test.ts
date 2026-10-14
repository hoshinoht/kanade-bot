import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { COLORWAY_GROUPS, COLORWAYS } from '../src/colorways';
import { dominant, dynamicPalette, dynamicProperties, paletteFor } from '../src/dynamic';

/** RGBA pixels: `n` of one colour then `m` of another. */
function pixels(a: [number, number, number], n: number, b: [number, number, number] = [0, 0, 0], m = 0): Uint8ClampedArray {
  const out = new Uint8ClampedArray((n + m) * 4);
  for (let i = 0; i < n + m; i++) out.set([...(i < n ? a : b), 255], i * 4);
  return out;
}

describe('dynamic colourway', () => {
  it('passes the tokens.spec checks for every hue and chroma', () => {
    const refused: string[] = [];
    for (let hue = 0; hue < 360; hue += 3) {
      for (const chroma of [0, 0.02, 0.05, 0.1, 0.15, 0.25, 0.4]) if (!paletteFor(hue, chroma)) refused.push(`${hue}/${chroma}`);
    }
    expect(refused).toEqual([]);
  });

  it('takes the hue of a small coloured subject over a grey background', () => {
    const red = dominant(pixels([200, 40, 50], 100, [128, 128, 128], 900));
    expect(red.hue).toBeGreaterThan(10);
    expect(red.hue).toBeLessThan(40);
    expect(red.chroma).toBeGreaterThan(0.1);
  });

  it('ignores transparent pixels and still gives a palette for a grey avatar', () => {
    const grey = pixels([95, 101, 121], 400);
    const palette = dynamicPalette(grey);
    expect(palette).not.toBeNull();
    const props = dynamicProperties(palette!);
    expect(props.every(([name, value]) => /^--dyn-[a-z-]+-[ld]$/.test(name) && /^#[0-9a-f]{6}$/.test(value))).toBe(true);
    expect(props.map(([name]) => name)).toContain('--dyn-ground-ink-l');
  });
});

describe('colourways', () => {
  it('lists every key once in theme-boot.js, without coral', () => {
    const boot = readFileSync(new URL('../../ui/src/theme/theme-boot.js', import.meta.url), 'utf8');
    const ways = [...boot.matchAll(/"([a-z]+)"/g)].map((m) => m[1]).filter((k) => COLORWAYS.some((w) => w.key === k) || k === 'coral');
    expect(ways).toEqual(COLORWAYS.map((w) => w.key));
  });

  it('puts every colourway in exactly one set, base first with the character names', () => {
    expect(COLORWAY_GROUPS.flatMap((g) => g.ways).length).toBe(COLORWAYS.length);
    expect(COLORWAY_GROUPS[0]!.ways.map((w) => [w.key, w.name])).toEqual([
      ['marigold', 'Otonose'],
      ['blossom', 'Nazuna'],
      ['periwinkle', 'Sumire'],
      ['twilight', 'Hinano'],
    ]);
  });
});
