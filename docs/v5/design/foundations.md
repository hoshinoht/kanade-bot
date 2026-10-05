# Foundations: tokens, colour, type, shape, motion, progress

Source of truth: `web/packages/tokens/src/` (`_tokens.scss`, `_contrast.scss`,
`colorways.ts`, `dynamic.ts`, `fonts.css`) and `web/packages/ui/src/styles/`
(`_easing.scss`, `_motion.scss`, `_progress.scss`). Citation keys: `README.md`.

## Colour tokens

Use only these; never a board hex. Light values live on `:root` and the
`[data-colorway]` blocks; dark faces override inside the dark media query and
`[data-theme="dark"]`. `_tokens.scss`

| Group | Tokens | Notes |
|---|---|---|
| Anchors (per colourway) | `--ground --surface --ink --dim --line --win --win-ink --accent --accent-ink --ok --risk` | 11 anchors per face |
| Fixed signals | `--warn` (amber, waiting), `--own` (blue, own time) | same on every ground |
| Text-grade variants | `--dim-text --ok-text --risk-text --own-text --warn-text --accent-text`, fill `--accent-fill` | in `_contrast.scss`; use for text and accent fills (blossom/coral-like accents fail as fills) |
| Derived | `--raise --line-soft --faint --off --accent-wash --win-soft` | `color-mix`, follow every face |
| M3E surfaces | `--select` (selected container), `--select-edge` (its light 1.5 px edge), `--select-ink`, `--pane` (= `--raise`), `--board`, `--row`, `--row-lift`, `--row-hover`, `--chip-fill`, `--seg-fill`, `--pageline`, `--ground-ink` | `[old §Tokens]`; the old spec's `--select-ring` is `--select-edge` (README D5) |
| Difficulty pills | `--pill-{e,n,h,c,x}-{bg,fg}` | the game's colours; no colourway touches them |
| Monogram tint | `--mono-s/-l`, `--mono-ink-*`, `--mono-line-*` + per-boss `--mono-hue` | |
| Entry-art veil | `--art-veil` (0.38), `--art-crop-sheet` (30 %), `--art-crop-card` (60 %) | crops hide name plates baked into the art |

- **Hover** is a state layer: light faces 8 % accent over `--row`, dark faces
  7 % ink (`_contrast.scss`); `tokens.spec` keeps it CIE76 ΔE ≥ 3.5 from
  `--pane`, `--row` and `--select`. `[DR 2026-10-02]`
- **Selection** is the `--select` fill plus `--select-edge`; the "open" label
  on a selected row is screen-reader only. `[web/AGENTS]`, `_m3e-primitives.scss`
- Board hex → token translation table: `[old §Mockup hex → token translation]`.

## Colourways

13 colourways in 4 sets, each with light and dark faces, chosen per browser
(`localStorage`), stamped on `<html data-colorway data-theme>` before first
paint by `theme-boot.js`; mode System / Light / Dark. `colorways.ts`,
`[DR 2026-10-04]`

| Set | Colourways (storage key → shown name) |
|---|---|
| Base | marigold → Otonose (default), blossom → Nazuna, periwinkle → Sumire, twilight → Hinano |
| Blue Archive | hoshino, mika, seia, hina, aris |
| Terminal | catppuccin, tokyonight, github |
| Dynamic | dynamic → "Avatar" (palette from the bot avatar, contrast-checked, marigold fallback; `dynamic.ts`) |

- Coral is retired; a saved coral falls back to the default. `[DR 2026-10-04]`
- Pickers group by set, sets collapsible; the set holding the current
  colourway starts open; open state is per session. `[DR 2026-10-04]`,
  `config/ThemeTiles.svelte`, `components/ThemePicker.svelte`
- **Planned:** none outstanding for colourways.

## Contrast

- WCAG AA: 4.5:1 text, 3:1 rings/borders/large UI, in every colourway and
  both faces. Approved overrides live **only** in `_contrast.scss`.
  `[guide] Token map`, `[web/AGENTS]`
