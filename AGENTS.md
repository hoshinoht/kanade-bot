# Kanade bot repository guide

## Toolchain and checks

- The v4 rollback tree is `legacy/python/`; run its Python commands from that directory.
- Use Python 3.12 and `uv`; run `uv sync --locked` there before v4 checks.
- A focused v4 test is `cd legacy/python && uv run pytest -q tests/test_<area>.py::test_<case>`.
- `uv run pytest` excludes the `live_model` marker through pytest config and needs neither Discord nor a model. `uv run pytest -m live_model -v` calls the real Kanata gateway and skips unless `KANATA_BASE_URL`, `KANATA_API_KEY_FILE` and `EXTRACT_MODEL`/`CHAT_PILOT_MODEL` are set; narrow the chatbot smoke test with `-k chat_live`.
- Match v4 CI from `legacy/python/` with Ruff, stylesheet generation, and the non-live-model suite; the rollback image is `docker build -f legacy/python/deploy/Dockerfile legacy/python`.
- Optional local hooks are enabled with `git config core.hooksPath .githooks`; pre-commit may format and re-stage Python files, while pre-push runs the non-live-model suite.
- If the repository moves and `.venv` commands report a bad interpreter, repair their absolute shebangs with `uv sync --reinstall`.

## Where behavior lives

- `legacy/python/` is the independently runnable v4 rollback implementation; Rust v5 work belongs at the repository root.
- Within v4, scheduling rules are under `legacy/python/bot/domain/`, persistence under `legacy/python/bot/infrastructure/`, and adapters under `legacy/python/bot/agent`, `extract`, `chat`, and `api`.
- v4 `bossctl` remains an HTTP client; do not add a second scheduling path or make it manipulate the live SQLite file directly.
- v4 catalogs, examples, docs, and container files are under `legacy/python/`; private deployment state remains at its existing root paths until manually mounted by an operator.
- `legacy/python/tests/` mirrors v4 behavior by feature and supplies Discord/model fakes; v4 guides are under `legacy/python/docs/`.
- `legacy/python/scripts/bench_extract.py` and `legacy/python/deploy/` (Dockerfile, its `Dockerfile.dockerignore`, Compose) are v4 rollback tooling; v5 container files belong in the root `deploy/` directory. TLS ingress is the shared edge (`~/projects/personal/edge`, site `sites/kanade`) over the internal `kanade_edge` network; kanade no longer ships Caddy.

## Generated, coupled, and private files

- Never edit generated, git-ignored `legacy/python/bot/api/static/portal.css`; validate it from `legacy/python/` with `python -m bot.portal_styles`.
- Boss portraits and entry artwork are intentionally git-ignored deployment assets. Their tests isolate themselves from whatever images happen to exist locally.
- Treat root and `legacy/python/` `.env`, data, guide, and live persona paths as deployment-private.
- Full Compose startup also expects the externally managed volume `kanade_botdata` and the private root `.env`. v4 model calls go only to the Kanata gateway (`KANATA_BASE_URL`, https) through `legacy/python/bot/infrastructure/llm/`; Compose mounts the bearer key as the `kanata_api_key` secret from `KANATA_API_KEY_HOST_FILE`, never as an env var. Do not reintroduce the `ollama` package or native Ollama endpoints in `bot/`.

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
<!-- recall:lessons:end -->
