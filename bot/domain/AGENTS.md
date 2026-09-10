# Domain guide

- Keep this package free of Discord, SQLite, HTTP, and model I/O. Callers live in infrastructure, agent, extraction, chat, and API layers.
- `weeks.py` owns boss-week and calendar-week arithmetic plus fixed-slot placement; use aware datetimes and the helpers in `timeutil.py`.
- `bosses.py` validates and resolves the catalog in `boss/bosses.yaml`; `boss_knowledge.py` validates complete per-boss strategy knowledge.
- `ids.py` owns UUID creation, display short IDs, and unique-prefix resolution.
- Preserve the distinction between calendar weeks and reset-based boss weeks when adding date language or storage boundaries.
- Focused check: `uv run pytest -q tests/test_weeks.py tests/test_bosses.py tests/test_ids.py tests/test_boss_knowledge.py`.
