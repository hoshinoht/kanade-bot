# History

**Purpose:** the change history: who changed what, revert, and backup
checkpoints.

**Boards:** `B_History`, `B_HistoryCk` (pairs: history, history-ck; approved
2026-10-02).

## As built

Code: `apps/admin/src/history/` (`HistoryPage`, `HistoryDetail`,
`CheckpointsPanel`, `RevertDialog`, `ActorName`, `describe.ts`);
`_history.scss`.

- Tabs **Timeline n / Checkpoints**; Week and Who filters as title-bar
  dropdown pills reading e.g. "Week: every week"; filters hidden on
  Checkpoints. `[DR 2026-10-02]`
- Timeline: boss-week headings and event rows on a **single rail** with tag
  chips (backup snapshot anchors; "reverted by #n" / revert markers); one
  active row per week group; no window or document scroll on open.
  `[DR 2026-10-04]`, `HistoryPage.svelte:225`
- Change pane `.side-pane--history` `min(400px, 40vw)` from 900 px
  (below, a modal; the shared single-pane switch, `[DR 2026-10-05]`): compact
  changed-field diffs (`field | was | → | now`), "Show raw JSON" in a modal
  (Chat pattern), Revert… (risk key) and the member-revert box at the pane
  foot, whose "Since" is the single-date picker (`DatePicker mode="single"`).
  `[DR 2026-10-02]`, `[DR 2026-10-04]`
- Checkpoints empty state keeps its wording: "No backups recorded yet" plus
  "No backup directory is configured on this server, so no checkpoint
  anchors the history." `[DR 2026-10-02]`

## Known gaps / Planned

- **Planned:** the populated Checkpoints view (verification card, glyph
  table, Verify again) with backup manifests (A5-9). `[DR 2026-10-02]`
