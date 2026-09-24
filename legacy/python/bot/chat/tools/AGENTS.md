# Chat tool guide

- `schemas.py` is the closed, ordered model-visible tool surface; update schemas and dispatch registration together.
- `contracts.py` defines `ToolContext`. Treat its author, channel, message provenance, and read-only flag as trusted runtime state, never model-supplied arguments.
- `dispatching.py` owns read/write registries and is the guarded non-raising execution boundary. Enforce read-only mode there for rejection follow-ups.
- Centralize existing-run authorization in `authority.py`; do not duplicate weaker ownership checks inside individual tools.
- Read tools return grounded schedule/catalog data. Write tools in `proposals.py` must create reviewable cards through shared `apply_plan` behavior rather than mutating schedules directly.
- Cover changes in `tests/test_chat_tools.py`, plus `tests/test_chat_authority.py` or `tests/test_chat_followup.py` when changing permissions/read-only behavior.
