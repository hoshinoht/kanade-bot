# Admin API used by the v5 PWA (proposal)

Status: every route below is what the PWA calls today against the dev mock
(`tools/pwa-mock`); each is **proposed** — the backend confirms or revises
it. DTO names are from `web/packages/api-types`. Errors are `ApiError`
(`{error, message}`); mutations that touch a run take the week's `version`
and answer `409` with a fresh read when it moved underneath.
Response JSON Schemas (frozen contract, endpoint index): [`api-schemas/`](api-schemas/README.md).

Conventions: `?week=this|next` selects the boss week on week reads; ids in
paths are URL-encoded (runs, inbox, members, fixed, limits, rescan); `PATCH` bodies are
partial. A partial `PATCH` takes one section per request and replaces arrays
whole; unknown or read-only keys are refused with 422.

## Schedule

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/admin/week?week=` | — | `Week` (with `version`) | Hidden done/cancelled runs are filtered client-side. |
| `GET /api/admin/stats?week=` | — | `Stats` | Answers chart. |
| `GET /api/admin/summary` | — | `Summary` | Now tiles + nav Inbox pip + model tile. |
| `GET /api/admin/members` | — | `MemberRow[]` | One list for the Members page, filter lists and roster adds (`MemberRow` extends `Member`). |
| `GET /api/admin/channels` | — | `Channel[]` | Filter lists, digest channel picker. |
| `GET /api/identity` | — | `Identity` | Masthead, login window. |
| `GET /api/admin/session` | — | `Session` | Who is signed in. |
| `POST /api/admin/runs/{id}/move` | `{day, time, version}` | `MoveResult` (`{run, previous, version}`) | Planner + keyboard moves; undo is a second move. |
| `PATCH /api/admin/runs/{id}/status` | `StatusRequest` (`{status, version}`) | `RunResult` (`{run, version}`) | |
| `POST /api/admin/runs/{id}/rsvp` | `RsvpRequest` (`{member_id, answer, version}`) | `RunResult` | |
| `PATCH /api/admin/runs/{id}/participants` | `ParticipantsRequest` (`{add?, remove?, version}`) | `RunResult` | Week-only roster edits. |
| `POST /api/admin/runs/{id}/reset` | `{version}` | `RunResult` | New in v5: back to the weekly timing. |
| `POST /api/admin/rescan` (per-run) | `{channels: [run.channel_id], window: 'week'}` | `RescanJob` | The run sheet and phone board re-read one channel; `Run.channel_id` is the explicit key (`party` is the legacy handle). |
| `POST /api/admin/runs/{id}/ping` | `{}` | `{message}` | Preview ping text. |

## Fixed timings, bosses, knowledge

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/admin/fixed` | — | `FixedRow[]` | |
| `POST /api/admin/fixed` | `FixedRequest` | `FixedRow` | `decisions` maps amended-run ids to `update`/`keep`. |
| `PATCH /api/admin/fixed/{id}` | `FixedRequest` | `FixedRow` | Same `decisions` for the update-or-keep step. |
| `DELETE /api/admin/fixed/{id}` | — | `{cancelled}` | Retire; names how many upcoming runs cancel. |
| `POST /api/admin/validate/bosses` | `{text}` | `ValidateResult` | Debounced bosscheck. |
| `GET /api/admin/bosses` | — | `BossRow[]` | |
| `GET /api/admin/bosses/events` | — | `EventBoss[]` | New in v5. |
| `GET /api/admin/bosses/{key}/knowledge` | — | `Knowledge` | Schema v2, served from `boss/knowledge/*.yaml`. |

