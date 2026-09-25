# Admin/public JSON API schemas (frozen contract)

JSON Schema (draft 2020-12) for every response DTO in `web/packages/api-types`,
one file per DTO family. Each file's `$id` is
`https://kanade.invalid/api-schemas/<file>` (a placeholder base, never fetched);
cross-file `$ref`s are relative (`common.json#/$defs/Boss`), so validators must
register every file as an in-memory resource under its `$id`.

- Closed TS interfaces are `additionalProperties: false`; `T | null` fields are
  required and nullable; `?:` fields are optional.
- Tightening over api-types' `number`: counts, days, versions, seqs,
  revisions and latencies are `integer` (≥ 0 where they cannot be negative);
  scores, rates and seconds stay `number`.
- Every non-2xx JSON body on both origins is `error.json#/$defs/ApiError`.
- `tools/pwa-mock` validates every endpoint below against these files
  (`src/contract.rs`); the Rust API slices validate against the same files.

## Endpoint → schema

Pointers are `<file>#/$defs/<Name>`.

| Endpoint | Schema |
|---|---|
| `GET /api/identity` (both origins) | `identity.json#/$defs/Identity` |
| `GET /api/public/status` | `identity.json#/$defs/PublicStatus` |
| `GET /api/public/week?week=` | `week.json#/$defs/PublicWeek` (503 `ApiError` `closed` while the portal is closed) |
| `GET /api/admin/session` | `identity.json#/$defs/Session` |
| `GET /api/admin/week?week=` | `week.json#/$defs/Week` |
| `GET /api/admin/stats?week=` | `week.json#/$defs/Stats` |
| `GET /api/admin/summary` | `week.json#/$defs/Summary` |
| `POST /api/admin/runs/{id}/move` | `week.json#/$defs/MoveResult` |
| `PATCH /api/admin/runs/{id}/status` | `week.json#/$defs/RunResult` |
| `POST /api/admin/runs/{id}/rsvp` | `week.json#/$defs/RunResult` |
| `PATCH /api/admin/runs/{id}/participants` | `week.json#/$defs/RunResult` |
| `POST /api/admin/runs/{id}/reset` | `week.json#/$defs/RunResult` |
| `POST /api/admin/runs/{id}/ping` | `common.json#/$defs/Message` |
| `GET /api/admin/runs/{id}/blame` | `history.json#/$defs/BlameEntries` |
| `GET /api/admin/members` | `members.json#/$defs/MemberRows` |
| `PATCH /api/admin/members/{id}` | `members.json#/$defs/MemberRow` |
| `POST /api/admin/members/{id}/aliases` | `members.json#/$defs/MemberRow` |
| `GET /api/admin/personas` | `members.json#/$defs/Personas` |
| `GET /api/admin/channels` | `common.json#/$defs/Channels` |
| `GET /api/admin/fixed` | `fixed.json#/$defs/FixedRows` |
| `POST /api/admin/fixed`, `PATCH /api/admin/fixed/{id}` | `fixed.json#/$defs/FixedRow` (request body `FixedRequest`; PATCH requires `version`) |
| `DELETE /api/admin/fixed/{id}` | `fixed.json#/$defs/FixedRetired` |
| `POST /api/admin/validate/bosses` | `fixed.json#/$defs/ValidateResult` |
| `GET /api/admin/bosses` | `bosses.json#/$defs/BossRows` |
| `GET /api/admin/bosses/events` | `bosses.json#/$defs/EventBosses` |
| `GET /api/admin/bosses/{key}/knowledge` | `bosses.json#/$defs/Knowledge` |
| `GET /api/admin/reminders` | `reminders.json#/$defs/Reminders` |
| `GET /api/admin/inbox` | `inbox.json#/$defs/Proposals` |
| `POST /api/admin/inbox/{id}/approve`, `/reject` | `common.json#/$defs/Message` |
| `GET /api/admin/extractions?…` | `extractions.json#/$defs/Extractions` (422 `invalid_filter`) |
| `GET /api/admin/extractions/{id}` | `extractions.json#/$defs/Extraction` |
| `GET /api/admin/rescan/targets` | `common.json#/$defs/Channels` |
| `POST /api/admin/rescan`, `GET`/`DELETE /api/admin/rescan/{id}` | `extractions.json#/$defs/RescanJob` |
| `GET /api/admin/chat?…` | `chat.json#/$defs/Chat` (422 `invalid_filter`) |
| `GET /api/admin/chat/{id}` | `chat.json#/$defs/ChatTurn` |
| `GET /api/admin/limits` | `limits.json#/$defs/Limits` |
| `DELETE /api/admin/limits/windows/{id}` | `common.json#/$defs/Message` |
| `GET`/`PATCH /api/admin/config` | `config.json#/$defs/ConfigView` (PATCH may add `notices`) |
| `POST /api/admin/config/profiles/reload` | `common.json#/$defs/ReloadResult` |
| `POST /api/admin/digest` | `common.json#/$defs/Message` |
| `GET /api/admin/access`, `POST /api/admin/access/recheck` | `config.json#/$defs/AccessReport` |
| `GET /api/admin/history?week=&actor=&before=&limit=` | `history.json#/$defs/HistoryPage` |
| `GET /api/admin/history/{seq}` | `history.json#/$defs/ChangeRecord` |
| `POST /api/admin/history/revert`, `/restore-week`, `/revert-actor` | `history.json#/$defs/RevertPlan` |
| `GET /api/admin/history/checkpoints` | `history.json#/$defs/Checkpoints` |
| any non-2xx | `error.json#/$defs/ApiError` |

## Files

`common.json` (enums, `Boss`, `Tally`, `Member`, `Channel`, `WeekDay`, `Head`,
`Message`, `ReloadResult`, `LogFacets`), `error.json`, `identity.json`,
`week.json`, `members.json`, `fixed.json`, `bosses.json`, `reminders.json`,
`inbox.json`, `extractions.json`, `chat.json`, `limits.json`, `history.json`,
`config.json`.

## Changes since A0

- A4: `fixed.json#/$defs/FixedRequest` (request body; `version` required by `PATCH`).
- A5: `history.json` `RowKey.table` adds `reminders` (records carry reminder
  rows and must keep matching their hash); `common.json` `Surface` adds
  `draft_merge`, `request_merge`, `cherry_pick` (all of `Surface::ALL`);
  `BlameEntry.field` uses the domain's blame names (`slot`, `rsvp:<id>`, …).
- A6: `inbox.json` `Proposal.kind` adds the request types
  (`new_fixed`, `change_fixed`, `join`, `leave`, `swap`), `source` adds
  `chat`, `flags` adds `requester_unauthorised`.
- A7 (all optional, so earlier responses stay valid): `extractions.json`
  `Extraction.refusals` (`[{change, code, message}]`), `RescanJob.unread`
  and per-channel `unread`/`errors`, `RescanJob.window` adds `24h`/`48h`
  (bot-started jobs); `ExtractionOutcome` and `chat.json` `ChatOutcome` add
  `unknown` (v4-imported rows).
