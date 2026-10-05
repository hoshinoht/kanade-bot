# Admin PWA design specification

The current, as-built design reference for the Kanade v5 admin PWA
(`web/apps/admin`, shared pieces in `web/packages/{tokens,ui}`). It replaces
`../../notes/m3e-rail-design-spec.md` and `../pwa-design-guidelines.md` as the place to
look things up; those stay as history. Written 2026-10-05 from the code at
`d47f78b` plus the user's decision register.

Anything not built yet is marked **Planned**. Everything else describes the
code as approved; if you find the code and this spec disagree, the code wins
until the user says otherwise, and you add the difference to "Drift found".

## Citation keys

| Key | Means |
|---|---|
| `[old §X]` | `../../notes/m3e-rail-design-spec.md`, section X (the 2026-09-28 handoff) |
| `[guide]` | `../pwa-design-guidelines.md` |
| `[web/AGENTS]` | `web/AGENTS.md` |
| `[lesson]` | root `AGENTS.md`, `recall:lessons` block |
| `[DR yyyy-mm-dd]` | a user decision in `.opencode/workplan/kanade-v5-roadmap.md` `## Decision register` (or its JSON notes) |
| a path | the code that implements it |

## Precedence

1. The user's decision register (newest entry wins).
2. This spec.
3. Board source (`*.dc.html`): exact values the spec does not state.
4. Board picture (`png/`): layout intent only.
5. The old specs (`[old §]`, `[guide]`): history and rationale.

`../../notes/public-portal-plan.md` specifies the public app; it is linked here, not
merged (user decision 2026-10-05).

## Read order by task

| Task | Read |
|---|---|
| Designing a board | this README → `principles.md` → `foundations.md` → `shell-and-components.md` → the screen file → `boards.md` |
| Implementing a screen | this README → the screen file → `shell-and-components.md` → `foundations.md` (tokens, type) → `verification.md` |
| Fidelity lane (board vs app) | this README → the screen file → `verification.md` → `boards.md` "Measure" |
| Reviewing a change | this README (MUST list, checklist) → the screen file → `verification.md` |

Files: `principles.md`, `foundations.md`, `shell-and-components.md`,
`boards.md`, `verification.md`, `screens/` (one file per admin screen:
week, run-sheet, inbox, members, fixed, bosses, history, config, reminders,
limits, chat, extractions, login).

## MUST rules

1. **CSP:** no inline `style`, no Svelte transitions, no `{@html}`; dynamic values go through CSSOM (`el.style.setProperty`) or classes. Every e2e flow ends with zero CSP/TT reports. `[web/AGENTS]`
2. **Tokens only:** colour, type size and radius come from `web/packages/tokens` / shared SCSS; never board hex or px. Contrast fixes live only in `_contrast.scss`. `[guide] Colour`, `[old §Tokens]`
3. **Fixed `100dvh` frame, one scrolling panel** at every width: document, page line, title bar, tabs and identity never scroll; only the selected panel does. No stacked windows, no body scroll. `[lesson]`, `_frame.scss`
4. **Fixed windows clip:** `overflow: clip` (not `hidden`) on fixed shells, and programmatic `focus({ preventScroll: true })`; `await tick()` before focusing something a filter just un-hid. `[lesson]`
5. **Area budget:** the primary surface keeps ≥ 55 % of the viewport height (≥ 360 px when the viewport is ≥ 655 px tall) at 1280×800, 1000×670, 1280×600, 390×844, 844×390; supporting rows degrade first (help → tiles → filters → status). `e2e/layout.spec.ts`, `[guide]`
6. **One window per page; window dots once.** `.card__head::before` / `.modal__head::before` draw the three dots; a screen never adds its own. `[lesson]`, `_base.scss`
7. **No window in a window.** A modal sheet is one window: identity card and tabs are sections of it, never a nested `.card`. `[DR 2026-10-05]`
8. **Server clock, never the browser's,** for anything time-relative (`Week.generated_at`, `apps/admin/src/shared/wall.ts`). `[web/AGENTS]`
9. **Two waves max:** at most two `WavyProgress` bars move on a screen; the rest are flat; reduced motion draws all flat. `limits/permits.ts`, `[DR 2026-10-04]`
10. **Real art for every visual judgement:** boards, captures, fidelity pairs and reviews use root `boss/` art (`KANADE_REAL_ART=1`); never placeholders or monograms. Say so if art is missing. `[DR 2026-10-05]`, `[web/AGENTS]`
11. **Invented fixtures only:** names, channels and messages in boards and tests are invented and match the pwa-mock; never copy live identities or transcripts. `[old §The mockups are not code]`, `[lesson]`
12. **Keyboard and focus:** every pointer action has a keyboard route; visible focus; focus returns after dialogs, panes, undo and route changes; lists are `listbox`/tables with arrow keys. `[web/AGENTS]`, `[old §Accessibility checklist]`
13. **AA contrast in every colourway, light and dark** (4.5:1 text, 3:1 rings/borders); `tokens.spec` and `a11y.spec` enforce it. `[guide]`
14. **Never colour alone:** pair colour with text, weight, shape, symbol or ring. `[old §Principles 9]`
15. **Human identity first:** member and boss names lead; ids are secondary. `[guide]`
16. **No bottom nav bar on phones;** phone frame = 48 px top bar + drawer. `[DR 2026-10-01]`
17. **Never ship `data-fid`:** tags survive only with `KANADE_FIDELITY=1`; `bun run leak` checks. `[DR 2026-10-02]`
18. **Reduced motion honoured** by every animation (no morph, no travel, still indicators, still art). `_motion.scss`

## Pre-handoff checklist

- [ ] Every value maps to a token or a shared class; no inline style; no new hex outside `packages/tokens`.
- [ ] Rendered at each frame size the screen supports (1280×800, 1000×670, 1280×600, 390×844, 844×390; boards at their `$preview` size).
- [ ] **Text measured, not eyeballed:** run `render/measure.mjs` on boards and the clipping checks in `verification.md` on the app; zero `clip-x`, `clip-y`, `cut` or `off-board` lines, and every `ellipsis` is one the spec allows.
- [ ] Real boss art in every capture and board shown to anyone.
- [ ] Window dots appear once per title bar; no nested windows.
- [ ] Only the owning panel scrolls; the document never scrolls (`layout.spec`).
- [ ] Keyboard path, focus return, accessible names checked; axe clean.
- [ ] Checked in at least marigold and blossom, light and dark.
- [ ] Planned items not presented as built; new user decisions added to the decision register.

## Drift found (still open)

Resolved items moved into their files with their decision dates: overshoot
stays opt-in, toasts 2 deep / 6 s / 10 s with Undo, the single-pane switch at
900 px and the real 700 faces (all three built 2026-10-05), the phone Limits and HeroPhone board fixes, the
`web/AGENTS.md` Week-window line and the one-window run sheet (all
2026-10-05), coral retired (2026-10-04), and the phone frame query
(2026-10-01).

| # | Where | Old text / decision | Code (as built) |
|---|---|---|---|
| D5 | `[old §Tokens]` `--select-ring` | | Named `--select-edge`, a finish rather than the state cue (`_tokens.scss`). |
| D10 | Board files | boards load `./support.js` | No `support.js` exists locally; `render/runtime.js` stands in for the canvas runtime. |
