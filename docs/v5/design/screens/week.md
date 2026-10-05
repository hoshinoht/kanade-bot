# Week

**Purpose:** what is on this boss week and next, who has answered, and move
runs. The landing page (`/`, `/?week=next`). Primary surface: the board.

**Boards:** `WeekRail1` (1280, collapsed rail), `WeekRail2` (1440, expanded),
`B_WeekSel` (run selected), `B_WeekRuns`, `B_WeekAnswers`, `B_PhoneWeek`.
PNGs in `docs/research/2026-09-28-m3e-mockups/png/`. Pairs: week-sel,
week-runs, week-answers, phone-week.

## As built

Code: `apps/admin/src/pages/WeekPage.svelte`, `week/` (`Glance`, `RunsTable`,
`AnswersView`, `Filters`, `progress.ts`, `waiting.ts`), `planner/`
(`Planner`, `PlannerCard`, `PlannerColumn`, `pointerDrag.ts`,
`keyboardMove.ts`, `dropTime.ts`), store `store.svelte.ts`; styles
`_week.scss`, `_board.scss`, `_runs.scss`, `_week-head.scss`, `_week-rail.scss`.

- **One window** (G3, `[DR 2026-10-01]`), `WeekPage.svelte:178`: title-bar tabs
  **Planner / Runs n / Answers n**, then This week / Next week, Filters (n),
  ⓘ How to move runs, Undo (icon, disabled until there is a move), Refresh.
  Phones: This/Next as a `.seg` under the bar with the count; Filters as an
  icon.
- **Board:** seven columns from the reset day; empty days collapse to dashed
  40 px columns; busy columns keep a **230 px floor** so the board scrolls
  sideways (Tue onwards start off-screen at 1280). `_board.scss`, `[lesson]`
- **Side pane:** no selection → **Glance** (from 1200 px; below, its facts fold
  into the footer, O5 `[DR 2026-10-01]`): Next up card + that run's party with
  each answer and an unanswered hint; the week-wide waiting list lives only in
  the Answers tab and footer `[DR 2026-10-04]`. Run selected → run pane
  (`screens/run-sheet.md`).
- **Footer:** run count, "n hidden · Show them", removable filter chips, next
  run (when no Glance), boss-week progress bar ("Day 5 of 7 · resets …", flat
  on phones). `[DR 2026-10-04]`
- **Runs tab:** one table (`RunsTable.svelte`), row click opens the same pane.
- **Answers tab:** answers by day (bars with "Show as table") + still waiting
  by member (`AnswersView.svelte`, loaded with its tab).

## Planner interaction (as built, `[web/AGENTS]`)

- Whole card is the drag source (6 px travel or 250 ms long press on touch)
  with a small grip; click/tap opens the run. Keyboard: focus a card, `M` to
  lift, arrows move, Up/Down step by Run lengths, Shift+Up/Down jump next to
  neighbours, `S` swaps, Enter drops, Escape cancels.
- Drops set the time (`dropTime.ts`): after the run above, before the run
  below at a day's top; own-time runs keep none; ghost shows "→ HH:MM"; a
  shared-member overlap is a clash shown with icon and words (still saves).
  Drop on a card swaps (one atomic change). `[DR 2026-10-01]`
- A card grows on hover (mouse, ≥ 900 px) and keyboard focus, not on click;
  growth pushes the cards below down (never overlays); selected card art and
  left mark are clipped to the rounded card. `[DR 2026-10-04]`
- Clicking the open run's card (or Runs row) again closes the pane, focus back
  on the card; opening the pane scrolls the board to keep the card in view.
  `[DR 2026-10-04]`
- Pointer engine hydrates on idle or first pointer-over; keyboard never waits.

## Phone

`week-window--phone`: the seven-cell week rail (`WeekRail`) sticky at the top
of the board panel, then the run list; run opens the full-screen sheet.
`[DR 2026-10-01]` (G7, O1)

## Budget

Board ≥ 55 % of a 1280×800 viewport and never shorter than v4's board
(`layout.spec.ts` "week: the board sits under the window title bar").
`[guide]`

## Known gaps / Planned

- **Planned:** live-update arrival motion for remote changes (FLIP exists for
  own and polled moves; SSE hints are backlog). `foundations.md`
- Pointer and press e2e must scroll the board first (`showWeekEnd`) or open
  the pane; never assume a day column is in view. `[lesson]`
