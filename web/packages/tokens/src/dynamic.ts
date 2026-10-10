/**
 * The Dynamic colourway: a palette from the bot's avatar. Pure (no DOM): the
 * caller reads the avatar's pixels from a same-origin canvas and applies the
 * result as `--dyn-*` custom properties, which `_tokens.scss` maps onto the
 * eleven anchors with marigold as the fallback.
 *
 * The palette keeps a vetted lightness ladder and takes only hue and chroma
 * from the avatar, then is checked with the same pairs as `e2e/tokens.spec.ts`
 * (including the derived `color-mix()` tokens); a failing palette is refused
 * and the page stays on marigold.
 */

export const ANCHORS = ['ground', 'surface', 'ink', 'dim', 'line', 'win', 'win-ink', 'accent', 'accent-ink', 'ok', 'risk'] as const;
export type Anchor = (typeof ANCHORS)[number];
export type Face = Record<Anchor, string>;
/** Light face also carries the text colour for the bare ground. */
export interface DynamicPalette {
  light: Face & { 'ground-ink': string };
  dark: Face;
}

type Rgb = [number, number, number];

const hexToRgb = (hex: string): Rgb => {
  const n = Number.parseInt(hex.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
};

const rgbToHex = (rgb: Rgb): string =>
  `#${rgb.map((c) => Math.round(Math.min(1, Math.max(0, c)) * 255).toString(16).padStart(2, '0')).join('')}`;

/** `color-mix(in srgb, a p, b)` for opaque colours: interpolation of the encoded channels. */
const mix = (a: Rgb, p: number, b: Rgb): Rgb => [0, 1, 2].map((i) => a[i]! * p + b[i]! * (1 - p)) as Rgb;

const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const unlin = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * c ** (1 / 2.4) - 0.055);
const lum = ([r, g, b]: Rgb) => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);

