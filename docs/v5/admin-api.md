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
| `GET /api/admin/session` | — | `Session` (`{display, method}`; `method` is `discord`, `tailscale` or `token`, and only `discord` sessions may decide proposals) + `X-Kanade-CSRF` | 401 `unauthenticated` when signed out. |

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
  this PATCH (both do; the client also sends `X-Kanade-CSRF` and an
  `Idempotency-Key` on every admin write). `POST` ignores it.
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
| `GET /api/admin/roles` | — | `Role[]` | `{id, name, color?}` for id→name display (`color` `#rrggbb`). **Implemented** from the current gateway guild cache, highest first, `@everyone` left out; 503 `unavailable` while the role directory is disconnected. |
| `GET /api/identity` | — | `Identity` | Masthead, login window. **Implemented** on both origins: `name` is the bot's guild nickname, else global name, else user name once the gateway is `READY` (`Kanade` before that and offline); `cached` reflects `KANADE_IDENTITY_DIR`; `version` (additive) hashes the name and cached art, and `avatar`/`banner` carry it as `?v=` so a refresh is a new URL. `/identity/{avatar,banner}` answer with an ETag (`If-None-Match` → 304) and `Cache-Control: public, max-age=86400, must-revalidate`; with nothing cached they are an SVG monogram of the name's first letter and the accent wash. |
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
| `GET /api/admin/inbox` | — | `Proposal[]` | Extractor/chat proposals and member requests together. **Implemented (A6)**, see "Inbox (A6)". |
| `POST /api/admin/inbox/{id}/approve` | `{version?, choices?, day?, time?}` | `{message}` | **Implemented (A6)**; `day`/`time` edit a proposal's time before approving; no `force`. |
| `POST /api/admin/inbox/{id}/reject` | `{version?, reason?}` | `{message}` | **Implemented (A6)**; `reason` required for member requests only. |
| `GET /api/admin/extractions` | — | `Extractions` | Paged client-side. **Implemented (A7)**, see "Logs and rescans (A7)". |
| `GET /api/admin/extractions/{id}` | — | `Extraction` | Tabs: changes, chat read, prompt, raw. **Implemented (A7)**; adds `refusals`. |
| `GET /api/admin/rescan/targets` | — | `Channel[]` | Watched channels only. **Implemented (A7)**. |
| `POST /api/admin/rescan` | `{channels[], window}` | `RescanJob` | `window`: `week`, `since_reset`, `two_weeks`. **Implemented (A7)**: queues and answers at once. |
| `GET /api/admin/rescan/{id}` | — | `RescanJob` | Polled per channel. **Implemented (A7)**. |
| `DELETE /api/admin/rescan/{id}` | — | `RescanJob` | Cancel. v4 `POST …/cancel`. **Implemented (A7)**; safe to repeat. |
| `GET /api/admin/chat` | — | `Chat` | **Implemented (A7)**. |
| `GET /api/admin/chat/{id}` | — | `ChatTurn` | **Implemented (A7; transcript fields)**; a withheld question is not shown; historical masked turns may carry `model_view` (admin listener only), while new raw turns do not create one. |
| `GET /api/admin/limits` | — | `Limits` | See `limits-contract.md` (**proposed**). |
| `DELETE /api/admin/limits/windows/{id}` | — | `{message}` | Clear one member's window. v4 `POST …/reset`. |

## Operate (members, reminders, config, history)

