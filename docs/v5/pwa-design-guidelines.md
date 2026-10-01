# Kanade v5 PWA design migration

Status: migration requirements, not a completed visual review.

## Direction

The PWA changes the delivery architecture, not Kanade's visual identity.
Adopt the established portal CSS design rather than introducing a new theme,
component-library aesthetic, typography system or generic dashboard layout.
Retain all agreed portal workflows except explicitly removed features.

## Authoritative reference

- `legacy/python/bot/api/static/portal.scss` lists the ordered style modules.
- Its `portal/` partials define tokens, base typography, shell, forms, buttons,
  status chips, boss/run presentation, windows, tabs, tables, themes, motion,
  responsive layouts, modals and icons.
- `legacy/python/docs/images/portal-*.png` provide checked-in visual references;
  source styles and a rendered reference resolve gaps or outdated screenshots.
- The existing portal/theme/icon tests record compatibility evidence, not a
  requirement to reproduce their Python implementation or every test verbatim.

Never edit or copy the ignored generated `portal.css` as the source of truth.
The legacy CSS concatenation build remains untouched by the PWA migration;
the v5 build must produce its own versioned static assets without Python.

## Design principles (carried from v4)

Ported from `legacy/python/bot/api/templates/AGENTS.md`; v5 deviations are marked.

- **Kanade's Desktop:** a saturated coloured desktop holds cream content windows
  with solid chrome title bars, rounded frames, restrained shadows and the shared
  three-dot motif. Never place readable content directly on the coloured ground,
  and don't nest cards where one window boundary already establishes the region.
- **One window, title-bar tabs:** related sections on a page become tabs in one
  window's title bar, not stacked chrome-topped cards. Each tab is one coherent
  section, keeps its count or state visible when useful, and swaps within the
  window so only the active panel owns the content area and its scrolling. No
  second row of tab-like controls inside that window; never make users scroll the
  document between sibling sections. Exception (user-confirmed, as in v4 3.0):
  Config's many sections use a contents list beside the panel (a scrolling strip on
  phones) instead of title-bar tabs.
  Summary stat cards inside a window (Week tiles, Limits summary) are also an
  accepted v4 pattern, not nested-card clutter.
- **Accepted patterns (user decision, 2026-09-25):** summary cards inside a
  window are allowed where each card summarises one peer object — Limits'
  backend groups, as v4's limits.html does. Config keeps its side contents
  list (a vertical tablist inside the Settings window; a sideways strip on
  phones) rather than title-bar tabs, because twelve sections do not fit a
  title bar.
- **Fixed frame:** the masthead, page caption, window title bar and bottom status
  stay put; the window, pane, table, board column or sidebar that owns the content
  scrolls. Prefer flex sizing over viewport-height arithmetic, and the frame must
  yield when it would make fallback content unreachable. *v5 deviation:* this holds
  at every width (v4 let phones use document flow). *v5 admin, superseded (M3E
  gates G2 and G6, user-approved 2026-10-01):* the page caption is a 36 px page
  line on the ground instead of a page-head card. Only its title group (title,
  count and short context, or breadcrumb and title) is contained, in an
  outlined surface shape with Medium (12 px) corners among the line's Full
  chips; every other item is its own chip, so nothing legible sits on the bare
  ground. It holds the title, the count, the
  page's controls, an ⓘ for one-time help, the Live chip and Commands; the
  page is no longer capped at 1180 px, and panes set their own readable
  measures instead.
- **Task-first hierarchy:** keep the current scope and primary state visible, give
  the main working surface the remaining space, and demote one-time explanation
  before shrinking controls or data. Times and key counts are the loudest row-level
  values; member and boss names lead machine IDs.
