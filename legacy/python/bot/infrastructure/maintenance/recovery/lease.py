"""Persisted, task-bound authority for closed-state runtime attempt resolution."""

from __future__ import annotations

import asyncio
import hashlib
import secrets
import sqlite3
import uuid
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import TYPE_CHECKING

from bot.domain.timeutil import to_iso

from ..coordinator import MaintenanceClosedError, MaintenanceStateError
from ..state import MaintenanceMode, read_state

if TYPE_CHECKING:
    from ..guard import WriteGuard, _Authority

RECOVERY_KIND = "runtime_recovery"
RECOVERY_MODES = frozenset({MaintenanceMode.BLOCKED, MaintenanceMode.FROZEN})


@dataclass(frozen=True, slots=True)
class RecoveryLease:
    operation_id: str
    token_hash: str
    generation: int
    owner: asyncio.Task[object]
    authority: _Authority


class RecoveryLeases:
    """At most one registered recovery lease; each lives for one transaction."""

    def __init__(self, conn: sqlite3.Connection, guard: WriteGuard, instance_id: str):
        self._conn = conn
        self._guard = guard
        self._instance_id = instance_id
        self._leases: dict[str, RecoveryLease] = {}

    def __len__(self) -> int:
        return len(self._leases)

    def acquire(self, task: asyncio.Task[object]) -> RecoveryLease:
        if asyncio.current_task() is not task or task.done():
            raise MaintenanceStateError("runtime recovery lease must belong to its current task")
        if self._leases:
            raise MaintenanceStateError("a runtime recovery lease is already active")
        token_hash = hashlib.sha256(secrets.token_urlsafe(32).encode()).hexdigest()
        operation_id = str(uuid.uuid4())
        with self._guard._runtime_recovery_lease_insert_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                state = read_state(self._conn)
                if state.mode not in RECOVERY_MODES:
                    raise MaintenanceClosedError("runtime recovery requires BLOCKED or FROZEN")
                self._assert_no_other_live_lease(None)
                self._conn.execute(
                    "INSERT INTO maintenance_leases "
                    "(operation_id, instance_id, owner_token_hash, generation, operation_kind, "
                    "started_at, owner_task_id, lifecycle, claim_state) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, 'live', 'active')",
                    (
                        operation_id,
                        self._instance_id,
                        token_hash,
                        state.generation,
                        RECOVERY_KIND,
                        to_iso(datetime.now(UTC)),
                        id(task),
                    ),
                )
            except BaseException:
                if self._conn.in_transaction:
                    self._conn.execute("ROLLBACK")
                raise
            else:
                self._commit()
        try:
            authority = self._guard._register_runtime_recovery(
                operation_id,
                token_hash,
                task,
                instance_id=self._instance_id,
                generation=state.generation,
            )
        except BaseException:
            self._delete_row(operation_id, token_hash, state.generation, id(task))
            raise
        lease = RecoveryLease(operation_id, token_hash, state.generation, task, authority)
        self._leases[operation_id] = lease
        return lease

    def assert_live(self, lease: RecoveryLease) -> None:
        """Recheck task, registry, persisted row and mode inside the write transaction."""
        task = asyncio.current_task()
        if (
            task is None
            or task is not lease.owner
            or task.done()
            or self._leases.get(lease.operation_id) is not lease
            or not self._guard._runtime_recovery_is_live(lease.authority)
        ):
            raise MaintenanceStateError("runtime recovery lease is not registered to this task")
        state = read_state(self._conn)
        if state.mode not in RECOVERY_MODES or state.generation != lease.generation:
            raise MaintenanceClosedError("runtime recovery lease is no longer valid")
        row = self._conn.execute(
            "SELECT instance_id, owner_token_hash, generation, operation_kind, owner_task_id, "
            "lifecycle, claim_state FROM maintenance_leases WHERE operation_id = ?",
            (lease.operation_id,),
        ).fetchone()
        if (
            row is None
            or row["instance_id"] != self._instance_id
            or row["owner_token_hash"] != lease.token_hash
            or row["generation"] != lease.generation
            or row["operation_kind"] != RECOVERY_KIND
            or row["owner_task_id"] != id(task)
            or row["lifecycle"] != "live"
            or row["claim_state"] != "active"
        ):
            raise MaintenanceStateError("persisted runtime recovery lease is not live")
        self._assert_no_other_live_lease(lease.operation_id)

    def release(self, lease: RecoveryLease) -> None:
        if (
            asyncio.current_task() is not lease.owner
            or self._leases.get(lease.operation_id) is not lease
        ):
            raise MaintenanceStateError("runtime recovery cleanup belongs to its owner task")
        try:
            if self._conn.in_transaction:
                raise MaintenanceStateError("runtime recovery cleanup found an open transaction")
            changed = self._delete_row(
                lease.operation_id, lease.token_hash, lease.generation, id(lease.owner)
            )
            if changed != 1:
                raise MaintenanceStateError("persisted runtime recovery lease disappeared")
        finally:
            self._guard._revoke_runtime_recovery(lease.authority)
            self._leases.pop(lease.operation_id, None)

    def _assert_no_other_live_lease(self, own_operation_id: str | None) -> None:
        if self._conn.execute(
            "SELECT 1 FROM maintenance_leases WHERE lifecycle = 'live' "
            "AND operation_id IS NOT ? LIMIT 1",
            (own_operation_id,),
        ).fetchone():
            raise MaintenanceClosedError("runtime recovery refuses while another lease is live")

    def _delete_row(self, operation_id: str, token_hash: str, generation: int, task_id: int) -> int:
        with self._guard._runtime_recovery_lease_delete_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                changed = self._conn.execute(
                    "DELETE FROM maintenance_leases WHERE operation_id = ? AND instance_id = ? "
                    "AND owner_token_hash = ? AND generation = ? AND owner_task_id = ? "
                    "AND operation_kind = ? AND lifecycle = 'live' AND claim_state = 'active'",
                    (
                        operation_id,
                        self._instance_id,
                        token_hash,
                        generation,
                        task_id,
                        RECOVERY_KIND,
                    ),
                ).rowcount
            except BaseException:
                if self._conn.in_transaction:
                    self._conn.execute("ROLLBACK")
                raise
            else:
                self._commit()
        return changed

    def _commit(self) -> None:
        try:
            self._conn.execute("COMMIT")
        except BaseException:
            if self._conn.in_transaction:
                self._conn.execute("ROLLBACK")
            raise