- `e2e/tokens.spec.ts` checks pairs in all 26 looks: `--select-ink`/`--ink` on
  `--select`, `--dim-text` on `--pane` and `--select`, `--ink` on `--row`,
  `--chip-fill`, `--seg-fill`, `--board`, `--pageline`, `--ground-ink` on
  `--ground`, `--accent-ink` on `--accent-fill`, `--surface` on `--risk-text`,
  text on `--row-hover`.
- Secondary text on a tinted row takes `--dim-text` (plain `--dim` fails on
  `--select`/`--row-hover`). `_m3e-primitives.scss`
- `a11y.spec` runs the full axe walk in marigold light + dark; other looks run
  `color-contrast` on eight representative screens. `[DR 2026-10-04]`

## Type

Faces (self-hosted, `fonts.css`): **Solway** 700/800 (`--display`: names,
titles), **Zilla Slab** 400/500/600 (`--body`: prose, controls), **Maple
Mono** 400/500/600 (`--mono`: times, ids, counts, latencies). Boards use
Sometype Mono because Maple Mono is not on Google Fonts; the app keeps Maple
Mono. `[old §The mockups are not code]`.

Weight 700 on Zilla Slab and Maple Mono is **synthesised** today (no 700 face
is loaded), so it renders heavier and wider than the boards' real bold.
**Planned (user decision 2026-10-05):** load real 700 faces for Zilla Slab and
Maple Mono in `fonts.css` (self-hosted, latin subset); re-run
`e2e/fonts.spec.ts` and the clipping checks after, since widths change.

Scale (never raw px): `_tokens.scss`

| Token | Size | Role (board px it replaces, `[old §Type]`) |
|---|---|---|
| `--fs-micro` | 0.7rem | eyebrows `.cap`, table heads, pills (9.5–10.5 px, uppercase, 0.12em) |
| `--fs-mini` | 0.76rem | timestamps, ids, counts in rows (11–12) |
| `--fs-small` | 0.83rem | chips, buttons, secondary text (12.5–13) |
| `--fs-body-sm` | 0.9rem | row text, forms (13.5–14) |
| `--fs-body` | 1rem | window and row titles (14.5–15) |
| `--fs-lg` | 1.1rem | emphasised number in a footer; dialog titles (17–18) |
| `--fs-brand` | 1.3rem | page title, detail heading (20–22, 800) |
| `--fs-clock` / `--fs-clock-narrow` | 1.55 / 1.25rem | a run's clock in a pane; decision-card slot (24–34) |
| `--fs-hero` / `--fs-hero-narrow` | 3.75 / 2.75rem | full run sheet clock only (≈60 / 44 px) `[DR 2026-10-04]` |
| `--fs-avatar` | 1.75rem | login monogram |

Overlines (`.cap`, field labels, side-pane `dt`): `--mono`, `--fs-micro`, 600,
uppercase, 0.12em, `--dim-text`. `_m3e-primitives.scss`

## Shape and space

- Radii: `--r` 12, `--r-in` 10, `--r-sm` 8, `--r-win` 20 (page window),
  `--r-win-in` 18 (title bar inside the 2 px border). `_tokens.scss`
- Component radii (pills 999, rows 6, selected rows 16–18, decision card 28,
  thread panel 20, settings cards 20): `shell-and-components.md`.
- `--shadow`, `--gutter: clamp(0.9rem, 3vw, 2rem)`.
- M3 shapes as `clip-path` classes (CSP-safe): cookie (9 lobes, 0.10), flower
  (6 lobes, 0.16), burst (12 points, 0.22). Uses: list portraits (cookie
  34–56 px), message avatars (flower 24–28), confidence badge (burst 40–52),
  empty/error glyph (cookie 112 / 72). Board source: `shapes.css`; app copy
  `$state-cookie` in `_m3e-primitives.scss` and `_inbox.scss`. `[old §Shapes]`