| Method & path | Request | Response | Notes |
|---|---|---|---|
| `PATCH /api/admin/members/{id}` | `MemberPatch` | `MemberRow` | Ping level, reply style. **Implemented** (`persona: ""` clears; unknown member 404). |
| `POST /api/admin/members/{id}/aliases` | `{alias}` | `MemberRow` | v4 `…/nick`. **Implemented**: one word (letters, digits, `-`, `_`, ≤ 32), lowercased; held by someone else is `422 alias_taken`. |
| `GET /api/admin/personas` | — | `Persona[]` | Reply-style picker. **Implemented**. |
| `GET /api/admin/reminders` | — | `Reminders` | `?run=` narrows client-side. **Implemented** for this and next boss week. |
| `GET /api/admin/config` | — | `ConfigView` | **Implemented** (A9): models from the live Kanata catalog (`reachable: false` keeps the last snapshot); `catalog[].trust_zone` and `leaves_homelab` identify listed external routes. Requests sent to them carry raw data when present, without an opt-in; model-check/startup logs warn about this routing. Unlisted aliases are treated as external until classified. `models.pii_pseudonymise` is a required, read-only compatibility boolean that is always `false`; the `env` list has no rows for retired privacy variables. `models.groups` lists one `{model, group, permits}` row per alias per group the governor runs; `models.groups_source` is `default` (one `gateway` group of `KANADE_MODEL_PERMITS` over the distinct role aliases) or `config` (`kanade.toml` `[[models.groups]]`, summarised in the env row). `key_limits.max_in_flight` is `null` (Kanata publishes no per-key limit). `capacity_check` checks each group's permits against the least admission of its aliases (`Group <name> …`) and, with declared groups only, warns `The <role> model <alias> is in no capacity group; its calls are refused.` All runtime settings, the Manage-Messages banner list, the env-only table, and `notices` (empty on GET). |
| `PATCH /api/admin/config` | One section, partial (see `ConfigPatch`) | `ConfigView` + `notices` | **Implemented** (A9): 422 `unknown_field`/`read_only`/`invalid`/`capacity`/`ungrouped`/`idempotency_mismatch`, 409 `conflict` for stale role-assignment digests, 503 `models_unreachable` for models and `unavailable` while a changed assignment cannot be checked against the current role directory; `models.groups` is `read_only` (set in `kanade.toml` `[[models.groups]]`; restart to apply); `persona.role_profiles` is writable with its required digest precondition (see Role profile assignments below); `persona.visibility` accepts selected-key deltas (see Reply profile visibility below); a no-limit alias only warns. Persona switches, model roles and role-profile assignments apply live (a saved assignment is used by the next chat question; calls already running keep their current state); `RoleModel.running` shows what the next model call uses; extraction and heading rewrites that had no model at startup start only after a restart, so a save giving them their first model adds a `notices` line saying so and `running` stays absent for them; pings apply on restart. With declared `[[models.groups]]`, switching a role to an alias no group lists is refused (422 `ungrouped`, naming the role and alias; an alias already saved never blocks another save); without declared groups the alias joins the default `gateway` group. A saved alias starts as leaving the homelab until Kanata's listing shows its zone. Chatbot rates: `count` 1–100 answers per `window_s`; `member_rate.count` may also be `0` (staff only: members without a per-member override are ignored silently, as the role gate does, with no reaction, reply, log row or model call). One section per save; arrays (`countdown_minutes`, `role_profiles`, `groups`) are replaced whole, while `persona.visibility` is a selected-key delta merged with current visibility. Runs the startup capacity check: nothing that would stop the bot is saved. A stranded reasoning level (an alias or extraction change invalidating a role that was not part of the request) is reset to `off`, or to the alias's lowest published level where `off` is not allowed, and reported in `notices`, e.g. `"chat reasoning reset to low: kanata/chat does not publish high."` Reasoning resolution: see "Config semantics"; `""` inherits the extraction role's effort and is legal only while that effort is legal for the alias. |
| `POST /api/admin/config/profiles/reload` | `{}` | `{message, reloaded}` | **Implemented** (A9); safe to repeat. Re-reads `config/personas/profiles/` after a file edit. Profile text is files-only by decision (a deliberate v4 drop); the app shows profiles read-only and only publishes them or assigns them to roles. |
| `POST /api/admin/digest` | `{week, channel_id?}` | `{message}` | v4 `POST /digest` parity, channel override included. |
| `GET /api/admin/access` | — | `AccessReport` | v4 `GET /access`. **Implemented**: watched text channels plus the digest channel (`posting.channel_id`), in channel order, read live from the gateway cache; unknown permissions count as granted (v4); `connected: false` with no rows until the guild is loaded; `checked_at` is guild-local (`Tue 29 Sep 12:00`). |
| `POST /api/admin/access/recheck` | `{}` | `AccessReport` | v4 `POST /access`. **Implemented**: the same fresh reading (the cache follows the gateway); CSRF applies; safe to repeat. |
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
- Reasoning (user decision 2026-09-26): `off` is legal only when the alias
  publishes no list (`null`: Kanata restricts nothing) or a list containing
  `none`, or has no reasoning control (nothing is sent); a list without `none`
  means the model requires reasoning, `ModelInfo.off_allowed` is false, and
  setting `off` is refused (422 `"<alias> requires reasoning: pick low,
  medium or high."`, naming its published levels). Any other level must be in
  the alias's `reasoning_efforts`; `null` accepts every level. No alias names
  are special-cased: a model that in fact needs reasoning but publishes `null`
  keeps whatever level is configured. `""` (chat/rewrite only)
  inherits extraction's effort and is legal only when that effort is legal
  for the role's alias. Validation runs after every role in the request is
  applied, so inheritance resolves against the FINAL extraction effort and
  each role's final alias. A level the request sets — including an explicit
  `""` whose resolved effort is illegal — is refused (422). A stored level the
  request did not set, stranded by an alias or extraction change, is reset to
  `off` — or, where `off_allowed` is false, to the alias's lowest published
  level — and named in `notices` (`"chat reasoning reset to low: kanata/chat
  does not publish high."`, or for a stranded `off` `"chat reasoning set to
  low: kanata/chat requires reasoning."`). The admin app sends all three
  roles, hides `off` where `off_allowed` is false, and resets a stranded
  inheritor the same way locally (with a visible note) before saving, so its
  saves never hit the 422. The model stack applies the same rule at runtime
  (setup effort resolution and the runner's per-call shaping). Live switch
  rule for a level the new alias does not publish: a level the request sets
  is refused (422, nothing saved or switched); an untouched level is reset
  with a notice and the reset level is what runs. The running stack itself
  never refuses: a level stranded later (a listing change, env seeds) is sent
  as the alias's floor and shown in `running.reasoning`.
- Reasoning variants: Kanata lists `<base>:<level>` aliases (`gpt-6-luna:high`)
  — the base model with reasoning baked in. An alias is a variant only when
  `<level>` is `none`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`
  and `<base>` is itself listed (`gpt-oss:120b-cloud` is not). `ModelInfo`
  carries `variant_of` and `fixed_effort` (`:none` is `off`); the picker lists
  base models only and reasoning is chosen separately. Saving a variant alias
  is still accepted: its fixed level wins, the stored `reasoning` is set to it
  (an explicit level, `""` included, that differs is ignored with a notice,
  `"chat reasoning is fixed at high by gpt-6-luna:high; the requested medium is
  ignored."`, never a 422; an untouched stored level is set with `"… set to
  high: gpt-6-luna:high fixes it."`), and inheritors of extraction see that
  level. A stored variant's `RoleModel` carries the same two fields, shown as
  "gpt-6-luna (fixed: high)". The model stack and `kanade models check` apply
  the same detection.
- Self-service mode (`self_service.mode`) sets how the extractor and the
  chatbot answer a change they detect: `cards_and_link` (default) keeps the ✅
  card and adds a pre-filled deep link for moves the author can make
  themselves; `link_first` sends only the link for those; `cards_only` is v4
  behaviour. `effective_mode` is `cards_only` whenever `public_portal` is
  off. Links carry the run id and proposed time only — no secret; members
  sign in with Discord and the server re-validates everything.
- Capacity: see `limits-contract.md` (per-alias admission rule and DTOs).

### Reply profile visibility

`persona.visibility` is persisted as the ordered, deduplicated
`v5.profile_visibility` list of member-selectable profile ids. A missing key is
empty/private; readable persona files never imply publication. The config
response still lists all readable profiles in `persona.profiles`, with
`public` reflecting the saved list. Profile prompt text is not returned.

Publish or make selected profiles private with a delta, not a replacement of a
stale list:

```json
{"persona":{"visibility":[{"key":"calm","public":true},{"key":"bold","public":false}]}}
```

Each `key` must identify a currently readable profile and each `public` must
be a boolean. Duplicate keys in one delta, malformed or unreadable ids and
unknown item fields are refused (`422 invalid` or `422 unknown_field`). The
delta merges under the config desk lock: unrelated visibility and existing
order are preserved; newly published keys append in request order. It can be
sent with `persona.active` in the same section. A changed list returns the
notice `Reply profile visibility updated.`; a no-op returns no visibility
notice. The resulting `persona.profiles[].public` values are authoritative.
Cookie writes require CSRF and `Idempotency-Key` follows the config PATCH
rules above; a concurrent reuse with another body has one winner.

Only published profiles appear in `GET /api/admin/personas` and the Members
picker and are accepted for member `/style` choices, autocomplete and chat
resolution. `PATCH /api/admin/members/{id}` may save a published profile;
`persona: ""` (or `"default"`) clears the selection. A saved choice stays on
the member row when unpublished, reports `persona_available: false`, and chat
uses the bundle default until it is published and readable again. Reloading
persona files updates choices immediately without dropping the saved
visibility list. Pure role-profile precedence is independent of publication.
These settings and prompt summaries remain admin-only; admin routes are still
404 on the public listener.

### Role profile assignments

`persona.role_profiles` is the ordered, saved list of `{role_id, profile}`
assignments. A missing `v5.role_profiles` row is empty. `persona.role_profiles`
in Config includes `role_name` from the current guild cache (nullable when a
role is unavailable) and `persona.role_profiles_digest`, an opaque,
order-sensitive `sha256-v1` digest of only the role IDs and profile IDs. Role
names are display metadata and are never persisted. Clients should treat the
digest as opaque and send it back as the write precondition.

```json
{
  "persona": {
    "role_profiles": [
      {"role_id": "700", "profile": "calm"}
    ],
    "role_profiles_digest": "sha256-v1:<digest from Config>"
  }
}
```

The assignment PATCH contains only `role_profiles` and its digest. It replaces
the ordered list; there may be at most 20 unique canonical positive Discord
role IDs. `@everyone` is excluded from `GET /api/admin/roles`; managed roles
present in that current guild list are allowed. New or changed assignments
must name a role in the connected current role directory and a readable Reply
profile. A readable private profile may be assigned; publication is only for
member selection. An unchanged assignment for a role no longer in the guild
may be reordered or removed, but not edited or rebound. When the directory is
disconnected, `GET /api/admin/roles` returns 503 `unavailable`; Config retains
saved assignments with null role names, and PATCH refuses new or changed
assignments with 503.

The digest is checked while holding the Config desk lock, after idempotency
replay. A stale list or order returns 409 `conflict`; reload Config and retry
with the new digest. Cookie writes require CSRF, and `Idempotency-Key` follows
the Config PATCH rules above. Saves apply to the next chat question: the first
readable matching role assignment wins before the member's saved profile.
Assignments never grant chatbot access. Role assignments and their display
metadata are admin-only; admin routes remain 404 on the public listener.

## Inbox (A6)

**Implemented (A6)** over the domain (`history.md` "Member requests",
"Proposals"). One route set serves both sources; ids are draft ids (one id
space), so `{id}` names a proposal or a member request, anything else is 404.

- **List.** Live proposals (`source` `extraction` | `chat`, `tab`
  `extractor`) and submitted member requests (`source` `self_service`, `tab`
  `self_service`), oldest first; items past their deadline but not yet
  closed by the tick are listed with `expired`. `kind` is the v4 change kind
  for proposals and the request type (`new_fixed`, `change_fixed`, `join`,
  `leave`, `swap`) for requests. `version` is the draft version (proposals
  never change it). `preview` is the domain's merge analysis against the
  current schedule (`preview_request` / `preview_proposal`, computed outside
  the writer lock, nothing written): `changes` are the result's field writes
  (`field` = the domain names `slot`, `status`, `participants`, `bosses`,
  `channel`, `day_time`, `note`, `owner`, `new_run`, `new_fixed`, `retired`,
  `answer:<id>`; prefixed `#<short id>` when several rows change; reminder
  rows are not shown), `conflicts` the three-way conflicts (`expected` = what
  the change was based on, `found` = now) plus a `change` line when it no
  longer applies (v4's words for proposals). `flags`: `conflict` (any
  conflict line), `expired`, `requester_frozen` (always false until a freeze
  store exists), `requester_unauthorised` (the requester may no longer have
  it approved), `no_effect` (the merge would write no row at all).
  `choices` (`change_fixed` only, else `null`) lists exactly the amended runs
  the edit would move (`amended: true`); other runs follow the timing, and
  the listed preview assumes `update` for each. `expires_at`: the earlier of
  a proposal's 24 h TTL and the reset ending its boss week; a request's
  boss-week reset, `null` for weekly-only requests. Proposals carry their
  stored card (`confidence`, `is_question`, `summary`, `evidence` read from
  the watched-message cache, `missing` when pruned, `card_url` once posted);
  a proposal whose card was never saved still lists, with `confidence:
  null`, `evidence: []`, `card_url: null`, `summary` = the draft title and
  its target from the draft subject. Requests: `confidence: null`,
  `evidence: []`, `card_url: null`, `summary` and `self_service.note` = the
  member's title (admin-only), `self_service.via` = `request`,
  `public_summary` = the generated `member request: <type> <subject>`.
- **Approve** `{version?, choices?, day?, time?}` (`choices`: amended run id
  → `update` | `keep`). Requests need `version` (422 `version_required`)
  and, for `change_fixed`, `choices` (send `{}` when none are listed); they
  take no edit (422 `edit_not_applicable`). Proposals take an optional
  `version` and no `choices`. Conflicts always block (`force: true` is 422
  `force_unsupported`). A proposal is approved as the signed-in Discord
  user with admin authority, exactly like their ✅ on the card (the same
  record: actor `member:<id>`, surface `extraction_approval` /
  `chat_approval`, request id `approve:<id>`); a Tailscale or token session
  gets 403 `discord_session_required` on proposal approve and reject. A
  request is merged as the session's admin (`request_merge`,
  `merge:<id>@v<version>`); every admin session may decide requests.
