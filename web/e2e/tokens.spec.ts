import { ADMIN, expect, test } from './support';

// The M3E tokens' contrast in every colourway and face, read from the real
// stylesheet (the browser resolves the color-mix() chains): text pairs need
// 4.5:1, the active-row ring 3:1 (docs/v5/m3e-rail-design-spec.md "Tokens"
// and "Accessibility checklist"). Overrides live in packages/tokens/_contrast.scss.

const LOOKS = (['marigold', 'blossom', 'periwinkle', 'coral', 'twilight'] as const).flatMap((c) =>
  (['light', 'dark'] as const).map((t) => [c, t] as const),
);

/** [foreground, background, minimum ratio]; a transparent background falls back to the ground. */
const PAIRS: [string, string, number][] = [
  ['--select-ink', '--select', 4.5],
  ['--ink', '--select', 4.5],
  ['--dim-text', '--pane', 4.5],
  ['--ink', '--row', 4.5],
  ['--ink', '--chip-fill', 4.5],
  ['--ink', '--seg-fill', 4.5],
  ['--ink', '--board', 4.5],
  // Page-line text in its title shape (every face, user decision 2026-10-01).
  ['--ink', '--pageline', 4.5],
  ['--select-ring', '--select', 3],
  ['--accent-ink', '--accent-fill', 4.5],
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
    const ratios = await page.evaluate((pairs) => {
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
      probe.remove();
      return out;
    }, PAIRS);
    const low = PAIRS.filter(([fg, bg, min]) => ratios[`${fg} on ${bg}`]! < min).map(
      ([fg, bg, min]) => `${fg} on ${bg}: ${ratios[`${fg} on ${bg}`]!.toFixed(2)} < ${min}`,
    );
    expect(low).toEqual([]);
  });
}
