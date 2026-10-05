# Bosses and boss knowledge

**Purpose:** the boss catalog and each boss's checked-in knowledge
(`boss/knowledge/*.yaml`, schema v2), including seasonal (event) bosses.

**Board:** `B_Bosses` (pair: bosses, 1280×800 and 390×844, fidelity-bosses).

## As built

Code: `apps/admin/src/bosses/` (`BossesPage`, `BossWorkspace`, `BossGrid`,
`KnowledgePage`, `KnowledgeGuide` and its parts `MissionCard`, `FactTiles`,
`HpBar`, `GuideTabs`, `GuideRows`, `MechanicBlock`, `PhaseCards`,
`StrategyCards`, `SourceList`; pure rules in `guide.ts`; `heroArt.ts`,
`event.ts`); `_boss-knowledge.scss`, `_boss-guide.scss`, `_boss-grid.scss`,
`_bosses.scss`; guide tones `--guide-{red,yellow,green,blue,neutral,risk,safe}`
(+ `-ink`) in `_tokens.scss`.

- List pane 330 px (`.bosses-list`) beside the knowledge detail;
  `/bosses/:key/knowledge` deep link; below 900 px (the shared single-pane
  switch) one at a time, as Chat: a pick pushes a tagged entry and focuses
  the detail; the "← Back to the catalog" button (rail frame) or the top
  bar's back (phone frame) pops it, or replaces a deep link with `/bosses`,
  and focus returns to the boss's link. `[DR 2026-10-05]`
- Knowledge header is one aligned line: title · Lv · researched date · source
  `[DR 2026-10-02]`; aside 240 px with "On this page" over the weekly timings using the boss.
- Seasonal bosses carry a tonal chip "Seasonal boss · Challengers World
  Season N" in the list and the header `[DR 2026-10-02]`.
- Knowledge hero plays a muted looping MP4 over the still poster; still image
  under reduced motion or on error (`heroArt.ts`, the `<video>` in
  `BossWorkspace.svelte`); the service worker bypasses `/art/animated/`
  (`apps/admin/src/sw.ts`) `[DR 2026-10-03]`. Fidelity captures use
  stills only.
- Values come only from the YAML; board `[FROM YAML]` markers are not data.
- The guide column is the tabbed M3E guide (user-approved mockup
  `boss-guides/research-2026-10-05/design/Main.dc.html` / `Phone.dc.html`,
  `[DR 2026-10-05]`), top to bottom: the difficulty switch (Champion/Destiny
  are info-only and outlined; ✓ + "the guild runs it" for timed ones); a
  mission card when the selected difficulty has `mission` (Destiny: series
  and "N of M" from the API's `missions`, numbered stops, or just
  "Mission N" while `missions` is empty; Union Champion: "Rank X" and stops
  labelled by rank letter from `order`, 1 B · 2 A · 3 S · 4 SS · 5 SSS,
  showing only the ranks tracked, so no Verus Hilla A `[DR 2026-10-05]`; the
  current stop is `aria-current="step"`; the difficulty tick carries no
  "solo", which is one of the rules when it applies; the
  modifier says "in your favour"/"against you" in words; needs and rules as
  chips); the lead (`lead`, else `summary`); fact tiles (boss level with
  "entry N", Defence (PDR), Authentic Force for sacred / Arcane Force, party,
  a lone HP total, and `recommended_spec.value`/`basis` as "Recommended");
  the HP breakdown (`HpBreakdown.svelte`, replacing the proportional bar,
  `[DR 2026-10-05]`): a pane titled "HP" with "Total HP" from the `total` row
  only (rounded rows are never summed, so no row means no total line), then
  one row per phase in data order: a dark `--win` label bar with the phase
  name (`Phase {base} ({target})` when the row has `target`, base = `phase`
  before '-', else `Phase {phase}`, so Limbo keeps "Phase 2-1"), plus, only
  for a phase split between targets, that phase's HP (value × count, stored
  decimals kept, t promoted to q at 1000: 3 × 920t = 2.76q; 2 × 1.05q =
  2.10q); a single-target phase shows its value once, on its bar
  `[DR 2026-10-05]`; then one bar per target (`count`) in the difficulty's
  pill colours, equal widths that wrap into rows of equal bars on narrow
  frames, a single target full width. Values stay short as stored; the
  spelled-out form ("1.407 quadrillion") is the `title` and the
  screen-reader text; then the
  difficulty's notes (its `difficulty_notes` letter, `notes`, and a long
  recommendation that has no tile figure).
