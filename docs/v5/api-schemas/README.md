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
- Every non-2xx JSON body on both origins is `error.json#/$defs/ApiError`,
  except `429 request_limit`, which adds `limit`
  (`public.json#/$defs/MemberRequestLimit`).
- Successful admin JSON `GET`s (outside `/api/admin/auth/`) carry a strong
  `ETag`; `If-None-Match` naming it answers `304` with no body and the same
  headers.
- `devtools/pwa-mock` validates every endpoint below against these files
  (`src/contract.rs`); the Rust API slices validate against the same files.

## Endpoint → schema

Pointers are `<file>#/$defs/<Name>`.

| Endpoint | Schema |
|---|---|
| `GET /api/identity` (both origins) | `identity.json#/$defs/Identity` |
| `GET /api/public/status` | `public.json#/$defs/PublicStatus` (the admin Config switch `self_service.public_portal`) |
| `GET /api/public/session` | `public.json#/$defs/PublicSession` + `X-Kanade-CSRF` (401 `unauthenticated` when signed out) |
| `GET /api/public/session/avatar` | the member's portrait image (monogram when none), not JSON |
| `GET /api/public/sessions` | `public.json#/$defs/PublicSessions` |
| `DELETE /api/public/sessions/{handle}` | `204`, no body (409 `current_session` for the caller's own session) |
| `POST /api/public/sessions/end-all` | `identity.json#/$defs/SessionsEnded` (ends the caller's session too and clears its cookie) |
| `GET /api/public/auth/discord/start`, `/callback`; `POST /api/public/auth/logout` | redirects / landing page / `204`, not JSON |
| `GET /api/public/week?week=` | `public.json#/$defs/MemberWeek` (absent/`this` or `next`, else 422 `invalid_query` as admin) |
| `GET /api/public/me/allowance` | `public.json#/$defs/MemberAllowance` (the caller's own figures only) |
| `GET /api/public/timings` | `public.json#/$defs/MemberTimings` (weekly timings the caller is on, with ownership) |
| `POST /api/public/timings/{id}/owner` | body `public.json#/$defs/OwnerHandOff`; `public.json#/$defs/MemberTiming` (owner only; fresh sign-in, else 401 `reauth_required`) |
| `POST /api/public/timings/{id}/owner-requests` | `public.json#/$defs/MemberOwnerRequest` (201; 200 for a retry with the same Idempotency-Key) |
| `POST /api/public/owner-requests/{id}/accept`, `/decline`, `/withdraw` | `public.json#/$defs/MemberOwnerRequest` (accept/decline: owner only, accept needs a fresh sign-in; withdraw: requester only) |
| `GET /api/public/runs/{id}` | `public.json#/$defs/MemberRunLink` (the Discord deep link; 404 `not_found` unless the run is in this or next boss week or the caller is or was on it) |
| `PUT /api/public/runs/{id}/answer` | body `public.json#/$defs/MemberAnswer`; `public.json#/$defs/MemberRunResult` (participant only, fresh sign-in; 403 `not_in_run`, 409 `run_closed`/`stale`) |
| `POST /api/public/runs/{id}/move` | body `public.json#/$defs/MemberMove`; `public.json#/$defs/MemberMoveResult` (participant only, current boss week, fresh sign-in; 409 `week_over`/`run_started`/`run_closed`/`stale`, 422 `outside_week`/`in_the_past`/`invalid_time`) |
| `GET /api/public/requests/mine` | `public.json#/$defs/MemberRequests` (the caller's own requests, limits and form choices) |
| `POST /api/public/requests` | body `public.json#/$defs/MemberRequestBody`; `public.json#/$defs/MemberRequest` (201; 200 for a retry with the same Idempotency-Key; fresh sign-in; 429 `public.json#/$defs/MemberRequestLimit` over a limit) |
| `POST /api/public/requests/{id}/withdraw` | `public.json#/$defs/MemberRequest` (requester only, while waiting; 404 `not_found`, 409 `request_closed`) |
| `GET /api/public/bosses` | `bosses.json#/$defs/BossRows` (as admin) |
| `GET /api/public/bosses/events` | `bosses.json#/$defs/EventBosses` (as admin) |
| `GET /api/public/bosses/{key}/knowledge` | `public.json#/$defs/PublicKnowledge` (no `path`, no bullet `detail`; 404 `not_found` for an unknown key) |
| `GET /art/{kind}/{key}` (public origin) | boss art as on the admin origin, signed-in members only (401 signed out, 503 `closed` while closed) |
| `GET /api/admin/session` | `identity.json#/$defs/Session` |
| `GET /api/admin/me` | `identity.json#/$defs/Me` |
| `GET /api/admin/me/sessions` | `identity.json#/$defs/AccountSessions` |
| `DELETE /api/admin/me/sessions/{handle}` | `204`, no body |
| `POST /api/admin/me/sessions/sign-out-others` | `identity.json#/$defs/SessionsEnded` |
| `GET /api/admin/auth/tonight` (no session) | `week.json#/$defs/Tonight` |
| `GET /api/admin/week?week=` | `week.json#/$defs/Week` |
| `GET /api/admin/stats?week=` | `week.json#/$defs/Stats` |
| `GET /api/admin/summary` | `week.json#/$defs/Summary` |
| `POST /api/admin/runs/{id}/move` | `week.json#/$defs/MoveResult` |
| `POST /api/admin/runs/{id}/swap` | `week.json#/$defs/SwapResult` |
| `PATCH /api/admin/runs/{id}/status` | `week.json#/$defs/RunResult` |
| `POST /api/admin/runs/{id}/rsvp` | `week.json#/$defs/RunResult` |
| `PATCH /api/admin/runs/{id}/participants` | `week.json#/$defs/RunResult` |
| `POST /api/admin/runs/{id}/reset` | `week.json#/$defs/RunResult` |
| `POST /api/admin/runs/{id}/ping` | `common.json#/$defs/Message` |
| `GET /api/admin/members` | `members.json#/$defs/MemberRows` |
| `PATCH /api/admin/members/{id}` | `members.json#/$defs/MemberRow` |
| `POST /api/admin/members/{id}/aliases` | `members.json#/$defs/MemberRow` |
| `DELETE /api/admin/members/{id}/aliases/{alias}` | `members.json#/$defs/MemberRow` |
| `GET /api/admin/personas` | `members.json#/$defs/Personas` |
| `GET /api/admin/channels` | `common.json#/$defs/Channels` |
| `GET /api/admin/roles` | `common.json#/$defs/Roles` |
| `GET /api/admin/fixed` | `fixed.json#/$defs/FixedRows` |
| `POST /api/admin/fixed`, `PATCH /api/admin/fixed/{id}` | `fixed.json#/$defs/FixedRow` (request body `FixedRequest`; PATCH requires `version`) |
| `DELETE /api/admin/fixed/{id}` | `fixed.json#/$defs/FixedRetired` |
| `POST /api/admin/validate/bosses` | `fixed.json#/$defs/ValidateResult` |
| `GET /api/admin/bosses` | `bosses.json#/$defs/BossRows` |
| `GET /api/admin/bosses/events` | `bosses.json#/$defs/EventBosses` |
| `GET /api/admin/bosses/{key}/knowledge` | `bosses.json#/$defs/Knowledge` |
| `GET /api/admin/reminders` | `reminders.json#/$defs/Reminders` |
| `GET /api/admin/inbox` | `inbox.json#/$defs/Proposals` |
| `GET /api/admin/inbox/past?before=&limit=` | `inbox.json#/$defs/PastPage` (422 `invalid_query`) |
| `POST /api/admin/inbox/{id}/approve`, `/reject` | `common.json#/$defs/Message` |
| `GET /api/admin/inbox/ownership` | `inbox.json#/$defs/OwnershipRequests` (open, unexpired, oldest first) |
| `POST /api/admin/inbox/ownership/{id}/accept`, `/decline` | `common.json#/$defs/Message` (404 `not_found`; 409 `conflicts` with the rule's words when the request is closed, expired or the requester left the party) |
| `GET /api/admin/extractions?…` | `extractions.json#/$defs/Extractions` (422 `invalid_filter`) |
| `GET /api/admin/extractions/{id}` | `extractions.json#/$defs/Extraction` |
| `GET /api/admin/rescan/targets` | `common.json#/$defs/Channels` |
| `POST /api/admin/rescan`, `GET`/`DELETE /api/admin/rescan/{id}` | `extractions.json#/$defs/RescanJob` |
| `GET /api/admin/chat?…` | `chat.json#/$defs/Chat` (422 `invalid_filter`) |
| `GET /api/admin/chat/{id}` | `chat.json#/$defs/ChatTurn` |
| `GET /api/admin/rewrites?…` | `rewrites.json#/$defs/Rewrites` (422 `invalid_filter`) |
| `GET /api/admin/rewrites/{id}` | `rewrites.json#/$defs/Rewrite` |
| `GET /api/admin/limits` | `limits.json#/$defs/Limits` |
| `DELETE /api/admin/limits/windows/{id}` | `common.json#/$defs/Message` |
| `GET`/`PATCH /api/admin/config` | `config.json#/$defs/ConfigView` (PATCH may add `notices`) |
| `POST /api/admin/config/profiles/reload` | `common.json#/$defs/ReloadResult` |
| `POST /api/admin/digest` | `common.json#/$defs/Message` |
| `POST /api/admin/headers/rewrite` | `202` `common.json#/$defs/Message` (the run continues in the background; `200` when nothing posted this boss week has a header; 409 `ApiError` `rewrite_running`/`rewrite_off`) |
| `GET /api/admin/access`, `POST /api/admin/access/recheck` | `config.json#/$defs/AccessReport` |
| `GET /api/admin/history?week=&actor=&run=&before=&limit=` | `history.json#/$defs/HistoryPage` |
| `GET /api/admin/history/{seq}` | `history.json#/$defs/ChangeRecord` |
| `POST /api/admin/history/revert`, `/restore-week`, `/revert-actor` | `history.json#/$defs/RevertPlan` |
| `GET /api/admin/history/checkpoints` | `history.json#/$defs/Checkpoints` |
| `GET /api/admin/events` | `text/event-stream`, not JSON: a `ready` event `{seq, boot}` (`EventReady`; a new `boot` means the server restarted), then one message per change `{topic, seq}` (`EventHint`, topics `schedule`, `inbox`, `chat`, `extraction`, `rewrite`, `delivery`, `settings`, `rescan`; no data) and `: keep-alive` comments; `429 too_many_streams` at the connection cap |
| any non-2xx | `error.json#/$defs/ApiError` |

## Files

`common.json` (enums, `Boss`, `Tally`, `Member`, `Channel`, `WeekDay`, `Head`,
`Message`, `ReloadResult`, `LogFacets`), `error.json`, `identity.json`,
`public.json`, `week.json`, `members.json`, `fixed.json`, `bosses.json`,
`reminders.json`, `inbox.json`, `extractions.json`, `chat.json`,
`rewrites.json`, `limits.json`, `history.json`, `config.json`.

## Changes since A0

- A4: `fixed.json#/$defs/FixedRequest` (request body; `version` required by `PATCH`).
- A5: `history.json` `RowKey.table` adds `reminders` (records carry reminder
  rows and must keep matching their hash); `common.json` `Surface` adds
  `draft_merge`, `request_merge`, `cherry_pick` (all of `Surface::ALL`);
  `BlameEntry` (since removed with `GET /api/admin/runs/{id}/blame`; the
  run change log is `GET /api/admin/history?run=<id>`).
- A6: `inbox.json` `Proposal.kind` adds the request types
  (`new_fixed`, `change_fixed`, `join`, `leave`, `swap`), `source` adds
  `chat`, `flags` adds `requester_unauthorised`.
- A7 (all optional, so earlier responses stay valid): `extractions.json`
  `Extraction.refusals` (`[{change, code, message}]`), `RescanJob.unread`
  and per-channel `unread`/`errors`, `RescanJob.window` adds `24h`/`48h`
  (bot-started jobs); `ExtractionOutcome` and `chat.json` `ChatOutcome` add
  `unknown` (v4-imported rows). Later, also optional: `RescanJob.started_at`,
  `messages` and `messages_total` (the Re-read card's progress line).
- Serve composition: `identity.json` `Session.method` (`discord` |
  `tailscale` | `token`), sent on `GET /api/admin/session` and every sign-in
  response so the admin app can hide proposal actions for non-Discord
  sessions up front. Optional in the schema so earlier responses stay valid.
- Reasoning floor: `config.json` `ModelInfo.off_allowed` (always sent by the
  server; optional in the schema so earlier responses stay valid). False when
  the alias publishes a `reasoning_efforts` list without `none`; `off` is then
  refused (422) and a stranded level resets to the lowest published level.
- Reasoning variants: `config.json` `ModelInfo.variant_of`/`fixed_effort`
  and the same two fields on `RoleModel` (all optional, sent only for a listed
  `<base>:<level>` alias whose base is listed).
- Names: `GET /api/admin/roles` (`common.json` `Roles`, gateway cache, highest
  first, `@everyone` left out; `[]` offline); `identity.json`
  `Identity.bot_user_id`; `inbox.json` `Evidence.author_id`;
  `extractions.json` `Extraction.messages[].author_id`; `chat.json`
  `ChatRow`/`ChatDetail` `member_id`. All optional in the schema so earlier
  responses stay valid; the server always sends them.
- Consequence: `inbox.json` `Proposal.consequence` (one line on what
  approving does, or `null`). Optional in the schema so earlier responses
  stay valid; the server always sends it.
- Sign-in strip: `week.json#/$defs/Tonight` for `GET /api/admin/auth/tonight`
  (owner decision 2026-10-04): today's next run as time, boss names and the
  aggregate tally only, answered without a session.
- Rewrites log: `rewrites.json` (`Rewrites`, `Rewrite`) for
  `GET /api/admin/rewrites` and `/api/admin/rewrites/{id}`; the events stream
  adds topic `rewrite`.
- Member portal identity (2026-10-08): `public.json` (`PublicStatus`, moved
  from `identity.json`; `PublicSession`, `PublicMember`, `PublicSessions`,
  `PublicSessionRow`) for the public sign-in and devices routes, frozen
  ahead of the server and mock (the member realm lands with
  `member-portal/identity-contract`). The anonymous `PublicWeek` and its
  `PublicRun` are deleted from `week.json`: signed-in members read
  `MemberWeek`.
- Member reads (2026-10-09): `public.json` `MemberWeek`, `MemberRun` and
  `MemberAllowance` for `GET /api/public/week` and
  `GET /api/public/me/allowance`; boss art is served on the public origin
  behind the member session.
- Ownership requests (2026-10-10): `inbox.json` `OwnershipRequest` and
  `OwnershipRequests` for the Inbox Ownership tab
  (`GET /api/admin/inbox/ownership`, accept/decline); `week.json`
  `Summary.inbox` now counts open ownership requests too.
- Weekly-timing ownership (2026-10-10): `public.json` `MemberTimings`,
  `MemberTiming`, `MemberOwnerRequest` and the `OwnerHandOff` body for
  `GET /api/public/timings` and the member's hand-off, ask, accept,
  decline and withdraw writes.
- Member boss guides (2026-10-10): `public.json` `PublicKnowledge` (and
  `WithoutDetail`) for `GET /api/public/bosses/{key}/knowledge`; the
  public list and event bosses reuse `bosses.json` `BossRows` and
  `EventBosses`.
