# Kanade bot repository guide

## Toolchain and checks

- Use Python 3.12 only (`pyproject.toml` rejects 3.13) and manage the environment with `uv`.
- Set up with `uv sync`; CI uses `uv sync --locked`, so run `uv lock` only when intentionally changing dependencies.
- A focused test is `uv run pytest -q tests/test_<area>.py::test_<case>`.
- `uv run pytest` excludes the `ollama` marker through pytest config and needs neither Discord nor a model. `uv run pytest -m ollama -v` hits the real local model (about 8 GB, slow); extractor fixtures skip if Ollama is unavailable, and the chatbot smoke test can be narrowed with `-k chat_live`.
- Match CI with `uv run ruff check .`, `uv run ruff format --check .`, `uv run python -m bot.portal_styles --output /tmp/portal.css`, and `uv run pytest -q -m "not ollama"`. CI also runs `docker build .` independently.
- Optional local hooks are enabled with `git config core.hooksPath .githooks`; pre-commit may format and re-stage Python files, while pre-push runs the non-Ollama suite.
- If the repository moves and `.venv` commands report a bad interpreter, repair their absolute shebangs with `uv sync --reinstall`.

## Where behavior lives

- `python -m bot` (`bot/__main__.py`) owns Discord, SQLite, and the FastAPI server on one event loop; the portal is not a separate application.
- Keep scheduling rules in `bot/domain/`, persistence and deployment integration in `bot/infrastructure/`, Discord orchestration in `bot/agent/`, extraction in `bot/extract/`, chatbot behavior/tools in `bot/chat/`, and HTTP presentation in `bot/api/`.
- `bossctl` is an HTTP client for the same API; do not add a second scheduling path or make it manipulate the live SQLite file directly.
- `boss/` holds the canonical boss catalog and checked-in strategy knowledge; `config/` holds deployment configuration examples and private live configuration.
- `tests/` mirrors behavior by feature rather than package and supplies Discord/model fakes. `docs/` contains setup, development, portal, extractor, chatbot, and command guides.
- `scripts/bench_extract.py` benchmarks extraction fixtures; Docker/Compose and `caddy/` own deployment packaging and reverse proxy configuration.

## Generated, coupled, and private files

- Never edit generated, git-ignored `bot/api/static/portal.css`; validate its source with `python -m bot.portal_styles`.
- Boss portraits and entry artwork are intentionally git-ignored deployment assets. Their tests isolate themselves from whatever images happen to exist locally.
- Treat `.env`, `.env.caddy`, `caddy/Caddyfile`, `data/`, `config/guide.yaml`, and live files under `config/personas/` as deployment-private. Change tracked example/template files unless the task explicitly targets local deployment state.
- Full Compose startup also expects the externally managed volumes `kanade_botdata`, `kanade_caddydata`, and `kanade_caddyconfig`, plus the private Caddy/env files. Ollama stays on the host and Compose reaches it through `host.docker.internal`.

## Coding policy

- Keep comments and docstrings concise.
- Read the nearest nested `AGENTS.md` before changing a subsystem; local files contain only subsystem-specific guidance.

## CHANGELOG
- when new features are added, changed, or bugfixes are made, add a changelog entry with a brief description.


<!-- recall:lessons:begin -->
- Keep chatbot schedule language distinct from storage boundaries: unqualified `this`/`next week` is calendar Monday–Sunday, explicit `boss week` uses the configured reset interval, and bare weekdays resolve forward; calendar/date reads may therefore merge multiple boss-week buckets while API/CLI/write paths retain boss-week semantics.
- Keep seeded chatbot tests on one fixture-scoped aware clock: pass the same instant into week materialization and patch each imported clock seam used by tools, commits, API service, and test helpers so reset-day rollovers cannot change the suite.
- Keep durable chatbot memory individually enrolled, notification-first, typed, and presentation-only: bossing-role membership never enrolls users, opt-out/deletion remain available while disabled, and checked-in boss YAML and schedule state always outrank memory.
- Schedule retention by monotonic elapsed time while passing an aware wall-clock instant into persistence, so system clock rollback cannot suppress expiry and purge work.
- Keep operational detail pages in a fixed `100dvh` shell with one tabbed window filling the remaining height: the document/body, masthead, back navigation, human identity, and tab strip never scroll; only the selected panel scrolls, including on narrow screens. Never fall back to stacked card windows, whole-window movement, or document-body scrolling.
<!-- recall:lessons:end -->
