# pwa-mock

Developer tool (`devtools/`, never shipped): a dev-only server for the web PWAs: serves `web/apps/admin/dist` on :4173 and
`web/apps/public/dist` on :4174 with the production CSP, a CSP report sink, a
synthetic in-memory boss week and same-origin boss/identity art. The e2e suite
starts its own copy on :4373/:4374 (:4383/:4384 for real-art captures) and
checks `GET /__mock/whoami` (`{mock, now, boss_dir, origin}`) before each test,
so a dev server is never mistaken for it. It is retired once the Rust `kanade`
binary serves the apps and the real JSON API.

Environment: `KANADE_WEB_DIR`, `KANADE_BOSS_DIR` (defaults to repo-root
`boss/`, git-ignored art), `KANADE_IDENTITY_DIR` (cached `avatar.*`/`banner.*`;
unset serves generated stand-ins), `KANADE_BOT_NAME`, `ADMIN_PORT`,
`PUBLIC_PORT`, and `KANADE_MOCK_NOW` (RFC 3339 UTC instant, e.g.
`2026-09-29T12:00:00Z`) to pin the clock for tests.

Art is `/art/{portraits,icons,entry}/{key}` (png/webp/jpg/jpeg) and
`/art/animated/{key}` (`artwork/animated/{key}.mp4`, `video/mp4`), which, like
the Rust API, answers single byte ranges (`206`, `416` past the end; multi-range
or malformed `Range` gets the whole file) with `Accept-Ranges`, an ETag and
`If-Range`.

Core primitives showcase (`showcase/`, dev-only): `bun run build && bun run
showcase` in `web/` builds it into `apps/admin/dist/__showcase/`, so the mock
serves it at `/__showcase/` on the admin origin under the production CSP. It
renders every core primitive (`web/packages/ui/src/styles/core/`, no screen
partial) for visual checks; `KANADE_SHOWCASE=1 bunx playwright test
e2e/showcase.spec.ts` captures it per colourway into
`web/e2e/.captures/showcase/`. A plain `bun run build` empties `dist`, so it
never ships.

Admin writes follow the server's API-5 contract (`src/writes.rs`): every
`POST`/`PATCH`/`DELETE` under `/api/admin/` (except the e2e `reset`) needs the
`X-Kanade-CSRF` token that `GET /api/admin/session` answers with (`403 csrf`),
and an `Idempotency-Key` replays the first successful answer (`422
idempotency_mismatch` for another request, `400 invalid_idempotency_key` for a
malformed one). `POST /__mock/csrf/rotate` stands in for signing in again.
The CSRF check is looser than the server's: the server also requires an
`Origin` or `Sec-Fetch-Site` header and checks each one sent
(`src/api/auth/csrf.rs`), while the mock only refuses an explicit cross-site
`Sec-Fetch-Site`, because Playwright's API requests send neither. Do not
read a mock pass as proof that a request carries the browser markers.
`PATCH /api/admin/fixed/{id}` requires `version` (`422 version_required`) and
is `409 stale` per field, as on the server: a form field whose value differs
from the stored timing and that a record after `version` changed (run edits
still check the whole week). Admin changes are attributed as the server does:
`admin:token` for the mock's session, `admin:discord:<id>` in the seed.
Unit tests pin their own clock and ignore `KANADE_MOCK_NOW`.

Sign-in follows admin-api "Sign-in and sessions" (`src/auth.rs`):
`GET /api/admin/auth/methods` offers Discord and the token (no tailnet edge);
Discord start redirects straight to the callback, whose landing page
refreshes to `next` (`POST /__mock/discord {"error": code}` makes the next
start fail with `/?login_error=<code>`); `POST /api/admin/auth/token` takes
the mock token `kanade-mock-token` (401 otherwise, 400 for a bad body);
`POST /api/admin/auth/logout` needs CSRF. Signed out, every admin route but
sign-in answers `401 unauthenticated`. There is one global mock admin and no
cookie. `GET /api/admin/session` sends `{display, method}`.

