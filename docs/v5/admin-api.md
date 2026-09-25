# Admin API used by the v5 PWA (proposal)

Status: every route below is what the PWA calls today against the dev mock
(`tools/pwa-mock`); each is **proposed** — the backend confirms or revises
it. DTO names are from `web/packages/api-types`. Errors are `ApiError`
(`{error, message}`); mutations that touch a run take the week's `version`
and answer `409` with a fresh read when it moved underneath.
Response JSON Schemas (frozen contract, endpoint index): [`api-schemas/`](api-schemas/README.md).

Rows marked **Implemented** are served by the Rust binary (`src/api/`);
listener, guard and static-serving behaviour is in `runtime-bootstrap.md`.
Unmounted `/api/admin/*` paths are `404`; every mounted admin route requires a
session (below) and answers `401 unauthenticated` without one.

## Sign-in and sessions (**Implemented**)

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/admin/auth/methods` | — | `{discord, tailscale, token}` booleans | `tailscale` is true only when this request carries an allow-listed identity from the trusted edge. |
| `GET /api/admin/auth/discord/start?next=/path` | — | `303` to Discord | Sets the pre-auth cookie. `next` must be a same-origin path (else `/`; never `//…`, `\`, schemes or `/api/…`). |
| `GET /api/admin/auth/discord/callback` | Discord's `code`, `state` | `200` HTML landing page (meta refresh + link to `next`, no script) + session cookie | Failures: `303 /?login_error=state\|denied\|forbidden\|discord\|unavailable\|rate_limited` (bad/expired/replayed state; cancelled; not staff or a bot account; code refused or a scope other than exactly `identify`; Discord (incl. its 429 cooldown) or member data unavailable; too many attempts). |
| `POST /api/admin/auth/tailscale` | `{}` or no body | `Session` + cookie + `X-Kanade-CSRF` | 401 unless the edge vouches for an allow-listed login. |
| `POST /api/admin/auth/token` | `{token}` | `Session` + cookie + `X-Kanade-CSRF` | Break-glass; every use is logged at WARN. 401 on a wrong token, 400 `invalid_body`, 429 `rate_limited`. |
| `POST /api/admin/auth/logout` | — | `204` + cleared cookie | Needs CSRF. Deletes the session server-side. |
| `GET /api/admin/session` | — | `Session` (`{display}`) + `X-Kanade-CSRF` | 401 `unauthenticated` when signed out. |

Contract for the frontend (API-5):

- Cookie `__Host-kanade_admin` (Secure, HttpOnly, Path=/, `SameSite=Strict`,
  `Max-Age` = absolute lifetime). The PWA never reads it; `fetch` sends it
  with `credentials: 'same-origin'`. The pre-auth cookie
  `__Host-kanade_admin_login` is `SameSite=Lax` (it must survive Discord's
  redirect back) and lives 10 minutes. The callback answers a same-origin
  `200` landing page that navigates on to `next`, so the first page load
  already carries the Strict cookie (a `303` chain begun cross-site would
  not). Real-browser confirmation belongs to the A10 browser e2e.
- CSRF: read `X-Kanade-CSRF` from `GET /api/admin/session` (or the tailscale/
  token login response) and send it as `X-Kanade-CSRF` on every `POST`,
  `PATCH`, `PUT` and `DELETE`. It is fixed for the session's lifetime and
  changes on every login. The browser's own `Origin`/`Sec-Fetch-Site` must
  say same-origin. Missing, foreign or cross-site: `403 csrf`. Login
  `POST`s need only the same-origin markers.
- `401 unauthenticated` means sign in again (expired, idle, logged out, staff
  revoked, or the edge identity changed). `503 auth_unavailable` means sign-in
  is not configured or the staff check cannot run; the session is kept.
