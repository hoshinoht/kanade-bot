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
`PATCH /api/admin/fixed/{id}` requires `version` (`422 version_required`) and
is `409 stale` when the week moved since, checked like run edits (whole week,
where the server checks per field). Unit tests pin their own clock and ignore
`KANADE_MOCK_NOW`.

`cargo test` also walks every endpoint the PWAs call and validates each
response against `docs/v5/api-schemas` (`src/contract.rs`).

Checks: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
