# Material 3 Expressive: evaluation for the admin PWA

Status: **proposal** (research 2026-09-26, checked against the live M3 site
the same day — values and sources in `m3-expressive-verification.md`; nothing
adopted beyond what `pwa-design-guidelines.md` already records). Adoption
starts next session with the "fold into window" layout; step 5 below needs a
user decision.

Question: is it worth bringing Material 3 (M3) Expressive into the admin PWA,
inside the windows and across the overall layout, while keeping the fixed
`100dvh` frame (document, masthead and window chrome never scroll; only the
selected panel scrolls)? If so, what, in what order, and what to avoid?

## Verdict

**Worth it selectively: adopt Expressive's layout and feedback ideas, not its
visual style.**

- Google reports 46 studies, 18,000+ participants; in eye tracking across 10
  apps, key elements were spotted "up to four times faster", and age effects in
  fixation times largely disappeared. This is vendor research on consumer
  mobile apps, not peer reviewed; "4×" is a best case (a larger Send button
  moved above the keyboard, in an accent colour).
- The same article's caveats apply to us: Expressive "might not be suitable for
  something like a banking interface"; breaking familiar patterns (a list
  replaced by scattered artwork) and removing text labels lowered usability; "a
  strong minority" preferred calmer versions; unfamiliarity lowered scores.
- M3's motion guidance says the **Standard** motion scheme suits utilitarian
  products; the repo already adopted the standard and effects springs.
- There are no official web components for Expressive: Material Web
  (`@material/web`) has been in maintenance mode since 2024-06-10. Everything
  is hand-built in SCSS and Svelte, as the current experiments already are.

Transferable lesson: **give each window's one key thing size, contrast and
containment**, keep familiar patterns and text labels, and keep a calmer option.

## The levers, applied

Google names colour, shape, size and containment (plus motion) as what makes a
screen expressive, used to make the key action stand out and to group related
items. Each window gets one key action, larger, placed where the user is
already working, in the accent colour, and always labelled:

| Window | Key action |
|---|---|
| Inbox | Approve |
| Config | Save |
| Planner | Move |
| Chat turn | Copy transcript |

## Inside the windows

1. **Title bar as an action row** (a Kanade adaptation, not M3 docked-toolbar
   compliance: M3's docked toolbar is 64 dp, bottom-placed, with 48×48 dp
   targets and overflow into a trailing menu). The folded stats strip and the ⓘ
   note live in the existing title bar/toolbar row, adding no height ("Area
   follows importance"). Use the standard (low-emphasis) scheme; the strip
   collapses to one chip when space is short. No floating toolbars over the
   panel: they cover rows and blur which area scrolls.
2. **One emphasized number per window** (e.g. "428 interactions"): M3's
   emphasized roles keep the baseline sizes and raise the weight (Medium, or
   Bold for title/label roles), so this is a weight step within the existing
   type scale; numbers in the mono face.
3. **Active-row containment:** a filled surface and a larger radius (M3 list
   specs: selected rows 16 dp all round on the primary-container role,
   unselected contained rows 4 dp inner / 16 dp outer), alongside the existing
   text, border and state cues (never colour alone). In list-detail this marks
   the row whose detail is open; it must not look like bulk selection.
4. **List-detail at 840 px and wider** (M3: two panes for Expanded and Large,
   one for Compact and Medium, not two panes at Medium when content is dense).
   A fixed list pane and a flexible detail pane, each scrolling on its own.
   Inbox already works this way; Chat and Extractions open details in their own
   view today, so changing them is a user decision (step 5).
5. **Filters** stay one row of connected button groups (Expressive replaces
   segmented buttons, now "no longer recommended") and collapse to
   "Filters (n)". Connected groups are for selectable options; plain actions use
   standard groups.
6. **Sticky headers inside the panel** (`position: sticky` on table heads and
   the week rail within the scrolling panel, never on the body).
7. **Loading indicator:** nothing under 200 ms; the indicator for 200 ms–5 s
   (inside buttons and for panel waits); a progress indicator over 5 s; never
   switch one into the other mid-wait. It replaces "Loading…" on a panel's
   first load, with visible text beside it.

## Overall layout

| M3 width class | Panes | M3 navigation | Kanade |
|---|---|---|---|
| Compact, < 600 | 1 | Nav bar or modal rail | One-row masthead (Week and Inbox pinned, the rest in "More"); no bottom nav bar (it costs ~64–80 px of `dvh`) |
| Medium, 600–839 | 1 | Nav bar or modal rail | 1 pane |
| Expanded, 840–1199 | 2 | Standard or modal rail | Two panes where they apply; masthead ~48 px |
| Large, 1200–1599 | 2 | Rail | Lift the `.shell` 1180 px cap for two-pane windows |
| Extra-large, ≥ 1600 | 1–3 | Rail | A third pane only if a clear need appears |

- Height matters because the document never scrolls: phones in landscape
  (e.g. 844×390, compact height) keep one pane.
- A collapsed navigation rail could help at about 1000×670, but it changes the
  navigation rule, so it would be an experiment.
- Margins: M3 uses 16 dp side margins at Compact and 24 dp from Medium up,
  with a 24 dp gap between panes; fixed list panes default to 360 dp
  (Expanded) or 412 dp (Large and up), and at least one pane stays flexible.
  The existing `--gutter: clamp(0.9rem, 3vw, 2rem)` is already close;
  "smaller outer margins" fits.
- Keeping `100dvh` on phones: keep `height: 100dvh` on `.frame`; add
  `viewport-fit=cover` with `env(safe-area-inset-*)` padding on the masthead and
  the frame's bottom edge. `interactive-widget=resizes-content` helps Chromium
  but is **unsupported on iOS Safari**, and `dvh` is not guaranteed to shrink
  for the keyboard, so handle keyboard overlap with `window.visualViewport`
  measurements as well. Cover it with an e2e at 390×844 with a focused input
  and a real-device Safari check.

## Motion and shape

- **Worth it:** standard springs for press, state and settle (as CSS
  `linear()`); a press shape-morph (radius step, ~150–250 ms); the morphing
  shape inside the loading indicator.
- **Marginal:** the planner pick-up overshoot (keep it small, dragged card
  only).
- **Row → detail:** use M3's simple forward/backward transition (standard
  easing `cubic-bezier(0.2, 0, 0, 1)`, about 250 ms enter / 200 ms exit); in
  two-pane layouts keep the list still and change only the detail. M3 cautions
  against container transforms in deep, utility-focused navigation, so reserve
  one (e.g. via the View Transitions API, our own choice) for a rare hero view,
  if at all.
