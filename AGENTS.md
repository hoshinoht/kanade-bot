# Repository guide

## Toolchain and checks

- Use Python 3.12 only (`pyproject.toml` rejects 3.13) and manage the environment with `uv`.
- Set up with `uv sync`; CI uses `uv sync --locked`, so run `uv lock` only when intentionally changing dependencies.
- A focused test is `uv run pytest -q tests/test_<area>.py::test_<case>`.
- `uv run pytest` excludes the `ollama` marker through pytest config and needs neither Discord nor a model. `uv run pytest -m ollama -v` hits the real local model (about 13 GB, slow); extractor fixtures skip if Ollama is unavailable, and the chatbot smoke test can be narrowed with `-k chat_live`.
- Match CI with `uv run ruff check .`, `uv run ruff format --check .`, `uv run python -m bot.portal_styles --output /tmp/portal.css`, and `uv run pytest -q -m "not ollama"`. CI also runs `docker build .` independently.
- Optional local hooks are enabled with `git config core.hooksPath .githooks`; pre-commit may format and re-stage Python files, while pre-push runs the non-Ollama suite.
- If the repository moves and `.venv` commands report a bad interpreter, repair their absolute shebangs with `uv sync --reinstall`.

## Where behavior lives

- `python -m bot` (`bot/__main__.py`) owns Discord, SQLite, and the FastAPI server on one event loop; the portal is not a separate application.
- Keep scheduling rules in `bot/domain/`, persistence and deployment integration in `bot/infrastructure/`, Discord orchestration in `bot/agent/`, extraction in `bot/extract/`, chatbot behavior/tools in `bot/chat/`, and HTTP presentation in `bot/api/`.
- `bossctl` is an HTTP client for the same API; do not add a second scheduling path or make it manipulate the live SQLite file directly.
- Tests use in-memory SQLite plus `tests/fake_bot.py`/`tests/chat_support.py`; add Discord-independent coverage at the layer being changed.
- SQLite schema and migrations are centralized in `bot/infrastructure/db.py`. Existing databases older than v9 are deliberately rejected; schema changes need creation, supported upgrade paths, `SCHEMA_VERSION`, and `tests/test_migration.py` kept together.

## Generated, coupled, and private files

- Edit `bot/api/static/portal.scss` and its ordered CSS-compatible partials, never the generated, git-ignored `bot/api/static/portal.css`. `@use` controls concatenation order; this project does not run a Sass compiler.
- `boss/bosses.yaml` is the canonical boss catalog. With chatbot channels configured, `boss/knowledge/` must contain `_meta.yaml` plus exactly one lowercase YAML file per catalog boss, with no extras; catalog/knowledge changes require a restart, while image changes require only a page reload.
- Boss portraits and entry artwork are intentionally git-ignored deployment assets. Their tests isolate themselves from whatever images happen to exist locally.
- Treat `.env`, `.env.caddy`, `caddy/Caddyfile`, `data/`, `config/guide.yaml`, and live files under `config/personas/` as deployment-private. Change tracked example/template files unless the task explicitly targets local deployment state.
- Full Compose startup also expects the externally managed volumes `kanade_botdata`, `kanade_caddydata`, and `kanade_caddyconfig`, plus the private Caddy/env files. Ollama stays on the host and Compose reaches it through `host.docker.internal`.
