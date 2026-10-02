import { ADMIN, expect, test } from './support';

// The M3E tokens' contrast in every colourway and face, read from the real
// stylesheet (the browser resolves the color-mix() chains): text pairs need
// 4.5:1 (docs/v5/m3e-rail-design-spec.md "Tokens" and "Accessibility
// checklist"); a selected row's state cue is its fill, shape and bold title,
// not its light edge. Overrides live in packages/tokens/_contrast.scss.

const LOOKS = (['marigold', 'blossom', 'periwinkle', 'coral', 'twilight'] as const).flatMap((c) =>
  (['light', 'dark'] as const).map((t) => [c, t] as const),
);

/** [foreground, background, minimum ratio]; a transparent background falls back to the ground. */
const PAIRS: [string, string, number][] = [
  ['--select-ink', '--select', 4.5],
  ['--ink', '--select', 4.5],
  ['--dim-text', '--pane', 4.5],
  // Secondary text on a selected row (it takes the text-grade dim there).
  ['--dim-text', '--select', 4.5],
  ['--ink', '--row', 4.5],
  ['--ink', '--chip-fill', 4.5],
  ['--ink', '--seg-fill', 4.5],
  ['--ink', '--board', 4.5],
  // Page-line text in its title shape (every face, user decision 2026-10-01).
  ['--ink', '--pageline', 4.5],
  // The unboxed page line (fidelity audit 2026-10-02): ink on the bare ground.
  ['--ground-ink', '--ground', 4.5],
  ['--accent-ink', '--accent-fill', 4.5],
  // Risk as the key action (Reject when Approve is blocked): label on the fill.
  ['--surface', '--risk-text', 4.5],
  // A selected item's light edge is a finish, not the state cue, but it must show.
  ['--select-edge', '--select', 1.2],
  // Text on a hovered row (the row state layer).
  ['--ink', '--row-hover', 4.5],
  ['--dim-text', '--row-hover', 4.5],
  ['--accent-text', '--row-hover', 4.5],
];

/** Fills a hovered row must stay apart from: [token, minimum CIE76 ΔE] (≈2.3 is a just-noticeable step). */
const HOVER_APART: [string, number][] = [
  ['--pane', 3.5],
  ['--row', 3.5],
  ['--select', 3.5],
];

for (const [colorway, theme] of LOOKS) {
  test(`tokens: M3E pairs meet contrast, ${colorway} ${theme}`, async ({ page }) => {
    await page.addInitScript(
      ([c, t]) => {
        localStorage.setItem('colorway', c!);
        localStorage.setItem('theme', t!);
      },
      [colorway, theme],
    );
    await page.goto(`${ADMIN}/?sw=off`);
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    const ratios = await page.evaluate(([pairs, apart]) => {
      // The computed colour of a probe painted with the token, as sRGB 0-1.
      const probe = document.createElement('i');
      document.body.append(probe);
      const rgba = (token: string): [number, number, number, number] => {
        probe.style.setProperty('color', `var(${token})`);
        const value = getComputedStyle(probe).color;
        const nums = (value.match(/[\d.]+/g) ?? []).map(Number);
        if (value.startsWith('color(srgb')) return [nums[0]!, nums[1]!, nums[2]!, nums[3] ?? 1];
        return [nums[0]! / 255, nums[1]! / 255, nums[2]! / 255, nums[3] ?? 1];
      };
      const lum = ([r, g, b]: number[]) =>
        [r!, g!, b!]
          .map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4))
          .reduce((sum, c, i) => sum + c * [0.2126, 0.7152, 0.0722][i]!, 0);
      const out: Record<string, number> = {};
      for (const [fg, bg] of pairs) {
        let back = rgba(bg);
        if (back[3] === 0) back = rgba('--ground');
        const a = lum(rgba(fg));
        const b = lum(back);
        out[`${fg} on ${bg}`] = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
      }
      // The hover state layer against the fills around it, by ΔE (Lab, D65).
      const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
      const lab = (token: string) => {
        const [r, g, b] = rgba(token).slice(0, 3).map(lin) as [number, number, number];
        const f = (t: number) => (t > 0.008856 ? Math.cbrt(t) : 7.787 * t + 16 / 116);
        const x = f((0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047);
        const y = f(0.2126 * r + 0.7152 * g + 0.0722 * b);
        const z = f((0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883);
        return [116 * y - 16, 500 * (x - y), 200 * (y - z)];
      };
      const hover = lab('--row-hover');
      for (const [token] of apart) {
        const other = lab(token);
        out[`--row-hover apart from ${token}`] = Math.hypot(hover[0]! - other[0]!, hover[1]! - other[1]!, hover[2]! - other[2]!);
      }
      probe.remove();
      return out;
    }, [PAIRS, HOVER_APART] as const);
    const low = PAIRS.filter(([fg, bg, min]) => ratios[`${fg} on ${bg}`]! < min).map(
      ([fg, bg, min]) => `${fg} on ${bg}: ${ratios[`${fg} on ${bg}`]!.toFixed(2)} < ${min}`,
    );
    const close = HOVER_APART.filter(([token, min]) => ratios[`--row-hover apart from ${token}`]! < min).map(
      ([token, min]) => `--row-hover vs ${token}: ΔE ${ratios[`--row-hover apart from ${token}`]!.toFixed(1)} < ${min}`,
    );
    expect([...low, ...close]).toEqual([]);
  });
}
