# Bot package guide

## Runtime and entry points

- `__main__.py` loads settings/catalogs, opens the repository, constructs `agent.client.BossBot`, and owns reconnect/backoff for `python -m bot`.
- `agent/client.py` wires Discord, extraction, chatbot, rescans, and the in-process API onto one asyncio event loop. Keep the SQLite connection on that loop.
- `cli.py` provides the `bossctl` HTTP client. `export.py`, `health.py`, `portal_styles.py`, and `extract/__main__.py` are separate operational entry points.

## Package boundaries

- `domain/` contains value parsing and pure scheduling rules; `infrastructure/` owns configuration, persistence, and deployment integration.
- `agent/` translates Discord events and commands. `extract/` turns watched messages into reviewable schedule cards; `chat/` handles conversational replies and tool calls.
- `api/` exposes shared operations through JSON routes and a server-rendered portal. Reuse API services rather than creating alternate persistence paths.
- Keep modules that do not need Discord objects independently testable; live gateway/model access is not required by the default test suite.

## Checks

- Run the focused test file for the package being changed, then `uv run ruff check bot tests` and `uv run ruff format --check bot tests`.
- Use `uv run python -m bot.portal_styles --output /tmp/portal.css` when portal styles or their generator change.
