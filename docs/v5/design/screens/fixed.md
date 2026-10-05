# Fixed (weekly timings)

**Purpose:** the weekly timings that materialise runs for this week and next;
add, edit, retire.

**Board:** `B_Fixed` (pair: fixed; approved 2026-10-02, committed 7776fa5).

## As built

Code: `apps/admin/src/fixed/` (`FixedPage`, `FixedEditor`, `snapshot.svelte.ts`);
`_fixed.scss`.

- Page line: count + key **Add a weekly timing** (plus icon).
- List: `When | Bosses | Party` grid rows; selection fills the whole row; flag
  words (amended, created from chat) are not uppercase. `[DR 2026-10-02]`
- Editor: side pane `.side-pane--fixed` `min(420px, 42vw)` from 900 px
  (the shared single-pane switch, `[DR 2026-10-05]`);
  modal below. Bosses picker shows each boss's difficulties **on one line**
  `[DR 2026-10-02]`; typed bosses and Note kept; Day / Time / **Owner** grid
  (Owner: rostered non-bot bossing member; stacked below 900 px)
  `[DR 2026-10-03]`; home channel dropdown; party pick chips; footer
  Retire… | Cancel | Save.

## Phone

Editor as a modal; Day, Time and Owner fit without overlap
(`e2e/phone-fit.spec.ts`).

## Known gaps / Planned

- **Planned (`[DR 2026-10-05]`):** the Time field uses the Move picker's
  `TimeStepper` (steps by Run lengths, PgUp/PgDn ±1 h, wraps at midnight)
  beside the weekday strip; typed entry keeps working. Today it is a plain
  text field.
