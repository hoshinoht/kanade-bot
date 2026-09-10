# Infrastructure guide

- `config.py` is the Pydantic settings boundary; keep deployment parsing here rather than in domain or presentation code.
- `db.py` is the sole SQLite schema/repository owner. A schema change must update database creation, supported upgrades, `SCHEMA_VERSION`, and `tests/test_migration.py` together. Databases older than v9 remain unsupported.
- The repository connection is event-loop-owned and uses WAL. Use `Repo.backup_to()`/SQLite online backup rather than copying a live database file.
- `watch.py` and `backfill.py` stay Discord-type-free; `modellock.py` coordinates extractor/chat model access; `events.py` emits non-blocking portal updates.
- Audit recording must not abort the underlying mutation. Identity artwork caching is cosmetic and failure-tolerant.
- Focused check: `uv run pytest -q tests/test_config.py tests/test_migration.py tests/test_watch.py tests/test_backfill.py tests/test_backup.py tests/test_events.py tests/test_audit.py`.