- Idle timeout 60 min, absolute 12 h (configurable); any authenticated
  request counts as activity. Discord staff status (Administrator, guild owner
  or admin role, from the bot's member data) is re-checked every 5 minutes;
  losing it ends all of that person's sessions. Tailscale sessions need the
  same edge-vouched login on every request.
- `Authorization: Bearer <ADMIN_TOKEN>` is accepted on any admin route for
  the CLI; it needs no CSRF token, is attributed to `token`, and a bad bearer
  never falls back to the cookie.
- Rate limits (token buckets per client IP and global, per route): Discord
  start and callback 10/min per IP, 60/min overall; token login 5/min per IP,
  20/min overall; wrong bearers 5/min per IP, 30/min overall (only failures
  count, but an exhausted client waits even with the right token). Answers:
  `429 rate_limited`, or `/?login_error=rate_limited` on browser flows. A
  client holds at most 5 unfinished Discord logins (its oldest yields); when
  256 are pending, new logins are refused. A Discord `429` pauses Discord
  sign-in for its `retry_after` (1 s–1 h) with `login_error=unavailable`.
- History attribution: `admin` actors `discord:<user id>`,
  `tailscale:<login>` or `token`.

Conventions: `?week=this|next` selects the boss week on week reads; ids in
paths are URL-encoded (runs, inbox, members, fixed, limits, rescan); `PATCH` bodies are
partial. A partial `PATCH` takes one section per request and replaces arrays
whole; unknown or read-only keys are refused with 422.

## Mutations (**Implemented**, A4)

- Every mutation needs a session and, with the cookie, `X-Kanade-CSRF`
  (403 `csrf`). Changes are recorded as the session's actor on surface
  `admin_portal` (`cli` for the bearer). One scheduler writer serialises
  them; reads never wait for it.
- **Week version (API-1):** each run edit declares the fields it changes
  (`slot`, `status`, `participants`, `rsvp:<member>`; reset declares `slot`,
  `participants`, `bosses`, `channel`). A field last changed after the
  request's `version` is `409 stale`; otherwise the change that set it is
  re-checked inside the commit, so an edit landing in between is `409 stale`
  too. Other fields of the same run merging freely is the point: edits to
  different fields at the same version never conflict. An edit that
  re-derives the status (roster, answers) changes `status` too.
- **Weekly timings (user decision 2026-09-25):** `PATCH /api/admin/fixed/{id}`
  requires `version` (missing: `422 version_required`); it declares every
  timing field (`day`, `time`, `bosses`, `participants`, `channel`, `note`)
  whose value differs from the stored row and goes through the same check.
  The body is the whole form, so a form loaded before someone else's edit
  resends the old value of that field and is `409 stale` rather than
  reverting it. The admin app and `tools/pwa-mock` must send `version` on
  this PATCH (client change scheduled separately). `POST` ignores it.
- Explicit `expect: [{field, seen}]` and admin `override: [{seq, hash}]`
  are accepted in the same bodies and replace the version-derived
  expectations; their refusals are 422 `unknown_field` | `duplicate_field` |
  `override_not_seen` | `override_unchanged` | `unknown_override`, 404
  `unknown_target`, 403 `override_forbidden`. The 409 body is a plain
  `ApiError` (`stale`); the proposed `conflicts` list needs an `error.json`
  extension first.
- **`Idempotency-Key`** (1–128 of `[A-Za-z0-9-_.:]`, else 400
  `invalid_idempotency_key`) is the request id: a replay answers the current
  state (200/201) without applying again, even after its own change moved the
  version; the same key with another request is `422 idempotency_mismatch`.
  Weekly-timing create/edit/retire look the key up before validating: a
  replay is checked against the roster and watched channels as the first
  attempt saw them (a member who has since lost the role does not turn it
  into a 422), and the scheduler still compares the request digest. A retire
  replay is matched on the recorded change (that timing deleted), not the
  digest, which names the materialised weeks and so moves at the weekly
  reset; it answers `{cancelled: 0}`, as the first count is not recorded.
- Known limits: the history walk behind `version`-derived expectations is
  unbounded; two concurrent RSVPs for different members both succeed (each
  declares only its own `rsvp:<member>`). A weekly-timing PATCH replay is
  matched on the fields that still differ: if another edit landed in
  between, the replay answers 422 `idempotency_mismatch` although the first
  attempt applied (nothing is written), and a reused key whose body now equals
  the stored row answers 200 without a mismatch check. Portal answers send no
  v4 decline notice yet.
- Other refusals: 404 `not_found`; 409 `move_conflict` (the weekly already
  has a run in that week) or `busy` (revision races outlasted the retries);
  422 `invalid` with the scheduler's own wording, `nothing_to_change`,
  `choices_required`, `choices_not_applicable`, `not_on_run`, `alias_taken`,
  `version_required`;
  400 `invalid_body` (unknown fields refused); 503 `unavailable` (no backend
  text is ever returned).

## Schedule

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/admin/week?week=` | — | `Week` (with `version`) | Hidden done/cancelled runs are filtered client-side. **Implemented**: `version` is the history head seq (API-1); per-field `versions` are not sent (the frozen `Week` schema has none); `?week=` other than `this`/`next` is `422 invalid_query`; cards show only `day_of`/`countdown_60`/`countdown_15` (the `CardKind` enum); own-time runs have `time: null`. |
| `GET /api/admin/stats?week=` | — | `Stats` | Answers chart. **Implemented**. |
| `GET /api/admin/summary` | — | `Summary` | Now tiles + nav Inbox pip + model tile. **Implemented**; `inbox` = live proposals + submitted member requests; `model` is `{busy: false, holder: null}` until the governor is composed. |
| `GET /api/admin/members` | — | `MemberRow[]` | One list for the Members page, filter lists and roster adds (`MemberRow` extends `Member`). **Implemented**: bossing members plus anyone with staff or pilot access, never bots. |
| `GET /api/admin/channels` | — | `Channel[]` | Filter lists, digest channel picker. **Implemented** over a `ChannelList` port. |
| `GET /api/identity` | — | `Identity` | Masthead, login window. **Implemented** on both origins (offline name `Kanade`; `cached` reflects `KANADE_IDENTITY_DIR`). |
| `GET /api/admin/session` | — | `Session` | Who is signed in. **Implemented** (see Sign-in and sessions). |
| `POST /api/admin/runs/{id}/move` | `{day, time, version}` | `MoveResult` (`{run, previous, version}`) | Planner + keyboard moves; undo is a second move. **Implemented**: `day` 0–6 within the run's boss week; `time` null only for own-time runs; a slot outside the run's boss week (day 0 before a non-midnight reset time) and done/cancelled runs are refused (422 `invalid`). |
| `PATCH /api/admin/runs/{id}/status` | `StatusRequest` (`{status, version}`) | `RunResult` (`{run, version}`) | **Implemented**; `at_risk` is derived, not settable (422). |
| `POST /api/admin/runs/{id}/rsvp` | `RsvpRequest` (`{member_id, answer, version}`) | `RunResult` | **Implemented** as v4 `set_rsvp`: `yes`/`no` are recorded with source `chat` and the status is re-derived without ending a status pin (`attendance.md`); `clear` removes any answer, `maybe` included; a member not on the run is `422 not_on_run` (`409 stale` if their answer changed since `version`). |
| `PATCH /api/admin/runs/{id}/participants` | `ParticipantsRequest` (`{add?, remove?, version}`) | `RunResult` | Week-only roster edits. **Implemented**. |
| `POST /api/admin/runs/{id}/reset` | `{version}` | `RunResult` | New in v5: back to the weekly timing. **Implemented**. |
| `POST /api/admin/rescan` (per-run) | `{channels: [run.channel_id], window: 'week'}` | `RescanJob` | The run sheet and phone board re-read one channel; `Run.channel_id` is the explicit key (`party` is the legacy handle). |
| `POST /api/admin/runs/{id}/ping` | `{}` | `{message}` | Preview ping text. **Implemented** as a preview only: nothing is posted (the delivery tick owns Discord sends). |

## Fixed timings, bosses, knowledge

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/admin/fixed` | — | `FixedRow[]` | **Implemented**; `runs` lists live runs this and next week. |
| `POST /api/admin/fixed` | `FixedRequest` | `FixedRow` | `decisions` maps amended-run ids to `update`/`keep`. **Implemented**: `201`; the timing's runs are materialised for the current and next two boss weeks; members need the bossing role and the channel must be watched (422). |
| `PATCH /api/admin/fixed/{id}` | `FixedRequest` | `FixedRow` | Same `decisions` for the update-or-keep step. **Implemented**: only fields that differ are edited; an amended run the edit would move needs a decision (`422 choices_required`), a decision for another run is `422 choices_not_applicable`; `version` required (`422 version_required`), `expect`/`override` as for runs (see "Weekly timings" above). |
| `DELETE /api/admin/fixed/{id}` | — | `{cancelled}` | Retire; names how many upcoming runs cancel. **Implemented** (live runs in the three materialised weeks). |
| `POST /api/admin/validate/bosses` | `{text}` | `ValidateResult` | Debounced bosscheck. **Implemented** (catalog parser; refusals are `422 invalid` with the parser's message). |
| `GET /api/admin/bosses` | — | `BossRow[]` | **Implemented**; keys are catalog short names (`MaleficStar`, exact case, as `/art/*` keys); hue from the catalog guide colour. |
| `GET /api/admin/bosses/events` | — | `EventBoss[]` | New in v5. **Implemented**. |
| `GET /api/admin/bosses/{key}/knowledge` | — | `Knowledge` | Schema v2, served from `boss/knowledge/*.yaml`. **Implemented**; unknown or non-alphanumeric keys are 404. |

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
| `PATCH /api/admin/members/{id}` | `MemberPatch` | `MemberRow` | Ping level, reply style. **Implemented** (`persona: ""` clears; unknown member 404). |
| `POST /api/admin/members/{id}/aliases` | `{alias}` | `MemberRow` | v4 `…/nick`. **Implemented**: one word (letters, digits, `-`, `_`, ≤ 32), lowercased; held by someone else is `422 alias_taken`. |
| `GET /api/admin/personas` | — | `Persona[]` | Reply-style picker. **Implemented**. |
| `GET /api/admin/reminders` | — | `Reminders` | `?run=` narrows client-side. **Implemented** for this and next boss week. |
| `GET /api/admin/config` | — | `ConfigView` | All runtime settings, the Manage-Messages banner list, the env-only table, and `notices` (empty on GET). |
| `PATCH /api/admin/config` | One section, partial (see `ConfigPatch`) | `ConfigView` + `notices` | One section per save; arrays (`countdown_minutes`, `role_profiles`, `groups`) are replaced whole. Runs the startup capacity check: nothing that would stop the bot is saved. A stranded reasoning level (an alias or extraction change invalidating a role that was not part of the request) is reset to `off` and reported in `notices`, e.g. `"chat reasoning reset to off: kanata/chat does not publish high."` Reasoning resolution: `""` inherits the extraction role's effort; inherit is legal only while that effort is `off` or published for the alias; a model that decides takes `low`/`medium`/`high`. |
| `POST /api/admin/config/profiles/reload` | `{}` | `{message, reloaded}` | **Proposed**. Re-reads `config/personas/profiles/` after a file edit. Profile text is files-only by decision (a deliberate v4 drop); the app shows profiles read-only and only publishes them or assigns them to roles. |
| `POST /api/admin/digest` | `{week, channel_id?}` | `{message}` | v4 `POST /digest` parity, channel override included. |
| `GET /api/admin/access` | — | `AccessReport` | v4 `GET /access`. |
| `POST /api/admin/access/recheck` | `{}` | `AccessReport` | v4 `POST /access`. |
| `GET /api/admin/history?week&actor&before&limit` | — | `HistoryPage` | `before` is a seq cursor; `next_before` pages older. **Implemented** (A5): newest first, genesis never listed; `week` is a boss-week start, either as records name it (RFC 3339 instant, `+` sent as `%2B`) or the guild-local start date (`Week.starts`); `actor` is `kind:id` (`admin:discord:1`); both filters combine; `limit` 1–100 (default 20); `total` counts every match. Any other key, a repeated key or a malformed value is `422 invalid_query`. Records are exactly the hashed `kanade.change.v1` bodies plus `hash` (`docs/v5/history.md`), reminder rows included. |
| `GET /api/admin/history/{seq}` | — | `ChangeRecord` | **Implemented**; genesis, unknown and non-numeric seqs are 404. The PWA reads records from the page list and does not call this yet. |
| `POST /api/admin/history/revert` | `{seqs[], force?, preview?, request_id?}` | `RevertPlan` | **Implemented** (rollback rules below). An empty `seqs` or an unknown/genesis seq is `422 invalid`. |
| `POST /api/admin/history/restore-week` | `{week, revision, force?, preview?, request_id?}` | `RevertPlan` | **Implemented**: `week` as in the history query (not a week start: 422); nothing to restore is `outcome: unchanged`. |
| `POST /api/admin/history/revert-actor` | `{actor, since, force?, preview?, request_id?}` | `RevertPlan` | **Implemented** (A5-7): `actor` is `kind:id`; `since` is a bare date (local midnight) or a naive local `YYYY-MM-DDTHH:MM[:SS]` in the guild zone; offsets, other shapes and DST-gap times are `422 invalid`. Nothing to revert is `unchanged`. |
| `GET /api/admin/history/checkpoints` | — | `Checkpoints` | **Implemented** (A5-9): `verified` is the full chain check; `backups` is `[]` until a backup directory is configured. Named checkpoints (tags) and checkpoint restore are **proposed** and need a schema extension. |
| `GET /api/admin/runs/{id}/blame` | — | `BlameEntry[]` | **Implemented** (A5-4/8): the domain's field names (`slot`, `bosses`, `participants`, `channel`, `status`, `status_pin`, `rsvp:<id>`, `attended:<id>`), `value` read from the current run; fields no record set are omitted; unknown run 404. |

Rollbacks (**Implemented**, A5): every body takes `force?`, `preview?` and
`request_id?`. `preview: true` plans the rollback now and writes nothing (its
request id is ignored); a strict plan with conflicts is `outcome:
conflicts` (200, nothing written, `rows: []`, `reverts` the requested seqs);
`force` puts the recorded `before` values back and lists the overridden
conflicts. Applying records one `rollback` change by the session's admin and
answers `outcome: applied` with `rows` equal to the record's. Reminder rows
are re-planned on apply, so their ids differ from the preview's. The request
id is `Idempotency-Key` or `request_id` (same rules; both must agree, else
`400 invalid_idempotency_key`): a retry answers the recorded rollback
(`reverts` = its refs), the same id for another rollback is `422
idempotency_mismatch`. CSRF is required even to preview; other refusals follow
the A4 table.

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
- Not in A5 (decision A5-9): unmounted until a schema exists. The service's
  preview is steps plus `{run_id, field, expected, current}` conflicts and a
  `force` expectation set a forced pick must echo, not a `RevertPlan`.

### History graph

Not in A5 (decision A5-9): unmounted until a schema exists; it needs the
drafts routes (API-6) and member requests (A6).

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
**Implemented**: the public listener is always closed today (any method on
`/api/public/*` other than status, and on `/art/*`, is `503 closed`).

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `GET /api/public/status` | — | `PublicStatus` (`{portal: 'open' \| 'closed'}`) | **Implemented** (always `closed`). The closed page needs nothing else to render. |
| `GET /api/public/week` | — | `PublicWeek` | No names, answers, party or version. `503 {error: 'closed'}` while the portal is off. |
