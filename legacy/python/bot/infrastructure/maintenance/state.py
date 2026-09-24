"""Typed maintenance-state rows and fail-closed bootstrap checks."""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass
from enum import StrEnum


class MaintenanceMode(StrEnum):
    OPEN = "OPEN"
    PREPARING = "PREPARING"
    BLOCKED = "BLOCKED"
    FROZEN = "FROZEN"
    RESUMING = "RESUMING"


@dataclass(frozen=True)
class MaintenanceState:
    mode: MaintenanceMode
    generation: int
    state_revision: int
    adoption_state: str
    adoption_id: str | None
    blocker_code: str | None = None


def read_state(conn: sqlite3.Connection) -> MaintenanceState:
    rows = conn.execute(
        "SELECT mode, generation, state_revision, adoption_state, adoption_id, blocker_code "
        "FROM maintenance_state WHERE id = 1"
    ).fetchall()
    if len(rows) != 1:
        raise RuntimeError("maintenance state is corrupt")
    row = rows[0]
    try:
        mode = MaintenanceMode(row[0])
    except ValueError as exc:
        raise RuntimeError("maintenance state is corrupt") from exc
    if row[1] < 0 or row[2] < 0 or row[3] not in {"pending", "complete"}:
        raise RuntimeError("maintenance state is corrupt")
    if row[3] == "pending" and not row[4]:
        raise RuntimeError("maintenance state is corrupt")
    return MaintenanceState(mode, row[1], row[2], row[3], row[4], row[5])
