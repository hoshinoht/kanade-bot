# pwa-mock

Dev-only server for the web PWAs: serves `web/apps/admin/dist` on :4173 and
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
instant (the `week` query also takes the local start date), blame under the
domain's field names, and answer a strict rollback conflict with `rows: []`
and every selected seq in `reverts` (revert, restore-week and revert-actor).
The hash is still a stand-in.

`cargo test` also walks every endpoint the PWAs call and validates each
response against `docs/v5/api-schemas` (`src/contract.rs`).

Checks: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