The public origin follows `docs/notes/member-auth-contract.md` §1–§3
(`src/public.rs`, `src/mock/portal.rs`): `GET /api/public/status` reads the
admin Config switch `self_service.public_portal` (the mock always has its
stand-in for the public Discord keys); Discord start redirects straight to the
callback, which signs in the one mock member (Asahi, id `100000000000001001`)
with an `HttpOnly; SameSite=Strict` cookie named `kanade_pub` (the server's
`__Host-` name needs HTTPS) and a landing page that refreshes to `next`.
`GET /api/public/session` (+ `X-Kanade-CSRF`), `/session/avatar`,
`/sessions`, `DELETE /sessions/{handle}` (`404`, `409 current_session`),
`POST /sessions/end-all` and `POST /auth/logout` answer as the contract says
(`503 closed` while closed, logout excepted; writes need the session's
token). The first sign-in finds two other devices already signed in; at most
ten sessions live, oldest ended first. Member reads (`src/mock/member.rs`,
`Store::member_allowance`): `GET /api/public/week?week=` (absent or `this`,
`next`; else `422 invalid_query`) is the admin week as `MemberWeek` — every
run, `mine`/`can_edit` for Asahi, participant ids snowflake-shaped like the
session's member id, no `short_id`, `channel_id`, `cards`, `amended` or
`roster_change`; `GET /api/public/me/allowance` is an invented member-side
window (20 per 6 h, seven used, cleared by the admin reset of `1001`; second
in the queue while the model is busy); `GET /art/{kind}/{key}` serves the
admin listener's art to a signed-in member. Boss guides (`src/public.rs`,
`mock/knowledge.rs` `PublicKnowledge`): `GET /api/public/bosses` and
`/bosses/events` are the admin list and event bosses as they are;
`GET /api/public/bosses/{key}/knowledge` is the admin page without `path`
and with every bullet's chatbot-only `detail` removed (`404` for an unknown
key). All of these answer `401
unauthenticated` signed out and `closed` while closed. Every other
`/api/public/` path answers `closed` while closed and `404` while open.
Weekly-timing ownership (`src/mock/ownership.rs`, the admin Inbox's requests,
as `src/api/public/ownership.rs`): `GET /api/public/timings` lists the live
timings Asahi is on (`MemberTimings`, snowflake-shaped ids) with the live
requests on each, all of them on her own timings and otherwise only hers
(Mika's on Carling shows, Tsubame's on Kalos does not).
`POST /timings/{id}/owner {to}` hands off at once (`422 invalid_body` for a
`to` that is not 17-20 digits or another field; it supersedes the timing's
open requests); `POST /timings/{id}/owner-requests` asks (`201`, the same key
`200` with the request as it now is, another timing under that key `422
idempotency_mismatch`, a second open ask `409 already_asked`);
`POST /owner-requests/{id}/accept|decline` (owner) and `/withdraw`
(requester) answer `MemberOwnerRequest`. Refusals are the server's: `403
not_owner`/`not_requester`, `409 not_on_party`/`already_owner`/
`request_closed`/`request_expired`, `404 not_found`. Every write needs the
session's token and an `Idempotency-Key` (`400 invalid_idempotency_key`);
hand-off and accept also need the 15-minute fresh window after sign-in
(`401 reauth_required`; the pinned clock never ages a sign-in, so
`POST /__mock/public/unfresh` does).
Only the ask and the hand-off replay by key, as on the server; a repeated
accept, decline or withdraw is refused. Member writes are not rate-limited.
Changes show in the admin Inbox and emit `inbox` + `schedule` hints.

Member writes (`src/member_writes.rs`, `src/mock/member_runs.rs`,
`src/mock/requests.rs`, as the member-writes contract): `PUT
/api/public/runs/{id}/answer {answer, version}` (yes/maybe/no; a no puts a
live run at risk) and `POST /runs/{id}/move {day, time, version}` (this boss
week, not started, to a slot after now; own-time runs keep their clock on
`null`) answer `MemberRunResult`/`MemberMoveResult`; refusals `404
not_found` (also last week's runs), `403 not_in_run`, `409
run_closed`/`week_over`/`run_started`/`stale`, `422
invalid_body`/`invalid_time`/`in_the_past`. Staleness follows the history
records newer than `version`: only a change to the run's slot (move) or the
caller's own RSVP (answer) is `409 stale`. `GET /runs/{id}` is
`MemberRunLink` (this, next or a past week the member was on; `removed` from
the record that took her off; the seeded last-week `p-kalos` points at this
week's `r-kalos`). Requests: `GET /api/public/requests/mine`
(`MemberRequests`, seeded with a waiting weekly change, approved, rejected,
withdrawn and expired ones), `POST /api/public/requests` (`201`, an exact
retry `200`, the key with another body `422 idempotency_mismatch`; `429
request_limit` with `limit: open|today` at 3 waiting or 6 sent in 24 h;
`404`/`409 already_in_party`/`no_effect`/`422 field_not_allowed`) and
`POST /requests/{id}/withdraw` (no fresh sign-in; `409 request_closed`).
Answer, move and submit need the fresh window; every write replays by key.
Requests sent here stay in the member's list only: the admin Inbox's
self-service items are its own seed, and no admin decides the portal's.

Mock control, public origin only: `POST /__mock/public/sign-in` signs in without the
redirects (cookie + token), `POST /__mock/public/discord {"error": code}`
makes the next start end with `/?login_error=<code>` (`not_eligible` sets no
cookie), `POST /__mock/public/end` ends every session (expired or no longer
eligible), and `POST /__mock/public/rotate` makes each session rotate its id
and token on its next request, as after a client IP change. Session lifetimes
are not simulated: `POST /__mock/public/unfresh` ages every sign-in past the
fresh window (until the next sign-in). `POST /__mock/public/remove {"run":
id}` has Ren take Asahi off a run (a recorded admin edit), and `POST
/__mock/public/end-week` makes this boss week's runs read as past (`week:
"past"`, writes refused) as after a reset with the page open.

The mock's admin signs in with Discord as Asahi (staff, `admin:discord:1001`)
after every `POST /api/admin/reset`; `POST /__mock/session {"method":
"discord" | "token" | "tailscale" | "none"}` signs in again another way (new
CSRF token) or signs out. The Inbox follows admin-api "Inbox (A6)": extractor and chat
proposals plus member requests (`new_fixed`, `change_fixed`, `join`,
`leave`, `swap`, `via: "request"`); `change_fixed` choices list only the
amended runs; conflicts always block (`force` is `422 force_unsupported`);
proposals take one edit (`day`, `time`) for a move, new run or split and are
decided only by a Discord session (`403 discord_session_required`), as the
approving member (`extraction_approval` / `chat_approval`); requests by any
session (`request_merge`). Repeating a decision answers 200 with the first
message (`422 idempotency_mismatch` if it differs).
`GET /api/admin/inbox/past?before=&limit=` serves ten invented closed items
(every outcome, both kinds, newest closed first; the approved proposal links
History record 3), paged by the last id shown as the server does; decisions
made in the mock session do not join it.
`GET /api/admin/inbox/ownership` lists two open weekly-timing ownership
requests on seeded timings (Mika for `f-carling`, asked 20 h ago; Tsubame for
`f-kalos`, 5 h ago), oldest first, and the summary's `inbox` count includes
them. `POST /api/admin/inbox/ownership/{id}/accept` pins the requester as the
timing's owner (recorded as an `admin_portal` change, closing the timing's
other open requests) and `/decline` leaves the owner; any session decides,
Discord or not. A repeat of the same decision answers 200 with the first
message; any other decision on a closed request is `409 conflicts`, an
unknown id `404 not_found`.

Roster characters (module `roster`, `src/mock/characters.rs`; the mock always
serves them, as the server does with `modules.roster` on): `GET`/`POST
/api/admin/members/{id}/characters` and `PATCH`/`DELETE
/api/admin/characters/{id}` follow the server's rules (IGN 4-12 letters or
digits, unique per world case-insensitively, `422 ign_taken`; the first
character is the main, `role: main` moves the crown, `409 main_required` for
retiring, demoting or deleting the main while others are active; `PATCH`
needs `version`, `409 stale`). Invented characters are seeded
(`characters::seed`; Ren #1013 has none); `POST /api/admin/reset` restores
them. Writes record no History rows here.

Character data (module `character_data`, `src/mock/character_data.rs`; on
with the roster, `POST /__mock/modules {"character_data": false}` turns it
off until the next reset): `GET`/`POST`/`DELETE
/api/admin/characters/{id}/link` and `POST /api/admin/characters/{id}/refresh`
follow the server's shapes and refusals (`version_required`, `stale`,
`already_linked`, `retired`, `region`, `not_linked`, `not_connected`,
`503 nexon_unavailable`). A request stays `pending`: the
mock has no Discord confirmation and never calls an Open API. Wholly
invented data at the pinned now: MikaBish (`ch-s3`), MikaNL, YuzuPala
(`ch-s8`) and TsubaKain (`ch-s9`) linked (MikaBish, YuzuPala and TsubaKain by
today's 04:01 batch), MikaHero linked but retired, Mikadere stale, Hotaru7
(`ch-s10`) linked with its data past 30 days (`expired`), AsahiDK (`ch-s5`)
pending, the rest not linked. An admin Refresh is always allowed (only held
calls refuse it); every link carries an invented `privacy_url`. `GET
/api/admin/config` carries the `character_data` health block while the module
is on; `POST /__mock/character-data {health}` picks `ok` (default), `alert`
(Nexon backoff, scouter `verification_required`, a stuck and an unknown card),
`setup` (no key: links read `not_connected`; no scouter URL: estimates off) or
`refused` (key refused, scouter unreachable).

The Add character lookup (`GET`/`POST /api/admin/characters/lookup`, on with
the module) answers by IGN prefix, case-insensitive: `Nope…` 404
`not_found`, `Busy…` 503 `nexon_unavailable` (held), `Offline…` 409
`not_connected`, `Wait…` 429 `lookup_cooldown`; any other MapleSEA IGN is
found with an invented class and level (stable per name), world Aquila and
today's Singapore date. The `setup` health is not connected and `alert` /
`refused` hold every lookup; there is no real per-admin spacing here.
Nothing is kept. A character create answers `CharacterCreated`: with the
module on and connected, a MapleSEA (`msea`) create also makes the member's
`pending` link (`link: "pending"`), otherwise `link: null`.

Member characters on the public origin (`src/member_characters.rs`,
`src/mock/member_characters.rs`, as `src/api/public/characters/`): `GET`/`POST
/api/public/me/characters` (`MyCharacters`; Add answers `201
MyCharacterCreated`, asks for no link, `422 class_level_required` without class
and level unless MapleSEA and connected), `PATCH /api/public/me/characters/{id}`
(the admin rules; no DELETE), `POST …/characters/lookup` (the admin lookup's
prefixes), `GET`/`POST`/`DELETE …/characters/{id}/link`, `GET
…/{id}/link/notice` (notice v1 word for word), `POST …/{id}/link/cancel`,
`DELETE …/characters/links`, `GET /api/public/me/data`, `GET`/`POST
/api/public/me/estimates` and `GET …/estimates/{job}` (only Asahi's own
characters; one counted start per 30 minutes, `429 member_limited` with
`next_at`, a failed lookup not counting; the pinned clock never ages it, so it
lasts until the reset) and `GET /api/public/runs/{id}/characters` (a run
Asahi is on: roster levels for the others, her own NEXON level, cached
readings per boss). Each answers `closed` while closed, then the generic 404
while its module is off (`roster`; `character_data`; estimates for the
lookups), before the session; another member's character or job is the
generic 404. Add, Edit and Link need the fresh window; Unlink and Unlink all
the token and key only. Link outcomes by IGN prefix: `Nope…` `not_found`,
`Busy…` `busy`, `Later…` `linked_later`; the `alert`/`refused` health is
`unavailable`. `POST /__mock/public/characters {"roster": "board"}` draws
Asahi's characters as the member-characters boards do (AsahiKanna linked
main with one share card, AsahiDK pending on Ren's request, AsahiPath stale,
AsahiBlaze expired, AsahiMech retired; ids `ch-b1`…`ch-b5`), `"empty"` gives
her none; the other board states come from `/__mock/modules`,
`/__mock/character-data {"health": "setup"}` (not connected) and
`/__mock/estimates {scenario}`. Not rate-limited otherwise.

Run readiness (`src/mock/readiness.rs`; on with the module):
`GET /api/admin/runs/{id}/readiness` answers each party slot as the server
does: its level (fresh NEXON level else the manual one) and power (NEXON
combat power from fresh data only; shown, never judged), and its level
against each boss's entry level from boss knowledge. `r-carling`'s party
covers every level verdict.

MapleScouter estimates (`src/mock/estimates.rs`; on with the module):
`POST /api/admin/estimates {character_ids | run_id, refresh?}` (`202
{job}`, `429 estimate_queue_full` past 16 waiting, the server's 422/404/400
refusals), `GET /api/admin/estimates/{job}` and `GET
/api/admin/estimates?character_ids=|run_id=` (`{job, running}`). Jobs wait
in one FIFO line (`queued` with `position`) and run one at a time; a target
already queued or running answers that job. There is no sidecar: each
`GET …/{job}` read of a running job finishes one character (its outcome and
result appear at once), and a party takes one more read for its total; a
repeat lookup of the same target answers `done` with `cache_hit` unless
`refresh`. `POST /__mock/estimates {scenario}` sets later lookups until the
next reset: `ok`, `queued` (waits behind two invented lookups, one place per
read), `assumed` (the first character's 89% critical rate assumed at 100%),
`busy` (429, 40 s), `critical_rate` (422), `manual_check` (503), `failed`
(502), `timeout` (504), `hold` (stays running; later lookups queue behind
it) or `off` (no scouter URL: every route 404). An error fails the last
character only: the others are found and the total covers them (one
character alone fails the job); the last character of a party has no Star
card (an incomplete total). Seeded: a done lookup of MikaNL (`ch-s2`).
Wholly invented numbers.

Run character picks (module `roster`, `src/mock/run_picks.rs`): `GET
/api/admin/runs/{id}/characters` and `PUT
/api/admin/runs/{id}/characters/{member}` `{character_id | null, version}`
follow the server's rules (active characters as candidates, `inferred`
computed on read, warnings for the same character on another run of a shared
boss this boss week; `version_required`, `invalid`, `not_on_run`,
`not_found`, `not_their_character`, `character_retired`, `409 stale`;
deleting a character drops its picks). The mock does not model frozen runs,
so it never answers `run_ended`. Seeded for the run-sheet boards: Asahi
picks AsahiDK on `r-carling`, Mika has three candidates and no pick, and
RenShade is inferred on `r-carling` and on the extra done `r-nstar` (Normal
Star, Sat 21:00), so it warns.

Live updates: `GET /api/admin/events` answers as the server's stream does
(`ready {seq, boot}` with one `boot` id per mock process, then `{topic, seq}` hints with no data), emitted after every
successful admin write (`runs/*`, `fixed`, `history/*` → `schedule`;
`inbox/*` → `inbox` + `schedule`; `config` → `settings`; `digest` →
`delivery`; `rescan` → `rescan`; `members/*` → `members`). With no streaming body, each response is
short: `EventSource` reconnects after 200 ms with `Last-Event-ID` and the next
response holds (up to 10 s) for a newer hint, so pages see the same events in
order. `POST /__mock/arrive {"kind": "reaction" | "move" | "run" | "proposal" | "chat" |
"extraction" | "member"}` stands in for a change made outside the portal
(Ren's ✅ on Kalos, another admin moving Hard Limbo to Thursday 21:00 or
adding a Normal Limbo run on Thursday 20:00 (`run`), a new extractor proposal, a chat turn, an extraction
call, Mika's new alias `mikan` from a roster sync), changes
the reads and emits the matching hint; `POST /api/admin/reset` clears them.

Admin JSON reads carry an `ETag` (std SipHash of the body; the server's is a
SHA-256 prefix) and answer `304` with the same headers and no body when
`If-None-Match` names it, as the server does.

Names: `GET /api/admin/roles` (three guild roles, one colourless),
`Identity.bot_user_id` (`1543532497948909578` on the admin origin, null on
the public one), `author_id` on inbox evidence and extraction messages, inbox
`thread` (cited messages `used`, gone ones absent; null for requests), and
chat `member_id`. Config models mirror the server's capacity report: the
default source runs one `gateway` group of `models.permits` over the role
aliases (`groups_source: "default"`; `declared_groups` in the store switches
to `config`), `key_limits.max_in_flight` is null, `capacity_check` holds only
the server's per-group verdicts (and ungrouped-role warnings with declared
groups), the catalog lists `model:level` variants (`variant_of`,
`fixed_effort`) and `kanata/think` requires reasoning (`off_allowed: false`).
Each check names its `group` (null for ungrouped-role warnings), env rows
carry `copy` values, and `last_digest` is the current boss week's Thursday
00:15 post in `#boss-schedule` until a manual digest post replaces it.

History records carry domain rows as the server encodes them (run instants
in UTC, `rsvps.state`, weekly timings with Monday = 0, unsent `reminders`
rows derived from each run's cards), name weeks by their starting RFC 3339
instant (the `week` query also takes the local start date), list one run's
change log for `run=<id>` (records changing its row or RSVPs; with `week` or
`actor`, or for an id with neither a run nor history, `422 invalid_query`),
and answer a strict rollback conflict with `rows: []`
and every selected seq in `reverts` (revert, restore-week and revert-actor).
The hash is still a stand-in. Checkpoints report a configured backup
directory with one backup in each anchor state (`matches`, `mismatch`,
`older_schema`), newest first.

`cargo test` also walks every endpoint the PWAs call and validates each
response against `docs/v5/api-schemas` (`src/contract.rs`).

Checks: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