- **Guardrails:** animate only `transform`, `opacity`, and radius or colour on
  small elements; never width/height or per-frame layout reads (the wavy
  progress path stays one element and stops when hidden); reduced motion uses subtle fades and disables shape morphing and other
  decorative effects (M3), with a still state for every loop; check 60 fps with a Playwright trace at 390×844.

## Adoption plan

Layout ships normally; visual steps sit behind the existing Experiments switch,
which stays a user choice because a strong minority prefers calmer designs.

1. **Fold into window:** 48 px masthead, stats strip and ⓘ in the title bar,
   denser one-line rows, smaller margins, safe areas; gated by layout e2e.
2. **Experiment C:** selected-row containment and one emphasized number per
   window; the key action per window (table above).
3. **Experiment D:** press shape-morph with the standard spring.
4. **Loading indicator as standard** for panel first loads (200 ms delay);
   promote experiments A (loading indicator) and B (wavy progress) out of the
   switch if the user approves.
5. **User decision:** list-detail at ≥ 840 px for Chat and Extractions, and
   lifting the 1180 px cap.
6. **Experiment E (optional):** planner overshoot, then forward/backward
   row → detail transitions (a container transform only for a rare hero view).

Judge each step on before/after screenshots with a small rubric in the spirit
of Google's attribute scales: can the key element be found at once, is the
window's purpose clear, does anything scroll that should not, and is any label
or familiar pattern lost.

## Do not adopt

- `@material/web` (maintenance mode).
- M3 dynamic colour or vibrant palettes (they would replace the five
  colourways).
- The Expressive motion scheme as the default.
- FAB or FAB menu.
- Floating toolbars over the panel; a bottom nav bar on phones.
- Large or hero type in operational windows.
- Automatic density scaling. M3 makes compact density opt-in (steps 0 to −3,
  about 4 dp each) and allows cautious targets below 48 px for dense scanning;
  48 px stays the default and the density control itself stays 48 px.
- Removing text labels, or replacing tables and lists with novel layouts.

## Risks

- Hand-built components drift from M3.
- Motion regressions on low-end phones.
- A weaker Kanade identity.
- Guideline churn (M3 changed its layout guidance again in May 2026).

## Still unconfirmed

Most earlier gaps are resolved in `m3-expressive-verification.md` (emphasized
type values, margins and pane widths, transition guidance, the seven design
tactics, component specs, iOS `interactive-widget` support). Still open:

- The exact 2026 search app-bar height and a universal expanded-rail width.
- Durations for reduced-motion springs and loading loops (M3 gives none).
- Safari and standalone-PWA keyboard behaviour across iOS versions.
- A maximum number of emphasized elements per screen (M3 states none; the
  "1–2 hero moments" limit is per product).

## Sources

All accessed 2026-09-26.

| Source | Read | Used for | Limits |
|---|---|---|---|
| Google Design, "Expressive Design: Google's UX Research" (2025), <https://design.google/library/expressive-material-design-google-research> | Full text | Study counts, "4× faster", age effects, the four levers, caveats, calmer minority, attribute scales | Vendor research, consumer apps, not peer reviewed |
| Android Developers, "Use window size classes" (updated 2026-09-22), <https://developer.android.com/develop/ui/compose/layouts/adaptive/use-window-size-classes> | Full text | Width breakpoints 600/840/1200/1600; height classes | Android-focused |
| M3 guidelines: breakpoints, scaffold, panes, density, toolbars, motion, loading indicator (lastmod 2026-02 to 05), via the unofficial mirror <https://github.com/Glavo/md3-reference-hub> | Full text (mirror) | Panes and navigation per class, density opt-in, docked vs floating toolbars, Standard scheme for utilitarian products, 200 ms–5 s rule | Unofficial snapshot (2026-05-21); may differ from the live site |
| M3 blog, "What's new at I/O 2026" (2026-05-19), same mirror | Partial | Expressive layout scaffold and spacing | Mirror |
| Material Web announcement, discussion #5642 (2024-06-10), <https://github.com/material-components/material-web/discussions/5642> | Full text | Maintenance mode, no new features | — |
| Chrome for Developers, viewport resize behaviour, <https://developer.chrome.com/blog/viewport-resize-behavior/> | Search snippet only | `interactive-widget` and `dvh` with the on-screen keyboard | Safari not verified |
