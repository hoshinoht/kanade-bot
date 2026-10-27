# API guide

- Two routers on separate listeners: admin (`admin/`) and public (`public/`, the member portal). Admin routes are never mounted on the public listener. Wire types live in `dto/`; every DTO the web apps read is listed in `ts_bindings.rs` (regenerate `generated.ts` after a change, root `AGENTS.md`). Request and response shapes are documented in `docs/v5/api-schemas/`.
- The public `Site` has live `ApiState` for member reads, so every shared handler that reads `site.state` also answers on the public origin: gate admin-only facts by `site.origin` (public `/api/identity` once leaked `bot_user_id`), and keep session-gated public responses off the shared `public, max-age` cache rules (public `/art/` is `private, no-cache`). The `rust_e2e_fixture member-portal` site needs read state too, or every member read answers 503 only in `e2e:rust`.
- The admin app shows "This isn't available on this server yet" for any `/api/admin/*` route the API has not mounted: `python3 scripts/api_routes/route_diff.py` lists the gaps between the paths the web apps call and the `.route(` paths here.
- chrono's `format()` is not compiled in: format dates by hand (see `admin/config/access.rs`).
- Fixed-PATCH replay identity is the full normalized incoming form, not a diff against current state (`admin/write/fixed/replay.rs`). Its fixed-only path rechecks the recorded identity after both successful and refused transactions, because the legacy `check_no_op` can swallow an atomic mismatch. Discord digest pins stay unchanged; pre-fix PATCH keys fail closed and need a reload and a fresh key.
- Check: `cargo test --all-features --test api`.
- Never import anything from `src/bot/` here; shared code comes from `src/app/`, `src/discord/` and `src/cards/` (see their guides).
