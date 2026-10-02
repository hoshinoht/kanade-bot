# @kanade/tokens

Kanade's v4 portal design tokens as SCSS custom properties, for both v5 PWAs.
`src/_tokens.scss` is a value-for-value port of
`legacy/python/bot/api/static/portal/_tokens.scss` (its hex values diff clean
against the source); `src/_contrast.scss` holds the only deliberate changes.

Entries: `@kanade/tokens/index.scss` (tokens + contrast layer),
`@kanade/tokens/fonts.css` (self-hosted faces), `@kanade/tokens/colorways`
(appearance option keys shared with `@kanade/ui`'s theme picker and
`theme-boot.js`).

## Token map

| Group | Tokens | v4 source | v5 status |
|---|---|---|---|
| Colourway anchors | `--ground --surface --ink --dim --line --win --win-ink --accent --accent-ink --ok --risk` | `:root` (marigold) and `[data-colorway]` blocks, two dark spellings | Ported; `--win` (blossom, periwinkle, coral, dark coral) and `--dim` (marigold, periwinkle, all dark faces) raised to 4.5:1 in `_contrast.scss` |
| Shared signals | `--warn --own` | `:root` + dark blocks | Ported unchanged |
| Derived | `--raise --line-soft --faint --off --accent-wash --win-soft` | `:root` `color-mix()` | Ported; `--faint` re-derived toward `--ink` (was 3.3-3.8:1) |
| Text-only signals | `--dim-text --ok-text --risk-text --own-text --warn-text` | none (v4 used the fill colours as text) | New: the fill colour mixed toward `--ink`, for chips, statuses and freshness |
| Difficulty pills | `--pill-{e,n,h,c,x}-{bg,fg}` | `:root` + dark blocks | Ported; light `--pill-n-bg`/`--pill-h-bg` darkened slightly (3.53/4.21 → 4.6:1) |
| Boss monogram | `--mono-s --mono-l --mono-ink-s --mono-ink-l --mono-line-s --mono-line-l` | `:root` + dark blocks | Ported |
| Entry-art veil | `--art-veil --art-crop-sheet --art-crop-card` | `:root` | Ported |
| Type families | `--display` Solway, `--body` Zilla Slab, `--mono` Maple Mono (v5, user choice; v4 used Sometype Mono) | `:root`, Google Fonts `<link>` | Same stacks; faces self-hosted from `@fontsource/*` (OFL-1.1), latin subset, v4's weights (700/800, 400/500/600, 400/500/600) |
| Type scale | `--fs-micro … --fs-brand` (7 steps) + display sizes `--fs-clock`, `--fs-clock-narrow`, `--fs-avatar` | `:root` | Steps ported unchanged; v4's component-local sizes now map to tokens (monograms → `--fs-micro`/`--fs-small`, brand mark → `--fs-small`, "own time" → `--fs-lg`) |
| Shape | `--r --r-in --r-sm --shadow --gutter` | `:root` | Ported unchanged |
| M3E depth 2 | `--select --select-edge --select-ink --pane --board --row --row-hover --chip-fill --seg-fill --pageline --ground-ink --r-win --r-win-in` (`--row-lift` helper) | none | New (`docs/v5/m3e-rail-design-spec.md` "Tokens"): `color-mix()` from the anchors; `_contrast.scss` deepens `--select-ink` (blossom, coral) and `--ground-ink` (text on the bare ground: the unboxed page line) toward black on the periwinkle, coral and twilight light grounds; `--select-edge` (the spec's accent 45% over surface) is a selected item's light edge, not its state cue; `--row-hover` is a row's hover state layer (accent 8% over `--row`; `_contrast.scss` swaps in ink 7% for dark faces, where the accent layer met `--select`); `--pageline` (= `--surface`) is no longer used by the page line |
| Appearance keys | `colorway` ∈ marigold/blossom/periwinkle/coral/twilight; `theme` ∈ light/dark/(absent = system) | `theme_boot.html`, `portal.js`, `templating.COLORWAYS` | Same localStorage keys and values; v4's legacy-name migration dropped |

Ratios for every contrast change are commented in `_contrast.scss`; the axe
suite (`web/e2e/a11y.spec.ts`) checks all ten faces.

## Component map

Global classes keep their v4 names so the partials can be compared line by
line. Styles live in `@kanade/ui/src/styles/` unless noted.

| v5 component (`@kanade/ui`) | v4 partial / markup | Change |
|---|---|---|
| `_base.scss` (reset, focus ring, window dots, `.vh`) | `_base.scss`, `.vh` from `_chips-status.scss` | Adds `.skip` link |
| `Masthead.svelte`, `_shell.scss` | `_shell.scss` masthead/brand | No multi-page nav (one window per PWA); avatar is a monogram (no identity route under `img-src 'self'`) |
| `_frame.scss` | `_panes.scss` `body.framed` | Fixed `100dvh` frame at every width; v4 fell back to body scrolling below 900px |
| `_page-furniture.scss` | `_page-furniture.scss` | Uses the framed-page band sizing by default |
| `Tabs.svelte`, `_tabs.scss` | `_tabs.scss` fragment-link tabs | Real ARIA tablist (arrows/Home/End); narrow screens keep one panel instead of v4's stacked panels; unselected tabs no longer dimmed to 72% |
| `DayColumn.svelte`, `RunCardBody.svelte`, `_board.scss` | `_board.scss`, `partials/board.html` | Flex classes replace the inline `grid-template-columns`; below 900px days stack inside the panel; finished runs use a dashed recessed face instead of 62% opacity |
| `StatusMark`, `AnswerChip`, `BossTag`, `Icon` | `_chips-status.scss`, `_bosses.scss`, `partials/icons.html` (Feather) | Same shapes/words; text uses `--*-text` tokens |
| `Modal.svelte`, `_modal.scss` | `_modal.scss` | Native `showModal()`; focus returns to the opener |
| `ThemePicker.svelte`, `_theme-picker.scss` | `_theme-picker.scss`, `config.html` | Swatch colours from per-colourway classes, not an inline `style` |
| `RunTable.svelte`, `_stats-tables.scss` | `_stats-tables.scss` | Bosses are the row header (HPK) |
| `CommandPalette.svelte` (component CSS) | none | New; selection in chrome colours |
| `ToastRegion.svelte`, `_toast.scss` | `.flash` | New; timed toasts pause on hover/focus |
| `NapArt.svelte`, `NapWindow.svelte`, `nap/` | `web/shared/nap` | SVG inlined without its `<style>` (moved to component CSS); `--nap-*` mapping unchanged |
| `_motion.scss` | `_motion.scss` | Same `settle`/`lift` keyframes; reduced-motion block also stops iteration |
