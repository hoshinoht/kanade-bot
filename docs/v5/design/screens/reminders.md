# Reminders

**Purpose:** queued, sent and stale reminder deliveries.

**Board:** `B_Reminders` (pair: reminders; approved, committed 0d029f4,
`[DR 2026-10-04]`).

## As built

Code: `apps/admin/src/reminders/` (`RemindersPage`, `ReminderTable`,
`ReminderFilters`, `when.ts`); `_reminders.scss`.

- Page line count; window tabs **Queued n / Sent n / Stale & other n**
  (`RemindersPage.svelte:63`), search, Filters (n) popover (kind, run, member,
  day). Run filter grouped by day with time and queued count
  `[DR 2026-10-04]`.
- One day-grouped table: Fires · In · Kind · Bosses · Run (link to the run on
  Week) · Party · Status; Sent tab shows the sent time instead of In; each tab
  keeps its own scroll position.
- Footer: next reminder (emphasised) and "n of N shown".

## Known gaps / Planned

- The "In" column reads the browser clock. **Planned:** a server-supplied fire
  time / `now` so it follows the server clock (MUST 8). `[DR 2026-10-04]`
- **Planned (not mocked, O9):** a row side pane previewing the Discord card.