- Pill tabs (the run pane's `.ptab`, a keyboard tablist: arrows, Home, End)
  Overview / Phases n / Strategies n / Notes n / Sources n; Overview has no
  count and an empty tab is not shown. The open tab is `?tab=` (replaced in
  place, Overview drops it; an unknown or empty tab falls back to Overview).
  Overview: mechanic blocks (ledger, zones, scale; tones always with their
  label), Core rows (old documents without `phases`), Danger rows (risk
  mark), Tips rows (ok mark). Phases: one timeline bar
  (`PhaseTimeline.svelte`, every boss with `phases`, `[DR 2026-10-05]`) of
  equal segments, each the phase `name`, its `tag` under it and its `tone`
  tint (`--guide-<tone>`; untoned segments are plain `--seg-fill`), that
  picks ONE phase: its card (group line, name, tag, items; no position
  number, since names already carry "Phase N" and data may skip phases)
  shows below. Adjacent phases sharing a `group` sit under one bracket
  labelled with the group; `cycle` adds "↻ … · repeats" in words. The bar is
  a tablist (arrows, Home, End move and select; the selected segment has an
  ink ring and the selection fill when untoned). The phase is `?phase=N`
  (1-based, the first drops it; changing the guide tab drops it). Narrow
  (below ~92 px per segment, all phones) the bar is vertical: full-width rows,
  each group under its label with a rule down its side. A boss with one
  phase (Kalos) shows only that phase's card: no bar and no tablist
  `[DR 2026-10-05]`. Strategies: cards with
  three pips + the word for risk and damage need, When, Payoff and the steps
  behind "Show N steps" (`aria-expanded`). Notes: titled rows. Sources: a
  chip per kind with its count, the list (title link, author, kind, fetched,
  updated) and the paraphrase note. Titled items show a bold title over one
  line; plain strings show as text; `detail` is the chatbot's and never shown.
- Below a 760 px wide detail (the 1000 px frame) the timings aside moves under
  the guide; phones stack the cards and scroll the tab strip sideways, and
  the difficulty switch wraps (Kalos has six). Mechanic cards pair up only
  when each gets 320 px; scale bands wrap onto a second line rather than
  squeeze; ledger figures stay on one line and their labels wrap; four zones
  make two pairs on a narrow card. The header line and a timing's other
  bosses ellipsise with a `title` (allowed in `clipping.spec`).
- Review round 2 `[DR 2026-10-05]`:
  - The guide column ends with 40 px of room below its last card on every
    tab (on the columns, since a scroll container's end padding does not
    extend its scroll range); the panel lays guide and aside out as a grid
    row sized to the guide.
  - HP is a disclosure ("HP" button with a chevron, `aria-expanded`), open by
    default; closed, "Total HP" stays in its head when there is a total.
  - The difficulty switch draws no rim on Destiny/Champion; selected, they
    take their plate (`--pill-destiny-bg` / `--pill-champion-bg`). The ticks
    in the catalog keep the Destiny rim.
  - The Phases bar is an M3E connected button group: 2 px apart, 6 px inner
    corners, round outer ends (left/right across, top/bottom when vertical),
    the selected segment a full pill (radius spring, still under reduced
    motion); untoned selected segments take `--accent-fill`, toned ones keep
    their tone with an ink ring.
  - "On this page" (`GuideToc.svelte`, a `nav`) heads the aside, which sticks
    to the top of the panel as one block: the page sections present (Mission,
    Facts, HP, the difficulty's notes) and the guide's tabs. A section entry
    scrolls the panel (never the document; smooth unless reduced motion) and
    focuses the section; a tab entry selects the tab, scrolls to the strip and
    focuses the tab. Scroll-spy marks the entry at the upper third of the
    panel (`aria-current="location"`: fill, start bar, bold). Hidden where the
    aside sits under the guide (below a 760 px detail, phones).
  - The hero collapses to one 52 px line (34 px portrait, name, the selected
    difficulty's pill; art, overline and meta line fade) once the panel is
    scrolled past 96 px and comes back under 12 px (two thresholds, so the
    height change cannot flip it); only when the panel has 160 px more to
    scroll. Classes only; reduced motion switches without the transition.
  - The Recommended tile lists `recommended_spec.parties` as one row per party
    size (label and figure), the basis underneath; `value` is the fallback.
  - Scale band labels keep figures whole (`figureParts`, `white-space:
    nowrap` on the figure, `text-wrap: balance`); a narrow band breaks between
    words only.

## Known gaps / Planned

- **Planned:** port this screen to the public portal (signed-in members only,
  read-only guide + "your runs with this boss this week"); see
  `../../public-portal-plan.md`. `[DR 2026-10-04]`
- No animated art in Discord cards. `[DR 2026-10-04]`
