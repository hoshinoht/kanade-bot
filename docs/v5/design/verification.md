# Verification: fidelity, captures, specs, approval

Commands run from `web/`. Citation keys: `README.md`.

## Check order per change

1. `bun run lint`, `bun run check`, `bun run test`, `bun run build`.
2. Only the touched e2e specs while iterating (screens/layout/shell, plus
   ops/a11y when Inbox or the top bar change); one full `bun run e2e` at the
   end of a step, on the merged tree with no other lane running.
   `[DR 2026-10-02]`, `[lesson]`
3. `bun run leak` (bundle leak + no `data-fid` in any bundle) before a commit
   or deploy. `e2e/bundle.spec.ts`
4. Real-art fidelity pairs and captures for the user's approval (below).

e2e serves the built `dist`: run `bun run build` first, or stale bundles fail
new specs. A failing motion, a11y or row-expansion test under heavy load is
rerun alone before it counts as a regression. `[lesson]`

## Specs that guard the design

| Spec | Guards |
|---|---|
| `e2e/layout.spec.ts` | area budget at 1280×800, 1000×670, 1280×600, 390×844, 844×390: primary surface ≥ max(55 % of height, 360 px when height ≥ 655); the document never scrolls; Week board ≥ v4's board |
| `e2e/tokens.spec.ts` | contrast pairs and hover ΔE in every colourway × light/dark (`foundations.md`) |
| `e2e/a11y.spec.ts` | full axe walk in marigold light + dark; `color-contrast` on eight screens in every other look `[DR 2026-10-04]` |
| `e2e/shell.spec.ts` | rail, expanded rail memory, phone top bar, drawer focus trap, swipe, page line |
| `e2e/clipping.spec.ts` | text clipping on every screen and main state at the five frames (below) |
| `e2e/phone-fit.spec.ts` | specific clipping/overlap checks on phones (Inbox heading tags, Members name cells, Fixed editor grid, Chat rows, Extractions bars) |
| `e2e/progress.spec.ts` | bar placements, two-wave cap, reduced motion flat |
| `e2e/motion.spec.ts`, `experiments.spec.ts` | motion and the Experiments switch |
| `e2e/select*.spec.ts`, `date-picker.spec.ts`, `move-picker.spec.ts` | pickers |
| `e2e/bundle.spec.ts` | public/admin split; no `data-fid` |
| CSP fixture (`e2e/support.ts`) | every flow ends with zero CSP/TT reports |

## Measuring text in the app

Board text is measured with `render/measure.mjs` (`boards.md`). In the app,
add an assertion for every new text-bearing region that can clip, in the
style of `e2e/phone-fit.spec.ts`: compare `scrollWidth`/`clientWidth` and the
text's range rect against its container at each frame size, including 200 %
text where relevant.

