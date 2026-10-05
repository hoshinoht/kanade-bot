# Run pane and full run sheet

**Purpose:** one run's time, bosses, party and status, and the edits on it
(Move, Swap, Preview ping, Reset to fixed, party, status, notes).
Budget order: time, bosses, party, status > roster edits > card timeline, ids.
`[guide]`

**Boards:** `B_WeekSel` (pane), `HeroSheet` (full sheet, from 840 px),
`HeroPhone` (phone sheet), picker boards `Main`, `P_MoveWidths`,
`P_MoveStates`, `P_MovePhone`. Pairs: week-sel, hero-sheet, hero-phone,
move-pane, move-widths, move-phone.

## As built

Code: `apps/admin/src/RunSheet.svelte` (pane and sheet), `sheet/`
(`MovePicker`, `parseWhen.ts`, `move.ts`, `RunLog.svelte`, `runLog.ts`);
styles `_run-sheet.scss`, `_runs.scss`, `_week.scss` (`.week-pane`),
`_move-picker.scss`.

- **Pane (≥ 840 px):** a side pane in the Week window, width
  `clamp(380px, 32vw, 480px)` (340 px at 840–999), with pill tabs
  **Run / Answers n / Changes**, close, and a pop-out button that opens the
  same run and tab in the full sheet. `[DR 2026-10-04]`
- **Full sheet:** the `Modal` (`flush`, `wide`) is the one window, light
  dismiss only when nothing is unsaved. Identity card (`.runsheet__hero`),
  flush on the modal surface with a line under it: clock in `--fs-hero`
  (`--fs-hero-narrow` on phones; sheet only), day · countdown, bosses with
  portraits and levels, status/answer chips, channel, `#short_id`,
  "View weekly timing" (only with `fixed_id`, to `/fixed?open=`),
  `AnswerBar`; on laptop sheets a three-column grid (clock, identity,
  actions: Move key, Swap, Preview ping, Reset, status); on phones Swap,
  Preview ping and Reset under More actions. Below it a pill tab strip
  **Party / Answers / Cards / Changes** ("Who changed this" on wide); only
  the selected panel scrolls. `[web/AGENTS]`, `[DR 2026-10-04]`
  (`--fs-hero`), `[DR 2026-10-05]`
- **Boss art** (`.run__arts`, `[DR 2026-10-05]`): only inside the identity
  card (and the pane's `.week-pane__art`), never behind text: laptop sheet —
  the backdrop of the actions column, fading into the surface before the
  identity text; pane and phone — a top-right corner (36% × 84 px) fading
  left and down. One picture, or up to three 12° slices with soft seams,
  lead first in run order (the first three bosses that have art); further
  bosses show portraits only. No text halo: `e2e/run-identity-contrast.spec.ts`
  measures every word against the rendered pixels (≥ 4.5:1, clock ≥ 3:1).
- **Move:** `MovePicker` (day strip, time stepper, typed "wed 21:30",
  suggestions). On the phone sheet Move replaces the sheet with Back.
- **Changes:** newest-first run log (summary, avatar · actor · relative time
  · surface · #seq; tap expands before → after; "By field" chip), backed by
  `GET /api/admin/history?run=`. `[DR 2026-10-04]`, `sheet/RunLog.svelte`
- **Countdown bar** in the pane head: fills 24 h → T-1h, then restarts over
  the last hour with T-15m at three quarters. `[DR 2026-10-04]`

## Rules

- **One window** `[DR 2026-10-05]`: the sheet is the modal window; the
  identity card and the tabs are sections of it. No nested `.card`, no stray
  accent band around the identity card (`e2e/run-identity.spec.ts` checks one
  title bar and no `.card` inside the dialog). This departs from
  `HeroSheet`'s separate card + `.win`, which is a whole page, not a modal.
- **Text over art:** contrast comes from where the art sits, never from text
  treatment (halos, shadows).

## Phone

Below 840 px the run opens as the full-screen sheet (`HeroPhone`).

**Planned (2026-10-05):** the pane/sheet switch moves from 840 px to the
shared 900 px breakpoint (`../shell-and-components.md`); the 840–999 px
340 px pane width is revisited with it.