## Kanade (inbox, extractions, chat, limits)

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/admin/inbox` | — | `Proposal[]` | Extractions and self-service requests together. |
| `POST /api/admin/inbox/{id}/approve` | `{day?, time?}` (moves) | `{message}` | Optional edit before approving. |
| `POST /api/admin/inbox/{id}/reject` | `{}` | `{message}` | Confirm in UI. |
| `GET /api/admin/extractions` | — | `Extractions` | Paged client-side. |
| `GET /api/admin/extractions/{id}` | — | `Extraction` | Tabs: changes, chat read, prompt, raw. |
| `GET /api/admin/rescan/targets` | — | `Channel[]` | Watched channels only. |
| `POST /api/admin/rescan` | `{channels[], window}` | `RescanJob` | `window`: `week`, `since_reset`, `two_weeks`. |
| `GET /api/admin/rescan/{id}` | — | `RescanJob` | Polled per channel. |
| `DELETE /api/admin/rescan/{id}` | — | `RescanJob` | Cancel. v4 `POST …/cancel`. |
| `GET /api/admin/chat` | — | `Chat` | |
| `GET /api/admin/chat/{id}` | — | `ChatTurn` | |
| `GET /api/admin/limits` | — | `Limits` | See `limits-contract.md` (**proposed**). |
| `DELETE /api/admin/limits/windows/{id}` | — | `{message}` | Clear one member's window. v4 `POST …/reset`. |

## Operate (members, reminders, config, history)

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `PATCH /api/admin/members/{id}` | `MemberPatch` | `MemberRow` | Ping level, reply style. |
| `POST /api/admin/members/{id}/aliases` | `{alias}` | `MemberRow` | v4 `…/nick`. |
| `GET /api/admin/personas` | — | `Persona[]` | Reply-style picker. |
| `GET /api/admin/reminders` | — | `Reminders` | `?run=` narrows client-side. |
| `GET /api/admin/config` | — | `ConfigView` | All runtime settings, the Manage-Messages banner list, the env-only table, and `notices` (empty on GET). |
| `PATCH /api/admin/config` | One section, partial (see `ConfigPatch`) | `ConfigView` + `notices` | One section per save; arrays (`countdown_minutes`, `role_profiles`, `groups`) are replaced whole. Runs the startup capacity check: nothing that would stop the bot is saved. A stranded reasoning level (an alias or extraction change invalidating a role that was not part of the request) is reset to `off` and reported in `notices`, e.g. `"chat reasoning reset to off: kanata/chat does not publish high."` Reasoning resolution: `""` inherits the extraction role's effort; inherit is legal only while that effort is `off` or published for the alias; a model that decides takes `low`/`medium`/`high`. |
| `POST /api/admin/config/profiles/reload` | `{}` | `{message, reloaded}` | **Proposed**. Re-reads `config/personas/profiles/` after a file edit. Profile text is files-only by decision (a deliberate v4 drop); the app shows profiles read-only and only publishes them or assigns them to roles. |
| `POST /api/admin/digest` | `{week, channel_id?}` | `{message}` | v4 `POST /digest` parity, channel override included. |
| `GET /api/admin/access` | — | `AccessReport` | v4 `GET /access`. |
| `POST /api/admin/access/recheck` | `{}` | `AccessReport` | v4 `POST /access`. |
| `GET /api/admin/history?week&actor&before&limit` | — | `HistoryPage` | `before` is a seq cursor; `next_before` pages older. **Proposed** (`docs/v5/history.md` is authoritative for the format). |
| `GET /api/admin/history/{seq}` | — | `ChangeRecord` | **Proposed**; the PWA reads records from the page list and does not call this yet. |
| `POST /api/admin/history/revert` | `{seqs[], force?, preview?, request_id?}` | `RevertPlan` | **Proposed**. |
| `POST /api/admin/history/restore-week` | `{week, revision, force?, preview?, request_id?}` | `RevertPlan` | **Proposed**. |
| `POST /api/admin/history/revert-actor` | `{actor, week?, force?, preview?, request_id?}` | `RevertPlan` | **Proposed**. |
| `GET /api/admin/history/checkpoints` | — | `Checkpoints` | **Proposed**. |
| `GET /api/admin/runs/{id}/blame` | — | `BlameEntry[]` | **Proposed**. |

## Config semantics

- `ModelInfo.leaves_homelab` fails closed: true when the alias's trust zone
  is `external`, when Kanata publishes no zone (`unknown` or anything
  unrecognised), or when the alias ends in `-cloud` — the Ollama cloud proxy
  reports `local`, so the suffix is what marks it. The PWA re-derives the same
  rule and warns for every such alias, and for a saved alias the catalog no
  longer lists (shown selected as "(not listed)").
- Reasoning: `off` is always legal; otherwise the level must be in the
  alias's `reasoning_efforts`, and `null` there means the model decides
  (`low`/`medium`/`high` accepted, as v4 offered). `""` (chat/rewrite only)
  inherits extraction's effort and is legal only when that effort is legal
  for the role's alias. Validation runs after every role in the request is
  applied, so inheritance resolves against the FINAL extraction effort and
  each role's final alias. A level the request sets — including an explicit
  `""` whose resolved effort is illegal — is refused (422). A stored level the
  request did not set, stranded by an alias or extraction change, is reset to
  `off` and named in `notices`. The admin app sends all three roles and
  resets a stranded inheritor to `off` locally (with a visible note) before
  saving, so its saves never hit the 422.
- Self-service mode (`self_service.mode`) sets how the extractor and the
  chatbot answer a change they detect: `cards_and_link` (default) keeps the ✅
  card and adds a pre-filled deep link for moves the author can make
  themselves; `link_first` sends only the link for those; `cards_only` is v4
  behaviour. `effective_mode` is `cards_only` whenever `public_portal` is
  off. Links carry the run id and proposed time only — no secret; members
  sign in with Discord and the server re-validates everything.
- Capacity: see `limits-contract.md` (per-alias admission rule and DTOs).

## Inbox and log filters (built against the mock)

- `GET /api/admin/inbox` items carry `tab` (`extractor` | `self_service`),
  `version`, `flags` (`conflict`, `expired`, `requester_frozen`,
  `no_effect`), `preview` (`{no_effect, changes[{field, from, to}],
  conflicts[{field, expected, found}]}`), `expires_at`, `choices` (weekly
  timings: `[{run_id, label, when, amended}]`) and `public_summary`.
  **Proposed**; the admin request routes below are the backend's names for
  the same operations.
- `POST /api/admin/inbox/{id}/approve` `{version?, day?, time?, choices?,
  force?}`: 409 `stale` (version moved) | `conflicts` (not `force`d after
  review) | `no_effect`; 410 `expired`; 422 `choices_required` |
  `choices_not_applicable`. Edit-then-approve sends `force` only when the
  admin ticked the reviewed-conflict box.
- `POST /api/admin/inbox/{id}/reject` `{version?, reason?}`: member requests
  need a reason of 1–500 characters (422 `reason_required` |
  `reason_invalid`); extractor proposals keep v4's reason-free reject.
- `GET /api/admin/chat` and `GET /api/admin/extractions` take `model`,
  `from`, `to` (guild-local `YYYY-MM-DD`, inclusive), `outcome`
  (comma-separated, any of), `channel`, `member`, `q`; Chat also `tool` and
  `min_ms`. Responses add `total` (unfiltered) and `facets` (`models`,
  `tools`, `outcomes`, `channels`). Outcomes — Chat: `answered`, `refused`,
  `clarified`, `error`, `timeout`, `rate_limited`, `turned_away`,
  `content_blocked`, `withheld`, `clean_retry`; Extractions: `proposed`,
  `no_change`, `failed`, `turned_away`, `content_blocked`,
  `self_service_link`. An unknown outcome, a malformed date, a `min_ms`
  that is not whole non-negative milliseconds (`1e3`, `-5`) or a Chat-only
  filter on Extractions is 422 `invalid_filter`, never a bare 400. Cursor paging
  (`cursor`, `next_cursor`) is **proposed**; the mock returns every match.
  Backend requirement: both logs persist the model alias per request round,
  reasoning level, the typed outcome, guardrail signals, tools used, request
  count and latency, indexed for these filters; v4-imported history maps to
  these outcomes where it can, else `unknown`.

## Contract deltas (batch 6, all proposed)

### Edit preconditions

- Run and weekly-timing reads carry `versions`: `{<field>: <seq>}`, read in
  the same transaction as the values they describe.
- Writes take optional `expect: [{field, seen}]` (the version the editor
  saw per field) and an admin-only `override: [{seq, hash}]` naming the
  change records the admin reviewed and chose to overwrite.
- Errors: 409 `stale_edit` with `conflicts: [{field, seen, current, by:
  {seq, hash, actor, at}}]`; 422 `unknown_field` | `duplicate_field` |
  `override_not_seen` | `override_unchanged` | `unknown_override`; 404
  `unknown_target`; 403 `override_forbidden` (member callers).
- UI (later batch): a conflict dialog — keep theirs / apply anyway (sends
  `override`) / save as draft.

### Member requests

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `POST /api/public/requests` | `{run_id \| fixed_id, change, note?}` + `Idempotency-Key` | `MemberRequest` | Signed-in member; a replayed key returns the first result. |
| `POST /api/public/requests/{id}/withdraw` | `{}` | `MemberRequest` | Own requests only. |
| `GET /api/public/requests/mine` | — | `MemberRequest[]` | |
| `GET /api/admin/requests?state=` | — | `MemberRequest[]` | The Inbox's Self-service tab. |
| `GET /api/admin/requests/{id}/preview` | — | `{version, preview, choices?}` | Same `preview` shape as the inbox. |
| `PATCH /api/admin/requests/{id}` | edit ops `[{op, field, value}]` + `version` | `MemberRequest` | Edit before approving. |
| `POST /api/admin/requests/{id}/approve` | `{version, choices?}` | `{message}` | `choices` required for `change_fixed`. |
| `POST /api/admin/requests/{id}/reject` | `{version, reason}` | `{message}` | Reason 1–500 characters. |

Errors: 409 `stale` | `conflicts` | `no_effect`; 422 `choices_required` |
`choices_not_applicable` | `reason_required` | `reason_invalid`; 410
`expired`; 429 `request_limit`. Member-facing routes collapse every
not-yours, unknown or forbidden case into one uniform not-found response.
`MemberRequest` carries `requester_frozen` and a generated `public_summary`.

### Cherry-pick (provisional — being reworked)

- `GET /api/admin/history/{seq}/cherry-pick?week=` → a `RevertPlan`-shaped
  preview of applying record `{seq}` to another boss week.
- `POST /api/admin/history/{seq}/cherry-pick` `{week, mode, reviewed?}`;
  forcing requires `reviewed` to list the conflicts the admin saw.

### History graph

`GET /api/admin/history/graph?week=&before=&limit=` → `{records: [{seq,
hash, parents, actor, at, summary, refs}], drafts: [{id, base_seq, author,
updated_at}], requests: [{id, base_seq, merged_seq?, state}], checkpoints:
[{seq, file}]}` for the tree view (UI in a later batch).

### Presence

The run-sheet poll doubles as a heartbeat: `GET /api/admin/runs/{id}?presence=1`
adds `viewers: [{name, since}]`, names for admins only (members see a count);
entries expire after two missed polls. UI in a later batch.

## Public

While the portal is closed the public origin serves only the app shell, the
status endpoint and the bot identity (name, avatar) the closed page needs;
`/api/public/*` data and `/art/*` answer `503 {error: 'closed'}`.

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/public/status` | — | `PublicStatus` (`{portal: 'open' \| 'closed'}`) | **Proposed**. The closed page needs nothing else to render. |
| `GET /api/public/week` | — | `PublicWeek` | No names, answers, party or version. `503 {error: 'closed'}` while the portal is off. |