**General text-clipping check (built 2026-10-05, user decision 2026-10-05):**
`e2e/clipping.spec.ts` visits every screen in `e2e/frames.ts` (the
`layout.spec` list) plus the main states (each Config section, Inbox
Self-service and an open item, Week Runs / Answers tabs and an open run) at
the five frames (1280×800, 1000×670, 1280×600, 390×844, 844×390), one test
per screen (`mode: 'parallel'`), one in-page audit per frame
(`e2e/text-audit.ts`, the app's counterpart of `measure.mjs`). For every
visible element with its own text it reports:

| Kind | Means |
|---|---|
| `clip-x` / `clip-y` | its content is wider (`scrollWidth`) or its glyphs taller than its own box under `overflow: hidden/clip` |
| `spill` | its text runs past its own box (`overflow: visible`) |
| `cut` / `hidden` | its text is partly / wholly outside a clipping ancestor that does not scroll |
| `off-screen` | its text runs past the viewport horizontally |
| `ellipsis` / `ellipsis-bare` | cut on purpose (`text-overflow: ellipsis` or a line clamp) with / without a `title` or `aria-label` holding the full text |

Vertical checks use the glyphs' em box, not the font's taller content area,
so tight line heights (clocks, mono numerals) do not count. Skipped:
`display: none`, `visibility: hidden`, opacity 0, `[hidden]`,
`aria-hidden`, `.vh` and other `clip-path: inset(50%)` / absolute 1 px boxes,
form fields, SVG, and text inside a scroll container (`overflow:
auto/scroll`: the scrolling panel, sideways tab strips, the Week board).
Absolute and fixed boxes are only clipped by their containing block.

Every finding fails, **ellipses included**, unless the spec's one `ALLOW`
list names it: selector (matched on the element or any ancestor), screens,
sizes, kinds and a reason. A positive-control test proves each kind is
caught. It runs with placeholder art (layout only). Allowed today:

| Selector | Where | Why |
|---|---|---|
| `.skip` | everywhere | the skip link waits above the frame until focused |
| `.knowledge-hero__meta` | Bosses knowledge header | one-line header (DR 2026-10-02): the source path gives way; titled |
| `.knowledge-aside__others` | Bosses knowledge aside | a timing's other bosses give way to its time and pill; titled |
| `.modelstats__chips` | Chat at 1000×670 | a chip that does not fit is dropped, not cut; "+n models" lists all |
| `.row-content__compact` | Inbox, History rows | one-line row summary; the opened row shows it in full |
| `.extract-row__facts` | Extractions rows | one-line call facts; the opened call shows them |
| `.chat-row__q` | Chat rows | titled question; the opened turn shows it |
| `.pageline__context` | Config at 1000×670 | the page line's context gives way first (`_page-line.scss`) |
| `.settings__problem` | Config at 1000×670 | the chip links to Channel access; its hint gives way |
| `.profile__open` | Config › Persona | the prompt preview is the button that opens the prompt |
| `.boss-stack__names`, `.chips .name` | Fixed | titled names |
| `.proposal__threadhead .proposal__threadfact` | Inbox | the thread head's channel and time span give way to Used / All; each message shows its time |

A new finding is fixed in CSS first; allow it only with a reason a screen
spec or the user backs. Per-region assertions (above) still suit 200 % text
and states the spec does not open.

## Fidelity lanes (board vs app)

- Tag app regions with literal `data-fid="<name>"` on native elements, the
  element whose box matches the board's; nothing (styles, tests, queries) may
  depend on a tag. `[DR 2026-10-02]`
- The Vite plugin `stripFidelityTags()` (`packages/ui/src/vite/index.ts`)
  removes tags unless `KANADE_FIDELITY=1`; never deploy a tagged build.
- `bun run fidelity [pair …] [--keep]` (`scripts/fidelity.ts`) builds tagged,
  runs `e2e/fidelity.spec.ts`, rebuilds clean (`--keep` skips the clean
  rebuild while iterating; rebuild before anything else). Reports per pair in
  `e2e/.captures/fidelity/<pair>/` (`report.md`, `composite.png` board left,
  app right, findings outlined). It reports; it never fails on findings.
- `KANADE_MOCKUPS=…/docs/research/2026-10-04-picker-mockups` for picker pairs.
- Pairs (`PAIRS` in `e2e/fidelity.spec.ts`, 2026-10-05): inbox-self,
  inbox-extractor (`VarRail2`), inbox-empty, phone-inbox, history,
  history-ck, fixed, bosses, members, cfg, cfg-persona, cfg-models, cfg-roles,
  week-sel, week-runs, week-answers, phone-week, reminders, limits, extract,
  extract-prompt, chat, chat-trace, states-error, phone-nav, hero-login,
  hero-phone, hero-sheet, dates-range, dates-since, dates-phone, move-pane,
  move-widths, move-phone, select-bar, select-search, select-multi,
  select-phone. **Planned:** live Limits pairs (`B_LimitsLive` …).
- The project agent `fidelity` (`.opencode/agents/fidelity.md`) measures and
  fixes structure/layout findings before pairs go to the user.
- Fidelity compares geometry and style, not fonts or colour; it does not
  catch clipped text. Measure text separately.

## Captures

| Command | Output | Use |
|---|---|---|
| `bunx playwright test capture` | `e2e/.captures/synthetic/` | automated assertions, layout only; placeholder art |
| `KANADE_REAL_ART=1 bunx playwright test capture` | `e2e/.captures/real/` | anything shown to the user or a reviewer |
| `KANADE_REAL_ART=1 bun run fidelity …` | `e2e/.captures/fidelity/` | board-vs-app pairs |

`KANADE_REAL_ART=1` points the mock at root `boss/` (and shifts e2e ports by
10). Captures are git-ignored. Fidelity captures use still art only.
`[web/AGENTS]`, `[DR 2026-10-02]`

## Ports for parallel lanes

e2e starts its own pwa-mock per worker (`KANADE_E2E_WORKERS`, default 4, max
5): worker i serves admin on base + 2i, public on base + 2i + 1; base 4373
(e2e owns 4373–4382; real art 4383–4392). Parallel worktree lanes set
`KANADE_E2E_PORT_BASE` 100 apart (4473, 4573, 4673) and their own Playwright
`--output` dir. Dev servers default to 4173/4174; the parent's long-running
mock uses 4273/4274: leave it alone; never point a dev server at e2e ports.
`[web/AGENTS]`, `[lesson]`

## Approval flow

1. Board approved by the user (decision register entry).
2. Lane implements in its worktree; tags regions; runs `bun run fidelity` with
   real art; fixes findings.
3. Real-art pairs (board | app) at 1280×800 and 390×844 (plus any other size
   the screen has a board for) go to the user, shown with the browser preview
   tool.
4. User approves or lists failures; repeat. Accepted residual findings are
   recorded with the approval.
5. Only then: full e2e, `bun run leak`, commit (by path, with permission),
   CHANGELOG bullet; deploy only when the user asks.
