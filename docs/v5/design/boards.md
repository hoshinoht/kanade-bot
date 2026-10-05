# Authoring boards

Boards are the design mockups the app is matched against. They live in
git-ignored folders because they embed private boss art:

| Folder | Holds |
|---|---|
| `docs/research/2026-09-28-m3e-mockups/` | every `B_*` screen board, `VarRail2`, `WeekRail1/2`, `Hero*`, `Spec`, rejected explorations; `png/` renders; `render/` tooling; `shapes.css`; `canvas.json` |
| `docs/research/2026-10-04-picker-mockups/` | `Main`, `P_Move*`, `P_Select*`, `P_Dates*`, `P_Faces` (user-drawn pickers); its `render/` is the same tooling |

Rejected or superseded, never build from them: `Main` (in the m3e folder),
`WeekFramed`, `WeekWin1–3`, `PhoneWeek`, `PhoneWin`, `Inbox`, `Members`,
`Reminders`, `VarTonal`, `VarRail`, `VarFull`, `HeroPublic`, `*Before`.
`[old §Visual reference]`, `[DR 2026-10-04]`

## Format

A board is one `<Name>.dc.html` file:

- `<x-dc>` wraps a `<helmet>` (Google Fonts link + one `<style>` block with the
  board's classes) and the markup, with `{{holes}}`, `sc-for` and `sc-if`.
- A `<script type="text/x-dc" data-dc-script data-props='{"$preview":{"width":1280,"height":800}}'>`
  holds `renderVals()` returning the data (names, times, art URLs). `$preview`
  is the artboard: 1280×800 desktop, 390×844 phone; add other sizes as
  separate boards (e.g. `P_MoveWidths`).
- The page head loads `./support.js` (the design canvas runtime). It is not in
  the local folder; `render/runtime.js` stands in for it (README D10).
- Boards use inline styles and literal hex: that is fine in a board and never
  in the app (`foundations.md` maps hex → token).

## Vocabulary to reuse

Copy classes from an approved neighbouring board instead of inventing new
ones; the app's primitives were built from these. `_m3e-primitives.scss`
header names the mapping.

| Board class | App equivalent |
|---|---|
| `.top` | page line (`PageLine.svelte`) |
| `.win`, `.tb`, `.tab`, `.tbtn` | window, title bar, title-bar tab, title-bar pill |
| `.pane`, `.lrow`, `.crow`, `.sel` | list pane, row, contained row, selected row |
| `.seg`, `.key`, `.btn` | connected group, key button, secondary button |
| `.cap` | overline |
| `.tonal2`, `.ok2`, `.err2`, `.ans.wait` | tonal / ok / risk chips, warn outline |
| `.cookie`, `.flower`, `.burst` (`shapes.css`) | `$state-cookie` etc. |
| `.pill.ph/.pn/.pe` | difficulty pills (`--pill-*`) |

- Fonts: Solway, Zilla Slab, Sometype Mono (stands in for Maple Mono).
- Default colourway for `B_*` boards is blossom; older explorations are
  marigold. Draw new boards in blossom; add a light/dark face sheet only when
  the colour itself is the question (`P_Faces`).
- Never draw window dots twice, never a window inside a window, nothing
  legible on the bare ground except the page-line title group.

## Data: invented, matching the mock

- Names, channels and messages are invented and must match what the pwa-mock
  serves (`tools/pwa-mock/src/mock/`: `people.rs`, `seed.rs`, `inbox.rs`,
  `chat.rs`, `limits.rs` …), pinned at `KANADE_MOCK_NOW` (e2e: Tue 29 Sep 2026
  12:00 guild time). A fidelity pair compares the board with the app showing
  mock data, so different data means false findings. `[web/AGENTS]`
- Never copy live screenshots' identities, transcripts or ids.
  `[old §The mockups are not code]`
- Values the API does not supply are not drawn (or are marked as a proposal
  for the user). If a board needs new mock data, say so: the dev-only mock
  seed is a separate approved change (e.g. Limits groups `[DR 2026-10-05]`).

## Real art, never placeholders

- Every board uses the real portraits, icons and entry art from root
  `boss/` (`boss/portraits`, `boss/icons`, `boss/artwork/entry`). Never
  monograms, striped stand-ins or `web/e2e/fixtures/boss`. `[DR 2026-10-05]`
- Art is referenced as `/_blob/<key>`; `render/blobs.json` maps each key to an
  absolute local file. To add art, add a new unique hex key → path entry (the
  existing keys come from the canvas). If an art key is missing under
  `boss/`, say so in the board's note rather than substituting.
- Crops: name plates are baked into many entry splashes; the app hides them
  with `--art-crop-card` / `--art-crop-sheet`. Match those crops.

## Tag regions for fidelity

Tag the board element whose box the app must match with a literal
`data-fid="<name>"`. `[DR 2026-10-02]`

- Fixed strings, on native elements, one tag per element; repeated items on
  the template; missing elements stay untagged and are reported.
- Shared names: `page-line`, `window`, `window-bar`, `window-tabs`,
  `window-filters`, `window-search`; screen names are prefixed
  (`limits-group`, `week-move`, `sheet-hero` …). Phone boards reuse the
  desktop names.
- Lanes may tag their own boards and append their own pairs to
  `web/e2e/fidelity.spec.ts` `PAIRS`; `fidelity-kit.ts` and the runner stay
  untouched. `[DR 2026-10-02]`

## Render

```sh
node docs/research/2026-09-28-m3e-mockups/render/render.mjs \
  docs/research/2026-09-28-m3e-mockups docs/research/2026-09-28-m3e-mockups/png [Board ...]
```

Headless Chrome at 2× (needs `web/` dependencies and local `boss/` art). For
picker boards pass the picker folder as the boards dir. Show renders to the
user with the browser preview tool; do not read PNGs inline in a long
session `[lesson]`.

## Measure (mandatory before handoff)

Do not judge clipping by eye. Run:

```sh
node docs/research/2026-09-28-m3e-mockups/render/measure.mjs <boardsDir> [Board ...] [--all]
```

It renders each board at its `$preview` size with the same runtime and art,
then reports every element holding text that:

| Kind | Meaning | Action |
|---|---|---|
| `clip-x` / `clip-y` | content wider/taller than its box under `overflow` ≠ visible | fix |
| `cut` | text partly cut by a non-scrolling clipping ancestor | fix |
| `off-board` | text past the artboard, not inside any clip | fix |
| `ellipsis` (info) | text ellipsised | allowed only where the screen spec says so |
| `hidden` (info) | wholly past a clip (rows below a list's fold) | check it is meant to scroll |
| `scrolled` (info) | inside a scroll container | fine; make sure the strip really scrolls |

Exit status 1 when any fix-kind line exists. A strip that is meant to scroll
sideways must be authored with `overflow-x: auto` (not `hidden` plus a fade),
or it reports `cut`. Run it at every frame size the screen has a board for;
when a board stands for several widths, draw or duplicate it at each.
2026-10-05: `B_PhoneLimits` and `B_PhoneLimitsAdmission` tab strips were
re-authored to scroll (`overflow-x: auto`, hidden scrollbar, edge fades kept,
`scroll-snap` centring the selected tab so the board opens scrolled to it),
and `HeroPhone`'s party list became the scrolling panel with a 5 px row gap
so "Open slot" fits; all three re-rendered and re-measured with 0 findings
(user decision 2026-10-05). The approved hero-phone pair predates the 5 px
gap: a 1 px-per-row spacing finding there is expected, not a regression.

## Approval

A new board is a proposal until the user approves it; record the approval
(date, boards, decisions) in the decision register. Then a fidelity lane
implements it (`verification.md`).