- **Edit, then approve** (proposals; v4's portal form): `day` (0–6) and
  `time` (`HH:MM`) in the boss week of the proposal's scheduled instant (a
  move's target, a new run's or a split-off run's slot) replace that
  instant, and the proposal is dry-run and merged as the same single record
  (`approve_proposal_at`), with `edited=<instant>` in the draft's `merged`
  event. The proposal itself never changes, so its card never shows a time
  ✅ would not apply. Authority, TTL, conflict and refusal rules are those of
  a plain approval; an edit equal to the proposed time is a plain approval.
  A change without a time is 422 `edit_not_applicable`; a day or time that
  is malformed, missing its pair, outside that boss week or in a past week
  is 422 `invalid`.
- **Reject** `{version?, reason?}`: requests need `version` and a reason of
  1–500 characters (422 `reason_required` | `reason_invalid`); proposals
  keep v4's reason-free reject (a non-empty `reason` is 422
  `reason_not_applicable`, as nothing would store it). Rejecting a proposal
  past its TTL closes it as expired and answers 200.
- **Refusals.** 409 `stale` (version moved, or decided by someone else) |
  `conflicts` (three-way conflicts, a replay that no longer applies, or a
  proposal refusal in v4's words) | `no_effect` | `requester_unauthorised` |
  `busy`; 410 `expired` (closed as expired by the attempt); 422
  `choices_required` | `choices_not_applicable` | `version_required` |
  `reason_required` | `reason_invalid` | `reason_not_applicable` |
  `force_unsupported` | `edit_not_applicable` | `invalid` |
  `idempotency_mismatch`; 403 `discord_session_required` | `csrf`; 404
  `not_found`; 400 `invalid_body` | `invalid_idempotency_key`; 503
  `unavailable`.
- **Retries.** Every decision is naturally repeatable, with the domain's own
  request ids: repeating a completed approval (same admin; same `choices`
  for a request, same edit for a proposal) answers 200 with the same
  message, and so does repeating one's own rejection with the same reason
  (and a plain approval, or ✅, after one's own edited approval). A request
  re-approved at the same version with other `choices`, a proposal
  re-approved with another edit, or
  a request re-rejected with another reason is 422 `idempotency_mismatch`
  (the edit is part of the approval digest). `Idempotency-Key` is validated
  as for A4 but not stored.
- **Not delivered yet.** The domain returns the merge summary notices, the
  requester notice (approve, reject, expiry) and proposal follow-up errors;
  until serve composition the API drops them, as A4 drops its notices. The
  Discord card is not refreshed either: it keeps its text until the card
  desk re-renders it, and a later ✅/❌ on it is answered in silence (stale).
  A follow-up that failed after a committed proposal merge is named in
  `message`; approving again re-runs it. Approving a proposal retires its
  live siblings about the same target (v4 `commit`).
- **Deferred.** Request edit (`PATCH /api/admin/requests/{id}`: the domain
  takes raw draft operations; no UI contract, like drafts under API-6),
  closed-request history (`?state=`), and every public member route (API-3).

## Logs and rescans (A7)

**Implemented (A7)** over `ModelLogStore` (logs) and a rescan port.

- **Lists** return every match, newest first (the store's keyset pages are
  walked server-side; the PWA pages client-side, so `cursor`/`next_cursor`
  stay proposed and a `cursor` key is `422 invalid_filter`). `total` is the
  unfiltered row count; `facets` are the distinct values stored (`outcomes`
  lists only outcomes that occur, with `withheld`/`clean_retry` when a row is
  flagged so; channel names from the guild's channel list, else the id). An
  outcome of `unknown` (v4-imported rows) is listed and filterable. Empty
  filter values are unset. Chat `summary` is per model over the listed rows
  (`errors` = `error` + `timeout`, `p50_ms` = median latency of answered
  questions). Extractions `model` is the newest call's alias until runtime
  settings (A9) name the configured one. Returning every match is
  deliberate (parent decision: admin-only, bounded by the 90-day log
  retention); the extraction list reads a projection without `prompt` and
  `raw_response` (the detail carries them), and chat rounds are read in one
  query per store page.
- **Chat rows**: `model` is the first round's alias (`—` when no model ran),
  `latency_ms` 0 when unrecorded, `member` `{id: "", name: "unknown"}`
  when the row has none. The turn's `tools` come from the rounds' logged
  calls: `round` is the request round whose reply asked for the call
  (1-based index into `rounds`; `rounds[].round`, `tools[].round` and
  `model_view.rounds[].round` share this logged-position numbering, so a
  clean retry after round N is N + 1 in all three and they join on it), `result` is the logged `result` (v4
  imports: `output`), `took_ms` the logged `took_ms` (v4: `ms`; `null` when
  absent or not a non-negative integer, so unknown and 0 ms differ; rows
  logged before 2026-09-26 show `""`), `rounds[].finish` is the finish
  reason (`""` when none). Each round also carries what the governed session
  actually sent (recorded at send time, so later alias or effort changes do
  not rewrite history): `model` (alias), `effort` (exactly what the body
  carried after capability shaping; `null` when no `reasoning_effort` went
  out, e.g. no reasoning control, or `off` for a model whose published list
  lacks `none`), `route` (`homelab`,
  `external_unmasked`; `external_masked` is historical; `null` for rows recorded before
  routes were), `latency_ms` (`null` when unknown) and `guardrail`
  `{clean, content_filter}`. The turn adds `persona` (bundle id),
  `profile` (reply profile id, `null` for the bundle default) and
  `profile_source` (`saved`/`role`/`default`), `route` (its last round's),
  `error` and a stable `error_code` (`timeout`, `malformed`,
  `content_blocked`, `kept_calling_tools`, `context_budget`,
  `route_refused` (historical),
  `identity_leak_blocked` (historical), `rate_limited`, a governor refusal such as `busy`
  or `queue_timeout`, or a model error such as `upstream_timeout`), the
  row's `guardrail` object, where current external calls set
  `external_unmasked` only after a request is admitted. `masked` and
  `model_view` describe historical masked rows only: their per-round masked
  requests, raw replies and tool-call arguments, decoded final `reply`, and
  `mapping` `[{token, name}]` with display names but no user IDs. Historical
  Model views remain admin-only; new turns have `masked: false` and
  `model_view: null`, and create no mapping or masked row. Withheld turns also
  return `model_view: null`. Historical store data remains under the existing
  chat-log retention and purge behavior. `cards` link the proposals the turn created once their card is
  posted, `raw` joins the rounds' non-empty responses. **Withheld**
  (`chat-orchestration.md`, pollution containment): the question shows as
  `[message withheld]` in the list and the detail, and so do that turn's
  `raw` and tool `arguments` and `result` (they can quote it); `said` (the fixed failure
  line) is kept. A withheld row matches `q` on its reply only, so the text
  cannot be found by search either. Withholding is a chat-surface rule
  (parent decision): the admin-only extraction log shows what the extractor
  read, including text that chat later withheld.
- **Extraction detail**: `messages` are the read messages still in the
  watched-message cache, looked up by id (pruned ones are left out; authors
  by roster name);
  `amendments` are the call's proposals from their stored cards (`when` in
  guild time, else the day/time words), `status` `proposed` | `confirmed` |
  `rejected` | `expired` | `withdrawn` | `superseded` | `discarded`, or
  `missing` for an id no draft has; `refusals` (additive) are the changes
  refused up front (`[{change, code, message}]`, migration `0011`).
- **`error`** (list and detail) is shown only when it is a known typed
  text: the extractor's fixed sentences (`the schedule could not be read`,
  `the channel history could not be read`, `no answer`, …), governor and
  session refusals, redacted provider errors (`LLM completion failed
  (<code>, digest=…)`), timeouts, schema-validation errors, and historical
  identity-decoding or external-route privacy-refusal errors, plus
  `date value out of range`. Anything
  else, including store text in rows logged before this rule and v4 imports,
  reads `The call failed; the server log has the detail.` The extractor logs
  a store failure as a fixed sentence and writes the store's own text only
  to the server log (`extraction_store_failed`), so store or backend text
  never reaches the portal.
- **Rescans.** `POST` validates `{channels, window}` (unknown fields `400
  invalid_body`; empty, unwatched or unknown channels and other windows,
  v4's `2weeks` included, `422 invalid`), then only queues the job
  (`extraction-orchestration.md` "Rescan jobs": a job already covering the
  channels is attached to) and answers `RescanJob`; the runner reads in its
  own task. `state`: queued/running → `running`, done/failed → `done`,
  `cancelled`. Channel `state`: `reading` while read, `done` once read (or
  failed: `errors` = `This channel could not be read.`), else `queued`;
  `messages` = the gated messages of that channel's window. Additive:
  per-channel `unread` (messages the model kept turning away) and `errors`
  (fixed sentences for unread messages, a failed Discord backfill and other
  failures — never the recorded text), and the job's `unread` total. A job
  the bot or `/rescan` started may carry `window` `24h` | `48h` (`2weeks`
  reads `two_weeks`). Per-channel `errors` count only real failures: a
  call the governor turned away and that was read on a retry is not one
  (still-unread messages are the `unread` sentence). `DELETE` cancels: a
  queued job at once, a running one after the call in flight (answered
  `cancelled` at once); a finished job is answered as it is, an unknown id
  is 404. `cancelled` means no further model calls start (parent decision):
  proposals from calls already made may still appear. The worker latches a
  job's final status under the lock a cancel reads, so a cancel accepted
  while the job ran always ends `cancelled` (even if every channel was
  read), and a job already `done` refuses the stop and is answered `done`.
- **Retries.** `POST` and `DELETE` honour `Idempotency-Key` (A4 rules): a
  replay answers the recorded job's current state without submitting or
  cancelling again; the same key with another request (other channels or
  window, another job, or the other verb) is `422 idempotency_mismatch`.
  Keys are kept in memory, scoped per admin actor but evicted as one
  global list of the newest 1024 across all actors: jobs do not outlive the
  process either. A key evicted (or lost to a restart) and then retried
  after its job finished submits a new rescan, which re-reads the channels
  (proposals are deduplicated as for any rescan). `DELETE` is also naturally
  repeatable.
- Every route needs an admin session; `POST`/`DELETE` need CSRF. No rescan
  runner composed: `503 unavailable`.

## Inbox and log filters (built against the mock)

- The inbox contract is "Inbox (A6)" above; the log contract as served is
  "Logs and rescans (A7)".
- `GET /api/admin/chat` and `GET /api/admin/extractions` take `model`,
  `from`, `to` (guild-local `YYYY-MM-DD`, inclusive), `outcome`
  (comma-separated, any of), `channel`, `member`, `q`; Chat also `tool` and
  `min_ms`. Responses add `total` (unfiltered) and `facets` (`models`,
  `tools`, `outcomes`, `channels`). Outcomes — Chat: `answered`, `refused`,
  `clarified`, `error`, `timeout`, `rate_limited`, `turned_away`,
  `content_blocked`, `withheld`, `clean_retry`; Extractions: `proposed`,
  `no_change`, `failed`, `turned_away`, `content_blocked`,
  `self_service_link`, `identity_leak` (historical scanner refusal; current
  calls do not produce it). Historical guardrail keys include `pseudonymized`
  and `identity_leak_blocked`; current external calls record
  `external_unmasked` after a request is admitted. Historical masked chat
  turns retain their Model view on the chat detail (`model_view`, above; admin
  listener only); new turns have none. An unknown outcome, a malformed date, a `min_ms`
  that is not whole non-negative milliseconds (`1e3`, `-5`) or a Chat-only
  filter on Extractions is 422 `invalid_filter`, never a bare 400 (A7 also:
  an unknown, repeated or undecodable key and an inverted range). Cursor paging
  (`cursor`, `next_cursor`) is **proposed**; the mock and the server return
  every match.
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
| `GET /api/admin/requests?state=` | — | `MemberRequest[]` | Folded into `GET /api/admin/inbox` (A6); closed-request history deferred. |
| `GET /api/admin/requests/{id}/preview` | — | `{version, preview, choices?}` | Folded into the inbox item (A6). |
| `PATCH /api/admin/requests/{id}` | edit ops `[{op, field, value}]` + `version` | `MemberRequest` | Deferred (A6): needs an op-edit contract. |
| `POST /api/admin/requests/{id}/approve` | `{version, choices?}` | `{message}` | Folded into `POST /api/admin/inbox/{id}/approve` (A6). |
| `POST /api/admin/requests/{id}/reject` | `{version, reason}` | `{message}` | Folded into `POST /api/admin/inbox/{id}/reject` (A6). |

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