- **Area follows importance (user rule, 2026-09-25):** screen area is a budget spent
  in order of importance. Each page names its primary surface and gives it the
  largest share of the viewport; supporting context (headers, summary tiles,
  filters, status lines) takes the least height that keeps it legible, and one-time
  help lives behind a disclosure. Never add chrome (an extra title bar, help line
  or padding) above the primary surface without taking the same height from
  something less important. Budgets per page:
  - *Week:* board ≥ 55% of a 1280×800 viewport and never shorter than v4's board
    at the same size; week header and tiles ≈ one compact row each; filters one
    row (active filters as chips); status line last.
  - *Inbox:* selected item's detail > side list > tabs/counts.
  - *Chat / Extractions:* result rows > filter bar > summary line; details open in
    their own view.
  - *Config:* the open section's panel > contents list.
  - *Run sheet:* time, bosses, party and status > roster edits > card timeline, ids.
  Layout tests assert the primary surface's share where it is measurable.
  Because the document never scrolls (fixed frame; only the owning window or
  panel scrolls), every pixel of fixed chrome is taken straight from the one
  scrolling area. So: the scrolling area must keep at least 55% of the viewport
  height and never less than 360 px on any supported size; when height is short
  (laptops ≈ 670 px, phone landscape, zoomed text), supporting rows degrade in
  order of least importance — help and explanations fold first, summary tiles
  collapse to one line then to a single summary chip, filters collapse to a
  "Filters (n)" button, and the status line shortens — before the primary
  surface gives up any height. Layout tests cover 1280×800, 1000×670, 1280×600,
  390×844 and 844×390 and fail if the scrolling area drops below the budget or the
  document itself becomes scrollable.
- **The boss week's shape:** wide screens use the seven-day board and return space
  from empty days to busy ones; narrow screens use the seven-cell summary rail plus
  a readable vertical run list. Scroll horizontally rather than compressing
  meaningful columns beyond legibility.
- **Type by role:** Solway for identity and titles, Zilla Slab for prose and
  controls, Sometype Mono for times, IDs, counts, latencies and aligned numbers.
  Use the shared type-scale tokens, never component-local sizes.
- **Colour:** only shared tokens; all five browser-local colourways (marigold
  default, blossom, periwinkle, coral, twilight) with System/Light/Dark, stamped
  before first paint, kept out of server state, and every light/dark set updated
  together. The bot's Discord avatar and banner supply identity (masthead, favicon,
  login) with monogram/colour fallbacks.
- **Never colour alone** for navigation, status, selection or risk: pair it with
  text, weight, borders, shapes, symbols and programmatic state, with visible focus
  and readable contrast in every colourway and mode.
- **Predictable navigation:** grouped destinations on desktop; Week and Inbox
  pinned on phones with the rest in a native disclosure. *v5 admin, superseded
  (M3E gates G1 and G7, user-approved 2026-10-01):* the grouped destinations
  sit in a navigation rail (labels always visible; expanded from 1440 px), and
  phones (and phone landscape) get a 48 px top bar with the Inbox always in
  reach plus a navigation drawer; never a bottom nav bar. The public app keeps
  its masthead until its own phone slice. Frequent actions stay
  visible, labels name outcomes, destructive actions stay visually separated, and
  every control is keyboard- and touch-usable.
- **Quiet motion:** only a short opacity/transform settle and clear busy feedback
  (v4's HTMX swap feedback becomes request/pending states). Never animate layout;
  honour reduced motion.
- **Boss art is scenery:** entry art is a faded, masked veil behind a card's text,
  never a banner or a free-standing image; missing art renders nothing.

## Component and layout rules

1. Carry forward existing custom-property tokens and their semantic roles.
   Inventory exact values, typography, spacing, borders, radii and appearance
   option keys before implementing the PWA shell; document deliberate changes.
2. Split styles and components by responsibility. Keep the app shell and page
   orchestration thin; avoid a monolithic stylesheet or page component.
3. Preserve the established operational detail layout: a fixed `100dvh` shell,
   stationary masthead/back navigation/human identity/tab strip, and one tabbed
   window filling remaining height. Only the selected panel scrolls, including
   on narrow screens. Do not replace it with stacked windows or body scrolling.
4. Lead with recognizable names, boss identities and schedule context, not IDs.
   Preserve useful table density and relationships on narrow screens.
5. Reuse existing visual treatments for buttons, forms, tabs, statuses, themes,
   portraits and icons. Critical state must have text/semantics, not color alone.
6. Preserve responsive behavior, keyboard access, visible focus, readable
   contrast and reduced-motion support. Improve accessibility without an
   unrelated redesign; record any necessary visual deviation.

## PWA behavior

- Replace Jinja/HTMX/SSE with the authenticated JSON client and bounded polling;
  keep all mutations on shared Rust application services.
- Preserve input after failures and focus after navigation/dialog actions.
  Include loading, empty, denied, stale/offline, error/retry and destructive
  confirmation/recovery states in the existing visual language.
- Separate public read-only schedule/OAuth and admin capabilities by server
  authorization, not hidden controls. Never cache private API responses or
  credentials in the service worker; cache only explicitly safe app assets.
- Service-worker-disabled operation remains supported. Legacy server-rendered
  no-JavaScript behavior does not imply a no-JavaScript PWA requirement.

## Migration acceptance

Before shell implementation, extend this guide with an extracted token/component
map and representative reference captures. Compare v4 and v5 with matching
synthetic content, themes and narrow/wide viewports. Exercise keyboard/focus,
panel scrolling, failed forms, offline recovery and service-worker updates.
Record intentional differences and rendered evidence; source inspection alone
does not establish visual parity. New PWA states must look native to Kanade.

## Token and component map (PWA addendum)

The extracted map lives with the code in `web/packages/tokens/README.md`:
every v4 token group, its v5 status, and each `@kanade/ui` component against
the v4 partial it ports. Summary of deliberate differences found while
choosing the stack (`docs/v5/pwa-stack.md`):

- Contrast: v4's `--win` (blossom, periwinkle, coral), `--dim` (marigold,
  periwinkle, all dark faces), `--faint`, two light pill fills, and signal
  colours used as small text fall below 4.5:1. v5 corrects them in one file,
  `web/packages/tokens/src/_contrast.scss`, and adds `--*-text` variants so
  borders and fills keep their v4 values. Approved by the user (2026-09-24).