export const ratio = (a: Rgb, b: Rgb): number => {
  const x = lum(a);
  const y = lum(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
};

const lab = (rgb: Rgb): Rgb => {
  const [r, g, b] = rgb.map(lin) as Rgb;
  const f = (t: number) => (t > 0.008856 ? Math.cbrt(t) : 7.787 * t + 16 / 116);
  const x = f((0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047);
  const y = f(0.2126 * r + 0.7152 * g + 0.0722 * b);
  const z = f((0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883);
  return [116 * y - 16, 500 * (x - y), 200 * (y - z)];
};

const deltaE = (a: Rgb, b: Rgb) => {
  const [p, q] = [lab(a), lab(b)];
  return Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]);
};

/** The derived tokens `_tokens.scss`/`_contrast.scss` compute from the anchors (shared rules only). */
function derive(face: Face & { 'ground-ink'?: string }, dark: boolean): Record<string, Rgb> {
  const t = Object.fromEntries(ANCHORS.map((k) => [k, hexToRgb(face[k])])) as Record<Anchor, Rgb>;
  const white: Rgb = [1, 1, 1];
  const raise = mix(t.ink, 0.06, t.surface);
  const select = mix(t.accent, 0.14, t.surface);
  const row = mix(dark ? mix(t.ink, 0.1, t.surface) : white, 0.45, t.surface);
  return {
    ...t,
    '--ground-ink': dark || !face['ground-ink'] ? t.ink : hexToRgb(face['ground-ink']),
    '--pane': raise,
    '--select': select,
    '--select-edge': mix(t.accent, 0.45, t.surface),
    '--select-ink': mix(t.accent, 0.55, t.ink),
    '--accent-text': mix(t.accent, 0.55, t.ink),
    '--dim-text': mix(t.dim, 0.75, t.ink),
    '--board': mix(t.ink, 0.03, t.surface),
    '--row': row,
    '--row-hover': dark ? mix(t.ink, 0.07, row) : mix(t.accent, 0.08, row),
    '--chip-fill': mix(t.ink, 0.08, t.surface),
    '--seg-fill': mix(t.ink, 0.11, t.surface),
    '--pageline': t.surface,
    '--accent-fill': mix(t.accent, 0.6, t.ink),
    '--risk-text': mix(t.risk, 0.62, t.ink),
  };
}

/** Mirror of `e2e/tokens.spec.ts` PAIRS and HOVER_APART. */
const PAIRS: [string, string, number][] = [
  ['--select-ink', '--select', 4.5],
  ['--ink', '--select', 4.5],
  ['--dim-text', '--pane', 4.5],
  ['--dim-text', '--select', 4.5],
  ['--ink', '--row', 4.5],
  ['--ink', '--chip-fill', 4.5],
  ['--ink', '--seg-fill', 4.5],
  ['--ink', '--board', 4.5],
  ['--ink', '--pageline', 4.5],
  ['--ground-ink', '--ground', 4.5],
  ['--accent-ink', '--accent-fill', 4.5],
  ['--surface', '--risk-text', 4.5],
  ['--select-edge', '--select', 1.2],
  ['--ink', '--row-hover', 4.5],
  ['--dim-text', '--row-hover', 4.5],
  ['--accent-text', '--row-hover', 4.5],
];
const APART: [string, number][] = [
  ['--pane', 3.5],
  ['--row', 3.5],
  ['--select', 3.5],
];

/** Every check `tokens.spec` would fail for this face, as readable lines (empty = passes). */
export function failures(face: Face & { 'ground-ink'?: string }, dark: boolean, slack = 0): string[] {
  const d = derive(face, dark);
  const get = (k: string) => d[k] ?? d[k.replace(/^--/, '')]!;
  const out: string[] = [];
  for (const [fg, bg, min] of PAIRS) {
    const r = ratio(get(fg), get(bg));
    if (r < min + slack) out.push(`${fg} on ${bg}: ${r.toFixed(2)} < ${min}`);
  }
  for (const [k, min] of APART) {
    const e = deltaE(get('--row-hover'), get(k));
    if (e < min + slack) out.push(`--row-hover vs ${k}: ΔE ${e.toFixed(1)} < ${min}`);
  }
  return out;
}

// ── OKLCH ──────────────────────────────────────────────────────────────────

const toOklab = (rgb: Rgb): Rgb => {
  const [r, g, b] = rgb.map(lin) as Rgb;
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
};

const fromOklch = (L: number, C: number, h: number): Rgb | null => {
  const a = C * Math.cos((h * Math.PI) / 180);
  const b = C * Math.sin((h * Math.PI) / 180);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const rgb = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  if (rgb.some((c) => c < -1e-4 || c > 1 + 1e-4)) return null;
  return rgb.map(unlin) as Rgb;
};

/** OKLCH to hex, giving up chroma (never lightness or hue) until it fits sRGB. */
export const oklch = (L: number, C: number, h: number): string => {
  for (let c = C; c >= 0; c -= 0.004) {
    const rgb = fromOklch(L, c, h);
    if (rgb) return rgbToHex(rgb);
  }
  return rgbToHex(fromOklch(L, 0, h)!);
};

/** The avatar's dominant hue and its chroma, from RGBA pixels (canvas ImageData order). */
export function dominant(pixels: ArrayLike<number>): { hue: number; chroma: number } {
  const BINS = 24;
  const weight = new Array<number>(BINS).fill(0);
  const sumX = new Array<number>(BINS).fill(0);
  const sumY = new Array<number>(BINS).fill(0);
  const sumC = new Array<number>(BINS).fill(0);
  for (let i = 0; i + 3 < pixels.length; i += 4) {
    if (pixels[i + 3]! < 128) continue;
    const [L, a, b] = toOklab([pixels[i]! / 255, pixels[i + 1]! / 255, pixels[i + 2]! / 255]);
    if (L < 0.15 || L > 0.97) continue;
    const C = Math.hypot(a, b);
    const h = (Math.atan2(b, a) * 180) / Math.PI;
    const bin = Math.floor((((h % 360) + 360) % 360) / (360 / BINS)) % BINS;
    // Weighted by chroma: a grey background should not outvote a small coloured subject.
    const w = C + 0.002;
    weight[bin]! += w;
    sumX[bin]! += Math.cos((h * Math.PI) / 180) * w;
    sumY[bin]! += Math.sin((h * Math.PI) / 180) * w;
    sumC[bin]! += C * w;
  }
  let best = 0;
  for (let i = 1; i < BINS; i++) if (weight[i]! > weight[best]!) best = i;
  if (weight[best] === 0) return { hue: 70, chroma: 0 };
  const hue = ((Math.atan2(sumY[best]!, sumX[best]!) * 180) / Math.PI + 360) % 360;
  return { hue, chroma: sumC[best]! / weight[best]! };
}

/** The unchecked palette for a hue and chroma. */
export function buildPalette(hue: number, chroma: number): DynamicPalette {
  const c = Math.max(0, chroma);
  const groundC = Math.min(Math.max(c, 0.03), 0.12);
  // A grey avatar still gets a hue in the accent, or hover and selection would meet at night.
  const accentC = Math.min(Math.max(c, 0.08), 0.15);
  // The surface is a tinted paper, not white: the row lift toward white and the
  // hover layer then stay apart from the ink-shaded pane (tokens.spec ΔE).
  const light: Face = {
    ground: oklch(0.76, groundC, hue),
    surface: oklch(0.92, 0.03, hue),
    ink: oklch(0.3, 0.03, hue),
    dim: oklch(0.44, 0.02, hue),
    line: oklch(0.85, 0.02, hue),
    win: oklch(0.37, 0.04, hue),
    'win-ink': oklch(0.92, 0.03, hue),
    accent: oklch(0.52, accentC, hue),
    'accent-ink': oklch(0.985, 0.01, hue),
    ok: '#2b7a57',
    risk: '#b3343f',
  };
  const dark: Face = {
    ground: oklch(0.2, 0.02, hue),
    surface: oklch(0.27, 0.015, hue),
    ink: oklch(0.94, 0.012, hue),
    dim: oklch(0.78, 0.02, hue),
    line: oklch(0.37, 0.03, hue),
    win: oklch(0.36, 0.035, hue),
    'win-ink': oklch(0.94, 0.012, hue),
    accent: oklch(0.78, Math.min(accentC, 0.14), hue),
    'accent-ink': oklch(0.24, 0.04, hue),
    ok: '#8fd6a6',
    risk: '#ff9098',
  };
  // Ink on the bare ground, deepened toward black as `_contrast.scss` does for light grounds.
  let groundInk = light.ink;
  if (ratio(hexToRgb(groundInk), hexToRgb(light.ground)) < 4.5) groundInk = rgbToHex(mix(hexToRgb(light.ink), 0.5, [0, 0, 0]));
  return { light: { ...light, 'ground-ink': groundInk }, dark };
}

/** A contrast-checked palette for a hue and chroma, or null when a check fails. */
export function paletteFor(hue: number, chroma: number): DynamicPalette | null {
  const palette = buildPalette(hue, chroma);
  // A little slack: the browser rounds color-mix() results slightly differently.
  if (failures(palette.light, false, 0.1).length || failures(palette.dark, true, 0.1).length) return null;
  return palette;
}

/** The palette for an avatar's pixels, or null (stay on marigold). */
export function dynamicPalette(pixels: ArrayLike<number>): DynamicPalette | null {
  const { hue, chroma } = dominant(pixels);
  return paletteFor(hue, chroma);
}

/** `--dyn-<token>-l|d` custom properties for a palette, as [name, value] pairs. */
export function dynamicProperties(palette: DynamicPalette): [string, string][] {
  return [
    ...Object.entries(palette.light).map(([k, v]) => [`--dyn-${k}-l`, v] as [string, string]),
    ...Object.entries(palette.dark).map(([k, v]) => [`--dyn-${k}-d`, v] as [string, string]),
  ];
}
