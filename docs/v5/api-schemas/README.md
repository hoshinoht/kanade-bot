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
| `PATCH /api/admin/members/{id}` | `members.json#/$defs/MemberRow` (a retry with the same Idempotency-Key answers the member's current row, writing nothing; another request under it is 422 `idempotency_mismatch`; an effective edit adds a History settings row, section `members`) |
| `POST /api/admin/members/{id}/aliases` | `members.json#/$defs/MemberRow` (Idempotency-Key and History as the PATCH) |
| `DELETE /api/admin/members/{id}/aliases/{alias}` | `members.json#/$defs/MemberRow` (Idempotency-Key and History as the PATCH) |
| `GET /api/admin/members/{id}/characters` | `characters.json#/$defs/MemberCharacters` (module `roster`: every character route answers the generic 404 while `modules.roster` is off; 404 `not_found` for a member never seen) |
| `POST /api/admin/members/{id}/characters` | `201` `characters.json#/$defs/CharacterCreated` (body `CharacterCreate`; the row plus `link`; 422 `invalid`, `ign_taken` (unique per world, case-insensitive, retired included); a member's first active character is the main, and `role: main` moves the crown; a retry with the same Idempotency-Key replays the first answer, another request under it is 422 `idempotency_mismatch`; each write adds one History settings row, section `characters`, a key `character.<id>` per row it changed. With module `character_data` on and a Nexon key, a MapleSEA character's member is asked to link it after the create commits, as `POST …/link` does (pending, no Open API call until they confirm in Discord, its own History row `link.<id>`): `link` `pending`, or `failed` when that request could not be written (the character stays; the separate character-data file makes this best-effort, not one transaction); `null` when none was asked) |
| `PATCH /api/admin/characters/{id}` | `characters.json#/$defs/CharacterRow` (body `CharacterPatch`; 422 `version_required`, 409 `stale` for another version, 409 `main_required` to retire or demote the main while the member has other active characters; refusals, Idempotency-Key and History as the POST; a no-op answers the row and records nothing) |
| `DELETE /api/admin/characters/{id}` | `common.json#/$defs/Message` (removes the row and its run and timing picks; History keeps its fields; 409 `main_required` for the main while the member has other active characters; Idempotency-Key as the POST) |
| `GET /api/admin/runs/{id}/characters` | `characters.json#/$defs/RunCharacters` (module `roster`, generic 404 while off; 404 `not_found` for a run that is gone; one slot per party member with candidates, the run's own `picked`, `brings` and its `basis` (the run's pick, else its weekly timing's pick read live, else the main, else the only active character; recomputed on every read; a read never writes) and `warnings`) |
| `PUT /api/admin/runs/{id}/characters/{member}` | body `characters.json#/$defs/CharacterPick`; `characters.json#/$defs/RunCharacterSlot` (that member's slot as it now reads; saves directly, never staged in a draft, outside the history chain; 400 `invalid_body`, 422 `invalid`/`version_required`, 409 `stale` for another slot version, 409 `run_ended` for a live run past its end, 422 `not_on_run` for a member not in the party, 404 `not_found` for an unknown run or character, 422 `not_their_character`, 422 `character_retired`; a no-op answers the slot and records nothing; Idempotency-Key as the character POST; each effective write adds one History settings row, section `characters`, key `pick.<run>.<member>`) |
| `GET /api/admin/fixed/{id}/characters` | `characters.json#/$defs/TimingCharacters` (modules `roster` and `character_data`: every timing character and readiness route answers the generic 404, before the session check, while either is off; 404 `not_found` for a timing that is gone (retired); one slot per member of the saved party with the timing's own `picked`, `brings` and `basis` (the pick, else the main, else the only active character); `warnings` always empty) |
| `PUT /api/admin/fixed/{id}/characters/{member}` | body `characters.json#/$defs/CharacterPick`; `characters.json#/$defs/RunCharacterSlot` (that member's timing slot as it now reads; saves directly, outside the timing's own PATCH, drafts and the history chain; runs made from the timing read it live unless they have their own pick; 422 `not_on_timing` for a member not in the saved party, 404 `not_found` for an unknown timing or character, other refusals, no-op and Idempotency-Key as the run pick; each effective write adds one History settings row, section `characters`, key `fixedpick.<fixed>.<member>`. A retired timing's picks stay stored but no read uses them) |
| `GET /api/admin/characters/lookup` | `character_data.json#/$defs/LookupStatus` (module `character_data`, generic 404 before the session check while off; `connected` is whether a Nexon key is configured) |
| `POST /api/admin/characters/lookup` | `character_data.json#/$defs/CharacterLookup` (body `CharacterLookupRequest`; the Add character dialog's lookup: `id` + `basic` for a typed MapleSEA IGN, shown only and never stored (no roster field, nothing in the character-data file, no History row); CSRF; 2 calls from the shared 800-call 24-hour budget (1 for an unknown name); 404 `not_found` when NEXON knows no such name (`/id`'s `400 OPENAPI00004` or `OPENAPI00003`); 409 `not_connected` without a Nexon key; 422 `invalid`, `region` for a non-`msea` region; 429 `lookup_cooldown` within 3 s of the same admin's last lookup; 503 `nexon_unavailable` while calls are held (key refused, maintenance, budget spent) or the call failed; 400 `invalid_body` for any other field) |
| `GET /api/admin/characters/{id}/link` | `character_data.json#/$defs/CharacterLink` (module `character_data`: every link and refresh route answers the generic 404, before the session check, while `modules.character_data` is off; 404 `not_found` for a character not on the roster; fresh data only, never expired or stamped in the future; the ocid is never sent; `attribution` is always `Data based on NEXON Open API`) |
| `POST /api/admin/characters/{id}/link` | `201` `character_data.json#/$defs/CharacterLink` (body `LinkRequest`; the link becomes `pending` and no Open API call is made until the member confirms in Discord; 422 `version_required`, 409 `stale` for another link version, 409 `already_linked`, 409 `not_connected` without a Nexon key, 409 `retired`, 422 `region` for a non-`msea` character, 404 `not_found`; Idempotency-Key as the character POST; adds one History settings row, section `characters`, key `link.<id>`, values `unlinked`/`pending`/`linked`/`stale` and the actor only) |
| `DELETE /api/admin/characters/{id}/link` | `common.json#/$defs/Message` (deletes the link and all its Nexon data in one write, makes the character's tracked Discord posts due for stripping at once; 404 `not_linked`; Idempotency-Key and History as the POST). Deleting a roster character, or changing its IGN, region or member, unlinks it the same way after the roster write commits |
| `POST /api/admin/characters/{id}/refresh` | `character_data.json#/$defs/CharacterLink` (fetches now, at any time: `basic` + `stat`, plus `id` when the ocid is missing or 29 days old, from the shared 800-call 24-hour budget; 409 `not_linked` unless active on an active character, 409 `not_connected`, 503 `nexon_unavailable` while calls are held (key refused, maintenance); a failed fetch still answers 200 with `refresh.last_error`; a retry with the same Idempotency-Key replays the first answer) |
| `GET /api/admin/runs/{id}/readiness` | `character_data.json#/$defs/RunReadiness` (module `character_data`, generic 404 before the session check while off; 404 `not_found` for a run that is gone; the slots of `GET …/characters` (what each member brings and its `basis`, recomputed; `candidates` counts their active characters), each with its level (fresh NEXON level, else the manual roster level), power (NEXON combat power, only from an active link's unexpired data; shown, never judged) and per boss of the run a level verdict against the knowledge `entry_level` and the character's MapleScouter reading (`scouter`: share, rating, minutes, `fetched_at`) when the estimates' 24-hour memory holds a successful lookup of that character, alone or in a party (the newest); `null` otherwise and always while estimates are off; never starts a lookup; computed on read, never stored, never blocking) |
| `GET /api/admin/fixed/{id}/readiness` | `character_data.json#/$defs/TimingReadiness` (modules `roster` and `character_data`; the saved timing's party and bosses, each slot as `GET /api/admin/fixed/{id}/characters` resolves it, with the values and verdicts of `RunReadiness`; 404 `not_found` for a timing that is gone) |
| `POST /api/admin/fixed/readiness` | `character_data.json#/$defs/TimingReadiness` (body `TimingPreview`: the Fixed editor's bosses and party as they stand and the timing they edit, absent for a new one; picks count only for members of the saved party, the rest bring their default; unknown boss tokens are left out; CSRF; nothing written; 404 `not_found` for a timing that is gone, 422 `invalid` past 64 bosses or members, 400 `invalid_body`) |
| `POST /api/admin/estimates` | `202` `character_data.json#/$defs/EstimateStarted` (body `EstimateRequest`: `character_ids` (1 to 6) or `run_id`, optional `refresh`; module `character_data` with `KANADE_SCOUTER_URL`: every estimate route answers the generic 404, before the session check, while the module is off or the URL unset; CSRF; nothing stored, no History row. A run looks up the character each party member brings (as `GET …/characters` resolves it); members without one are `missing`. Jobs wait in one FIFO line and run one at a time: a new job is `queued` with its `position`, or `running` when nothing is ahead; a target already queued or running answers that job; the job is already `done` with `cache_hit` when the 24-hour memory holds the same IGN set; 429 `estimate_queue_full` when 16 jobs wait already; 422 `invalid`, `region` (not MapleSEA), `unsupported_ign`, `nothing_to_look_up`, `too_many`; 404 `not_found` for an unknown character or run; 400 `invalid_body` for any other field) |
| `GET /api/admin/estimates?character_ids=<a>,<b>` or `?run_id=<id>` | `character_data.json#/$defs/EstimateStatus` (that target's job, with its `position` while queued, and the running job; 422 `invalid` for any other query) |
| `GET /api/admin/estimates/{job}` | `character_data.json#/$defs/EstimateJob` (each character's `outcome` and the members found so far fill in as each finishes; one failure never stops the others, and the party total covers the members found, `complete` only when all were. 404 `not_found` once gone. Jobs live in memory only, lost on restart: one per target, kept 24 hours; a new lookup of the same characters or run replaces a finished one (the old id answers 404). Memory is bounded at 512 targets and 1024 cached IGN sets; only past those is the oldest dropped early) |
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
| `POST /api/admin/inbox/{id}/approve`, `/reject` | `common.json#/$defs/Message`. A member request's approve body may carry `edit: {retime: [{change, day, time}], drop: [change]}` (change ids from `Proposal.requested`; `day` 0 = Monday): approved with those changes, 409 `stale` when the request is no longer the `version` edited, 422 `unknown_change`/`not_retimable`/`nothing_approved` |
| `GET /api/admin/drafts?state=open\|closed` | `drafts.json#/$defs/DraftList` (the session admin's own drafts; 422 `invalid_query`) |
| `POST /api/admin/drafts` | body `{title?}`; `drafts.json#/$defs/DraftWritten` (201) |
| `GET /api/admin/drafts/graph?from=&to=` | `drafts.json#/$defs/DraftGraph` (the session admin's own drafts whose span touches History records `from..=to`, any status; both seqs required, `from <= to`, else 422 `invalid_query`) |
| `GET /api/admin/drafts/{id}` | `drafts.json#/$defs/DraftDetail` (404 `not_found` for anyone but its author) |
| `PATCH /api/admin/drafts/{id}`, `DELETE /api/admin/drafts/{id}?version=` | body `{version, title}` / none; `drafts.json#/$defs/DraftWritten` (rename / discard) |
| `POST /api/admin/drafts/{id}/ops`, `DELETE /api/admin/drafts/{id}/ops/{n}?version=` | body `{version, op}` (`drafts.json#/$defs/OpBody`); `drafts.json#/$defs/DraftWritten` (422 `op_refused`/`op_needed`) |
| `GET /api/admin/drafts/{id}/preview` | `drafts.json#/$defs/DraftPreview` (410 `expired`) |
| `POST /api/admin/drafts/{id}/rebase` | body `{version, settle: {conflict key: keep\|take}}`; `drafts.json#/$defs/DraftWritten` (409 `conflicts` while any conflict is unsettled or the result no longer applies) |
| `POST /api/admin/drafts/{id}/merge` | body `{version}`; `drafts.json#/$defs/DraftWritten` with `seq` (409 `base_moved` off the head, `conflicts`, `no_effect`; 422 `empty`) |
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
| `GET`/`PATCH /api/admin/config` | `config.json#/$defs/ConfigView` (PATCH may add `notices`; both carry one while the saved models are not running) |
| `POST /api/admin/config/profiles/reload` | `common.json#/$defs/ReloadResult` |
| `POST /api/admin/digest` | `common.json#/$defs/Message` |
| `POST /api/admin/headers/rewrite` | `202` `common.json#/$defs/Message` (the run continues in the background; `200` when nothing posted this boss week has a header; 409 `ApiError` `rewrite_running`/`rewrite_off`) |
| `GET /api/admin/access`, `POST /api/admin/access/recheck` | `config.json#/$defs/AccessReport` |
| `GET /api/admin/history?week=&actor=&run=&before=&limit=` | `history.json#/$defs/HistoryPage` |
| `GET /api/admin/history/{seq}` | `history.json#/$defs/ChangeRecord` |
| `POST /api/admin/history/revert`, `/restore-week`, `/revert-actor` | `history.json#/$defs/RevertPlan` |
| `GET /api/admin/history/checkpoints` | `history.json#/$defs/Checkpoints` |
| `PUT /api/admin/presence` | body `presence.json#/$defs/PresenceCheckIn` (`{at: {kind: request\|run, id}, mode: viewing\|editing, seq}`); `presence.json#/$defs/PresenceHere` (the other admins there; the caller is never listed). Admin origin only, in memory only (never stored; empty after a restart; at most 64 places × 8 tabs, an entry gone 60 s after its last check-in). `seq` grows with each of a tab's calls: a check-in older than the tab's latest, or no newer than its leave, is answered but changes nothing. CSRF like every admin write, no Idempotency-Key; never extends the session's idle window; 400 `invalid_body` for an unknown kind, an id outside `[A-Za-z0-9_-]{1,64}` or a missing `seq` |
| `GET /api/admin/presence?at=<kind>:<id>` | `presence.json#/$defs/PresenceList` (a read without a check-in; 422 `invalid_query`) |
| `DELETE /api/admin/presence?at=<kind>:<id>&seq=<n>` | `204`, no body (best effort on leaving the page; the tab is kept as left for 60 s, so a check-in with `seq <= n` arriving later does not bring it back; 422 `invalid_query`) |
| `GET /api/admin/events` | `text/event-stream`, not JSON: a `ready` event `{seq, boot}` (`EventReady`; a new `boot` means the server restarted), then one message per change `{topic, seq}` (`EventHint`, topics `schedule`, `inbox`, `chat`, `extraction`, `rewrite`, `delivery`, `settings`, `rescan`, `presence`; no data) and `: keep-alive` comments; `429 too_many_streams` at the connection cap |
| any non-2xx | `error.json#/$defs/ApiError` |

## Files

`common.json` (enums, `Boss`, `Tally`, `Member`, `Channel`, `WeekDay`, `Head`,
`Message`, `ReloadResult`, `LogFacets`), `error.json`, `identity.json`,
`public.json`, `week.json`, `members.json`, `characters.json`, `fixed.json`,
`bosses.json`, `reminders.json`, `inbox.json`, `drafts.json`, `extractions.json`,
`chat.json`, `rewrites.json`, `limits.json`, `history.json`, `config.json`,
`presence.json`.

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
- Admin drafts (2026-10-11): `drafts.json` for the Drafts page
  (list, detail, preview with History diff rows and conflicts, writes;
  every write takes `version` and honours `Idempotency-Key`);
  `inbox.json` `Proposal.requested` / `RequestedChange` for approving a
  member request with changes; `public.json` `MemberRequest.edited` and
  `not_approved`.
- Character roster (2026-10-11): `characters.json` (`CharacterRow`,
  `MemberCharacters` and the `CharacterCreate`/`CharacterPatch` bodies)
  for the admin character routes of module `roster`.
- Run character picks (2026-10-12): `characters.json` (`RunCharacters`,
  `RunCharacterSlot`, `CharacterChoice`, `CharacterClash` and the
  `CharacterPick` body) for `GET /api/admin/runs/{id}/characters` and
  `PUT /api/admin/runs/{id}/characters/{member}` of module `roster`.
- History graph (2026-10-12): `drafts.json` `DraftGraph` and `GraphDraft`
  for `GET /api/admin/drafts/graph?from=&to=` (own drafts spanning the
  loaded History records, with each close time from the draft's event log).
- Admin presence (2026-10-12): `presence.json` (`PresenceCheckIn` body,
  `PresenceHere`, `PresenceList`, `PresenceEntry`) for
  `PUT`/`GET`/`DELETE /api/admin/presence`; the events stream adds topic
  `presence` (someone arrived at, left or changed mode at a place).
- MapleScouter estimates (2026-10-12): `character_data.json`
  `EstimateRequest`, `EstimateStarted`, `EstimateStatus`, `EstimateJob`,
  `EstimateResult`, `EstimateError` for `/api/admin/estimates`, and
  `CharacterDataHealth.scouter` (`ScouterHealth`: `off`, `ok`,
  `unreachable`, `verification_required`, the last lookup and error).
- Run readiness (2026-10-12): `character_data.json` `RunReadiness`,
  `ReadinessSlot`, `ReadinessLevel`, `ReadinessPower`, `BossReadiness` for
  `GET /api/admin/runs/{id}/readiness` (each slot's level and power, and a
  level verdict per boss against its knowledge `entry_level`).
  `BossReadiness.scouter` (`ScouterReading`, or `null`) carries the
  character's cached MapleScouter reading per boss; `ReadinessPower.kind`
  is only `nexon_cp` (the reserved `scouter` kind is removed).
- Admin refresh and privacy link (2026-10-12): `RefreshInfo.refreshable_at`
  and `429 refresh_cap` are removed (an admin may refresh at any time;
  only the call budget limits it); `CharacterLink.privacy_url` names the
  public `/privacy` page, `null` without a public listener.
- Estimate queue and assumed critical rate (2026-10-13): `EstimateJob.state`
  adds `queued` with `position` (lookups ahead, the running one included);
  `EstimateJob.characters[]` adds `outcome` (`pending`, `ok`, `failed`) and
  `error`; `result` carries the members found so far while the job is
  queued or running, and `error` on a `done` job is the party total's own
  failure. `EstimateResult.members[]` and `ScouterReading` add
  `critical_rate_assumed` and `critical_rate_actual`. `409 estimate_running`
  is removed (`429 estimate_queue_full` past 16 waiting jobs).
- Add character lookup (2026-10-13): `character_data.json` `LookupStatus`,
  `CharacterLookupRequest`, `CharacterLookup` for
  `GET`/`POST /api/admin/characters/lookup` (shown only, never stored), and
  `characters.json` `CharacterCreated`: the character create answers its row
  plus `link` (`pending` when it also asked the member to link a MapleSEA
  character, `failed`, or `null`).
- Fixed readiness and timing picks (2026-10-13): `characters.json`
  `TimingCharacters` for `GET`/`PUT /api/admin/fixed/{id}/characters[/{member}]`
  and `character_data.json` `TimingReadiness` (body `TimingPreview`) for
  `GET /api/admin/fixed/{id}/readiness` and `POST /api/admin/fixed/readiness`
  (modules `roster` and `character_data`). `CharacterBasis` (`run_pick`,
  `timing_pick`, `main`, `only`) replaces the inferred notion everywhere:
  `RunCharacterSlot.inferred` becomes `brings` + `basis` (plus `retired`,
  `picked_at`, `picked_by`), `CharacterClash.inferred` becomes `basis`, and
  `ReadinessSlot.inferred` becomes `basis` (plus `candidates`). The main is
  now the default on runs too.
