# Extraction guide

## Pipeline

- Preserve the package order documented in `__init__.py`: deterministic gate/prompt/schema handling surrounds the model call, then Python resolves, merges, matches, and plans changes.
- `gate.py` performs cheap pure screening; `llm.py` is the guarded Ollama boundary under the shared model lock. Model output is untrusted and non-authoritative.
- Keep literal date/time references unresolved in `schema.py`; resolve them from evidence timestamps in `resolve.py`. `merge.py` owns latest-explicit-value semantics.
- `pipeline.py` coordinates stages; `commit.py` applies confirmed cards through repository and agent helpers. Do not let extraction bypass card review for schedule writes.
- `python -m bot.extract` is an offline JSONL runner and writes SQLite only with `--record`.

## Checks

- Deterministic suite: `uv run pytest -q tests/test_extract_gate.py tests/test_extract_schema.py tests/test_extract_resolve.py tests/test_extract_merge.py tests/test_extract_match.py tests/test_extract_pipeline.py tests/test_extract_commit.py`.
- Real-model fixtures are opt-in: `uv run pytest -m ollama -v tests/test_extract_fixtures.py`.
