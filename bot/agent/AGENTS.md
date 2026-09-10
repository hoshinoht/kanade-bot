# Discord agent guide

- `client.py` is the concrete Discord boundary: lifecycle, event routing, posting/editing cards, reactions, reminders, and worker startup.
- `commands.py` owns slash commands, autocomplete, role checks, and command-triggered mutations; register commands through its existing registration path.
- Keep deterministic logic in helpers: `formatting.py` builds messages without Discord objects, `rsvp.py` handles statuses, `pings.py` centralizes mention policy, and `util.py` resolves authorization/participants.
- Use `pings.audience()` for mention targets rather than passing participant lists directly.
- `materialise.py` persists fixed timings as runs/reminders; do not introduce parallel in-memory scheduler state. `rescan.py` owns the persisted sequential rescan queue.
- Message handling offers the chatbot before extraction; reactions may commit extractor/chat proposal cards. Preserve that ordering and the shared approval path.
- Focused check: `uv run pytest -q tests/test_pings.py tests/test_rsvp.py tests/test_materialise.py tests/test_rescan_worker.py tests/test_say.py tests/test_debug.py`.