- Opacity-as-quietness (unselected tabs at 72%, finished runs at 62%, eyebrows
  mixed into title bars) is replaced by shape and surface cues.
- The fixed `100dvh` frame and single scrolling panel apply at every width; v4
  let phones scroll the body and stacked tab panels.
- No inline `style` anywhere (CSP): board track sizing and swatch colours move
  to classes; the nap SVG's `<style>` moves to component CSS.
- Reference captures are generated, not committed: `cd web && bunx playwright
  test capture` writes placeholder-art captures to `web/e2e/.captures/synthetic/`;
  `KANADE_REAL_ART=1` writes real-art ones to `web/e2e/.captures/real/`.
  A v4-vs-v5 comparison with matching synthetic content is still outstanding.

## Experiments

Revertible design trials inspired by Material 3 Expressive and fitted to this
theme. They are under review and on by default. One switch controls them all;
with it off, every usage site renders exactly what it did before.

- **Switch:** `?experiments=off` (or `=on`) on any admin URL, remembered in
  `localStorage` as `kanade.experiments`; or the palette command "Turn design
  experiments off/on". The state is mirrored on `<html data-experiments>`.
  Code: `web/packages/ui/src/experiments/experiments.svelte.ts`, called from
  `apps/admin/src/main.ts`; the command lives in `App.svelte`.
- **A: morphing loading indicator** (`LoadingIndicator.svelte`, wrapped for
  buttons by `PendingLabel.svelte`). Used for short waits (under ~5 s): the
  Pings, Chatbot and Self-service Save buttons, Reload profiles, sign-in
  (token and Tailscale), inbox Approve, Move & approve and Reject change. It
  morphs circle → pill → rounded square (our own radius scale) while it turns.
  Shape changes use the M3 standard spatial spring as a CSS `linear()` curve.
  The hidden label keeps the button's width, and the button's name becomes
  "Saving…" and so on. Reduced motion shows a still rounded square.
  Revert: delete both components, replace each `<PendingLabel …>X</PendingLabel>`
  with `X`, and drop the `saving`/`rejecting`/`via`/`signingIn` flags.
- **B: wavy progress** (`WavyProgress.svelte`) on the rescan job in
  `extractions/RescanPanel.svelte`: a determinate `progressbar` whose fill is a
  slowly drifting sine wave, then a small gap and a flat track, with round
  ends. It flattens when done and under reduced motion. The path is recomputed
  per frame (SVG attributes, so it is CSP-safe). Revert: delete the component
  and the `{#if job && experiments.on}` block.
- Both experiments share `web/packages/ui/src/styles/_experiments.scss`, the
  exports at the end of `packages/ui/src/index.ts`, the `@use "experiments"`
  line in `admin.scss`, `packages/ui/test/experiments.test.ts` and
  `web/e2e/experiments.spec.ts`. Remove these once neither experiment remains.
  Captures from the e2e spec go to `web/e2e/.captures/experiments/`.