- Focus ring: `2px solid var(--accent)`, offset 2 (`_base.scss`); pills inside
  a scrolling strip use `outline-offset: -2px` so the strip cannot clip them.
  `[DR 2026-10-03]`

## Motion and loading

Values: `_easing.scss` (copied in `motion/easing.ts`; change both).

| Name | Value | Use |
|---|---|---|
| `$spring` / `SPRING` | sampled spatial spring, 250 ms | shape changes only (never colour/opacity) |
| `$standard` | `cubic-bezier(0.2, 0, 0, 1)`, enter 250 ms | pane, dialog, toast enter |
| `$exit` | `cubic-bezier(0.3, 0, 1, 1)`, 200 ms | exits |

- Animate only `transform`, `opacity`, `border-radius`, colour. Class-driven
  `@keyframes`, `@starting-style` or Web Animations (`element.animate()`);
  never Svelte transitions. `_motion.scss`, `[old §Live updates]`
- **Press shape-morph (Exp D):** key buttons, `.btn--risk` and `.seg` buttons
  step their corners down while pressed; on by default, no switch.
  `[DR 2026-10-01]`, `_motion.scss:33`
- **Planner overshoot (Exp E):** stays **opt-in**, off by default, behind the
  Experiments switch (palette "Turn planner overshoot on", `kanade.overshoot`;
  `experiments.svelte.ts`). User decision 2026-10-05, superseding the
  2026-10-01 "on by default" approval.
- **Loading:** nothing for 200 ms (`motion/delay.svelte.ts` `LOADING_DELAY_MS`),
  then `LoadingState` centred in the waiting pane with visible words; long
  waits (> 5 s, e.g. re-read) show a progress bar from the start.
  `components/LoadingState.svelte`, `[old §Motion and loading]`
- **Exp A morphing indicator** stays behind the Experiments switch
  (`?experiments=on|off`, `kanade.experiments`). `[guide] Experiments`
- Presence (exit animations) `motion/presence.svelte.ts`; FLIP for card moves
  `motion/flip.ts`; pane enter `motion/enter.ts`.
- Reduced motion: no morph, no travel, still indicators, flat progress, still
  boss art (poster image). `_motion.scss`, `WavyProgress.svelte`
- **Planned:** data-driven arrival motion (new rows, number ticks) waits on
  live updates (SSE hints, backlog `live-updates`). `[old §Live updates]`

## Progress bars

Always on for everyone, not an experiment. `[DR 2026-10-04]`

- `WavyProgress` (`components/WavyProgress.svelte`, `_progress.scss`):
  determinate `progressbar`, round ends, gap, flat track. `wavy` drifts a sine
  fill; `wavy={false}` is flat; `ticks` marks fractions; `tone="warn"`;
  `fullWave` keeps a full bar of live work moving (Limits at 100 % in flight).
- `AnswerBar`: one flat segmented bar (on, maybe, waiting hatched; out leaves
  the track bare); counts are its accessible name.
- **At most two bars wave per screen**; the rest flat. `limits/permits.ts`
  `MAX_WAVES = 2`.
- **Server clock only** (`Week.generated_at`, `shared/wall.ts`). The Reminders
  "In" column still reads the browser clock: **Planned** server fire time /
  `now` field. `[DR 2026-10-04]`
- Placements `[DR 2026-10-04]`: boss-week progress in the Week footer
  ("Day 5 of 7 · resets Thu 00:00", flat on phones); run countdown in the run
  pane head and Glance (fills 24 h → T-1h, then restarts over the last hour
  with T-15m at three quarters; `week/progress.ts`); answers fill in run cards
  and pane; proposal expiry in Inbox (flat, warn near the end); model permits
  in Limits and Config (flat, wavy only while calls are in flight). No day
  progress on the Live clock.
