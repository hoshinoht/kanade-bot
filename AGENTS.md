# Kanade bot repository guide

## Repository layout

- Root = Rust v5 crate `kanade` (`Cargo.toml`, edition 2024, toolchain pinned in `rust-toolchain.toml`). `legacy/python/` = frozen v4 rollback (Python), independently runnable.
- `src/main.rs` installs the rustls `ring` provider and delegates to `src/runtime/` (command dispatch, env-only config, JSON logs, TLS); `src/cli/` parses `serve`, `healthcheck` and reserved `ctl`/`import`/`export`; `src/api/` is the bootstrap health server; `src/chat/persona/` loads the v5 persona layout.
- Feature code: `src/domain/` (pure rules), `src/extract/` (pure extraction rules), `src/infrastructure/` (`llm/` provider, `store/` SQLite + journal), `src/bot/` (Discord). Each has its own `AGENTS.md`.
- `tests/<target>/main.rs` integration suites (see `tests/AGENTS.md`); `docs/v5/` contracts, decisions and frozen v4 vectors (see `docs/v5/AGENTS.md`).
- `config/personas/` tracks only `README.md`, `catalog.example.yaml`, `bundles/kanade.yaml`, `profiles/example.yaml`; everything else there (and `config/personas-v4/`, mounted by the v4 container) is private.
- `web/` is the production Svelte 5 PWA workspace (see `web/AGENTS.md`); `tools/pwa-mock/` is its dev-only Axum mock server (own Cargo project); the stack-evaluation spike was removed (restore from commit `6aecff4` if needed); `scripts/` holds the v5 inventory checker (`check_v5_inventory.py`, `v5_inventory/`), `boss_knowledge/` import tooling and `bench_headers.py`.
- `boss/knowledge/` is the tracked v5 boss knowledge (schema v2); v4's copy under `legacy/python/boss/knowledge/` must not change because the frozen v4 container validates it at startup. Root `boss/portraits` and `boss/artwork` are private, git-ignored art.
- Current planning state lives in git-ignored `.opencode/workplan/kanade-v5-roadmap.{json,md}`; its `## Decision register` records user decisions that override older plan text. Predecessor plans are marked archived; their historical decisions and receipts remain preserved.

## v5 toolchain and checks

- CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --locked --all-targets --all-features`, `cargo build --locked --release`.
- Targets `provider_contract`, `scheduler`, `notify`, `store`, `discord`, `delivery`, `governor`, `extract`, `api`, `chat` are declared in `Cargo.toml` with `required-features = ["test-support"]`; run one with `cargo test --all-features --test <name>`. `domain`, `persona`, `runtime_bootstrap` are auto-discovered from `tests/<name>/main.rs`.
- The suite is offline: fake Discord/model providers, loopback stubs, temp stores. Never read `.env`, `data/` or private `config/` from tests.
- Only `serve --offline` and `healthcheck` run today; config comes from the process environment (`KANADE_TIMEZONE` required, `KANADE_ADMIN_BIND` and optional `KANADE_PUBLIC_BIND` loopback-only). The admin and public routers are separate: admin routes are never mounted on the public listener. See `docs/v5/runtime-bootstrap.md`.
- Pin new dependencies exactly (`=x.y.z`) with minimal features; keep rustls on `ring` only (no aws-lc/native-tls/openssl).

## v4 rollback toolchain

- The v4 rollback tree is `legacy/python/`; run its Python commands from that directory.
- Use Python 3.12 and `uv`; run `uv sync --locked` there before v4 checks.
- A focused v4 test is `cd legacy/python && uv run pytest -q tests/test_<area>.py::test_<case>`.
- `uv run pytest` excludes the `live_model` marker through pytest config and needs neither Discord nor a model. `uv run pytest -m live_model -v` calls the real Kanata gateway and skips unless `KANATA_BASE_URL`, `KANATA_API_KEY_FILE` and `EXTRACT_MODEL`/`CHAT_PILOT_MODEL` are set; narrow the chatbot smoke test with `-k chat_live`.
- Match v4 CI from `legacy/python/` with Ruff, stylesheet generation, and the non-live-model suite; the rollback image is `docker build -f legacy/python/deploy/Dockerfile legacy/python`.
- Optional local hooks are enabled with `git config core.hooksPath .githooks`; they cover only `legacy/python/` (pre-commit lock check, Ruff format/re-stage and lint; pre-push non-live-model suite) and skip when `uv` is missing.
- If the repository moves and `.venv` commands report a bad interpreter, repair their absolute shebangs with `uv sync --reinstall`.

## Where v4 behavior lives

- Within v4, scheduling rules are under `legacy/python/bot/domain/`, persistence under `legacy/python/bot/infrastructure/`, and adapters under `legacy/python/bot/agent`, `extract`, `chat`, and `api`.
- v4 `bossctl` remains an HTTP client; do not add a second scheduling path or make it manipulate the live SQLite file directly.
- v4 catalogs, examples, docs, and container files are under `legacy/python/`; private deployment state remains at its existing root paths until manually mounted by an operator.
- `legacy/python/tests/` mirrors v4 behavior by feature and supplies Discord/model fakes; v4 guides are under `legacy/python/docs/`.
- `legacy/python/scripts/bench_extract.py` and `legacy/python/deploy/` (Dockerfile, its `Dockerfile.dockerignore`, Compose) are v4 rollback tooling; v5 container files belong in the root `deploy/` directory. TLS ingress is the shared edge (`~/projects/personal/homelab/edge`, site `sites/kanade`; Kanata lives in `~/projects/personal/homelab/kanata`) over the internal `kanade_edge` network; kanade no longer ships Caddy.

## Generated, coupled, and private files

- Never edit generated, git-ignored `legacy/python/bot/api/static/portal.css`; validate it from `legacy/python/` with `python -m bot.portal_styles`.
- Boss portraits and entry artwork are intentionally git-ignored deployment assets. Their tests isolate themselves from whatever images happen to exist locally.
- Treat root and `legacy/python/` `.env`, data, guide, and live persona paths as deployment-private.
- Full Compose startup also expects the externally managed volume `kanade_botdata` and the private `legacy/python/.env`. v4 model calls go only to the Kanata gateway (`KANATA_BASE_URL`, https) through `legacy/python/bot/infrastructure/llm/`; Compose mounts the bearer key as the `kanata_api_key` secret from `KANATA_API_KEY_HOST_FILE`, never as an env var. Do not reintroduce the `ollama` package or native Ollama endpoints in `bot/`.

## Coding policy

- Keep comments and docstrings concise.
- Prefer small, cohesive modules over monolithic files. Split by responsibility and keep orchestration thin; avoid arbitrary fragmentation or abstractions used only to reduce line counts.
- Organize new release code and tooling into responsibility-based subdirectories, not flat catch-all directories. Do not grow monolithic hand-written files; generated schemas are artifacts, not a reason to combine their source generators.
- Grow Rust features under `bot/`, `chat/`, `extract/`, `api/`, `domain/`, `infrastructure/`, `cli/`, and `runtime/` as code lands. Keep entrypoints thin; do not put feature logic in a catch-all runtime module or create empty placeholder modules.
- Read the nearest nested `AGENTS.md` before changing a subsystem; local files contain only subsystem-specific guidance.

## CHANGELOG
- when new features are added, changed, or bugfixes are made, add a changelog entry with a brief description.


<!-- recall:lessons:begin -->
- Keep chatbot schedule language distinct from storage boundaries: unqualified `this`/`next week` is calendar Monday–Sunday, explicit `boss week` uses the configured reset interval, and bare weekdays resolve forward; calendar/date reads may therefore merge multiple boss-week buckets while API/CLI/write paths retain boss-week semantics.
- Keep seeded chatbot tests on one fixture-scoped aware clock: pass the same instant into week materialization and patch each imported clock seam used by tools, commits, API service, and test helpers so reset-day rollovers cannot change the suite.
- Personal chatbot memory is removed (v4 schema v16 drops `chat_memor*` after the pre-upgrade snapshot); do not reintroduce per-member memory. The delivery-target CHECK still accepts `memory_*` only so retained journal rows stay valid history.
- Schedule retention by monotonic elapsed time while passing an aware wall-clock instant into persistence, so system clock rollback cannot suppress expiry and purge work.
- Keep operational detail pages in a fixed `100dvh` shell with one tabbed window filling the remaining height: the document/body, masthead, back navigation, human identity, and tab strip never scroll; only the selected panel scrolls, including on narrow screens. Never fall back to stacked card windows, whole-window movement, or document-body scrolling.
- On every existing-store (v14+) Repo reopen, enable connection-local SQLite foreign-key enforcement before using delivery attempts; migration-time PRAGMAs do not persist across connections, including FROZEN maintenance restart.
- A bound Discord delivery claim survives native row retirement: replace a digest only after confirmed remote deletion, then atomically retire the exact bound attempt, release its target with actor/reason, and retire the native row under the same live lease; ambiguous deletion must suppress replacement.
- Record every new user decision or scope change for v5 in the workplan as it happens: a JSON note in the active workplan (`.opencode/workplan/kanade-v5-roadmap.json` since 2026-09-30; predecessor plans are archived) AND a line in its `.md` `## Decision register` (resume packets omit old notes), then refresh the checkpoint at milestones so a compacted or fresh session resumes with full context.
- macOS checkouts are case-insensitive but CI (Linux) is not: file-backed tests (boss art keys such as `MaleficStar`, persona files) must use mixed-case fixtures and exact-case paths, or case bugs pass locally and fail in CI.
- Parallel Rust slices run in detached worktrees (`../kanade-v5-<lane>`) and are integrated into `kanade-v5` with `git diff --cached --binary` + `git apply --3way`, re-running fmt/clippy/full tests before each commit; `git apply --3way` stages everything, so `git reset -q` before splitting one patch into several commits by path, and stage a slice's own paths rather than whole directories (`git add -A docs` once swept untracked research captures into a commit). When building a commit's path list from a patch, include both sides of `rename from`/`rename to` (taking only the `a/` side once committed a deleted `tests.rs` without its new `tests/mod.rs`), then build every intermediate commit before moving on. Review agents cannot read `~/projects/personal/homelab/kanata`; use a `researcher` for read-only Kanata contract checks.
- The v5 container has a fixed IP on `kanade_edge` (192.168.97.10), so `docker compose -f deploy/compose.yaml run … bot <cmd>` fails with "Address already in use" while `kanade-v5` runs: use `docker exec kanade-v5 /usr/local/bin/kanade <cmd>` for read-only checks, and stop the bot first for store-owning commands (`import v4`, `--refresh-logs`).
- The admin app shows "This isn't available on this server yet" for any `/api/admin/*` route the Rust API has not mounted (the pwa-mock serves them all); find the gaps by comparing the `/api/` paths the web apps call with the `.route(` paths in `src/api`. chrono's `format()` is not compiled in: format dates by hand (see `src/api/admin/config/access.rs`).
- Web e2e serves the built `dist`, not the sources: run `bun run build` before `bun run e2e`/`bunx playwright test`, or stale bundles fail new specs. The e2e mock ports 4373/4374 are shared by every worktree, so serialize e2e runs across parallel lanes (never kill another lane's mock).
- v5 deploy: tag the running image `kanade-v5:rollback-<sha>`, `docker compose -f deploy/compose.yaml build`, stop the bot, tar the `kanade_v5_data` volume with a throwaway `alpine` container into `~/.config/kanade/v5/backups` (0600), `up -d`, then `docker exec kanade-v5 /usr/local/bin/kanade healthcheck`.
- External chat, extraction and rewrite models receive raw member names, IDs, messages and URLs when present; Kanata provider ZDR concerns retention, not transmission. Neither masking nor an external-unmasked opt-in exists. Both retired privacy TOML/env keys refuse startup even when false or empty; preserve historical `chat_masked` rows and use wholly invented fixtures for model-bound tests, never real transcripts.
- Discord shutdown must track and abort the actual interaction task, not only a wrapper awaiting its `JoinHandle` (aborting that wrapper detaches the command); share one bounded task-drain deadline, join aborted tasks before closing the store, and gate the first delivery tick on completed roster reconciliation.
- A chat model may copy the bot's display name or invent an unrecognized mention in `get_schedule.participant`; recover the asker only from a trusted, unambiguous self-only source question and an exact bot name or one unknown mention. Never turn mixed/third-person or arbitrary unknown handles into self.
- Kanata answers an over-cap `max_tokens` and an unsupported size field with the same `400 invalid_request` (capability check first); treat it as a non-downgrading `size-limit` only when the sent value exceeds the alias's published `max_output_tokens`, otherwise downgrade `sampling_controls` as usual.
- A `Drop` guard that may run during unwinding (e.g. chat `Held`) must not repeat work that can panic (directory lookups): a second panic aborts the process. Defer the full cleanup to a spawned task and keep a panic-free settle/refund fallback; clear the armed flag only after the fallible call returns.
- Extraction marks rows read (`mark_read_exact`, all-or-nothing) before any effect; every non-consuming outcome (stale version, store error, contention) must `claim.retry` the call's rows, or unedited siblings sit unprocessed until the next startup rescan.
- Reconnect extraction catch-up is deferred: a submission cutoff on rescan rows alone cannot keep later edits out of model prompts. Any future design must apply the same latest-mutation boundary to Discord backfill, cached candidates, and `Extractor::load` context before model bursts; do not treat a queued pass as safe until a deterministic edited-message test proves all three.
<!-- recall:lessons:end -->
