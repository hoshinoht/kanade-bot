# History

**Purpose:** the change history: who changed what, revert, and backup
checkpoints.

**Boards:** `B_History`, `B_HistoryCk` (pairs: history, history-ck; approved
2026-10-02).

## As built

Code: `apps/admin/src/history/` (`HistoryPage`, `HistoryDetail`,
`ConfigDetail`, `CheckpointsPanel`, `RevertDialog`, `ActorName`,
`describe.ts`, `settings.ts`);
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
- Config section saves (`[DR 2026-10-05]`) interleave by time with the
  change rows inside the boss-week groups: a square dot, a "Config" chip,
  "Config · Persona — persona, chat_mode" and "n settings"; the Who and
  Week filters apply to them. Opening one shows the same pane (phones: the
  sheet) view-only: "Persona settings saved", the compact field diff (JSON
  rows field by field), "Show raw JSON", a note that saves are outside the
  change chain, and "Open Persona in Config" (`/config?section=`); no
  Revert…, no Restore week, no member-revert box. `ConfigDetail.svelte`,
  `settings.ts`.
- Checkpoints empty state keeps its wording: "No backups recorded yet" plus
  "No backup directory is configured on this server, so no checkpoint
  anchors the history." `[DR 2026-10-02]`

## Known gaps / Planned

- **Planned:** the populated Checkpoints view (verification card, glyph
  table, Verify again) with backup manifests (A5-9). `[DR 2026-10-02]`
