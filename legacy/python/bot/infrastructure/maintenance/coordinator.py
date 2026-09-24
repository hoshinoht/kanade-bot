"""Task-owned durable lease admission and conservative maintenance transitions."""

from __future__ import annotations

import asyncio
import contextlib
import contextvars
import hashlib
import secrets
import sqlite3
import uuid
from collections.abc import AsyncIterator, Callable
from dataclasses import dataclass
from datetime import UTC, datetime, time
from typing import TYPE_CHECKING, TypeVar
from zoneinfo import ZoneInfo

from bot.domain.timeutil import to_iso

from .adoption.evidence import MessageObservation
from .adoption.model import AdoptionSeedReport, SourceFamily, SourceKey
from .guard import WriteGuard, _Authority, _current_task
from .state import MaintenanceMode, read_state

if TYPE_CHECKING:
    from .recovery.lease import RecoveryLease
    from .recovery.model import BlockerReport

_ResolutionResult = TypeVar("_ResolutionResult")


class MaintenanceError(RuntimeError):
    """Base class for typed maintenance admission and state failures."""


class MaintenanceClosedError(MaintenanceError):
    """Ordinary work was not admitted because maintenance is closed."""


class MaintenanceStateError(MaintenanceClosedError):
    """A maintenance transition or reservation state is invalid."""


class MaintenanceTransactionError(MaintenanceError):
    """A caller supplied an unsupported transaction on the repository connection."""


@dataclass(frozen=True, slots=True)
class MaintenanceLease:
    operation_id: str
    token: str
    token_hash: str
    generation: int
    operation_kind: str
    owner: asyncio.Task[object]
    claim_state: str = "active"


@dataclass(frozen=True, slots=True)
class MaintenanceReservation:
    """A durable child reservation with no SQL write authority of its own."""

    operation_id: str
    token: str
    token_hash: str
    generation: int
    operation_kind: str
    owner: asyncio.Task[object]

    @property
    def claim_state(self) -> str:
        return "reserved"


@dataclass(frozen=True, slots=True)
class _LiveLease:
    lease: MaintenanceLease
    authority: _Authority


@dataclass(frozen=True, slots=True)
class _AdoptionSeedLease:
    operation_id: str
    token_hash: str
    generation: int
    adoption_id: str
    owner: asyncio.Task[object]


@dataclass(frozen=True, slots=True)
class _AdoptionResolutionLease:
    operation_id: str
    token_hash: str
    generation: int
    adoption_id: str
    owner: asyncio.Task[object]
    authority: _Authority


class MaintenanceCoordinator:
    """Own maintenance metadata and task-bound application capabilities."""

    def __init__(self, conn: sqlite3.Connection, guard: WriteGuard, instance_id: str | None = None):
        self._conn = conn
        self._guard = guard
        self.instance_id = instance_id or str(uuid.uuid4())
        self._current: contextvars.ContextVar[MaintenanceLease | None] = contextvars.ContextVar(
            "maintenance_lease", default=None
        )
        self._active: dict[str, _LiveLease] = {}
        self._active_by_task: dict[asyncio.Task[object], _LiveLease] = {}
        self._reservations: dict[int, MaintenanceReservation] = {}
        self._adoption_seed_leases: dict[str, _AdoptionSeedLease] = {}
        self._adoption_resolution_leases: dict[str, _AdoptionResolutionLease] = {}
        from .recovery.lease import RecoveryLeases

        self._runtime_recovery_leases = RecoveryLeases(conn, guard, self.instance_id)
        self._prepare_task: asyncio.Task[object] | None = None

    @contextlib.asynccontextmanager
    async def operation(self, kind: str) -> AsyncIterator[MaintenanceLease]:
        """Admit one operation, or reuse only the live lease of this task."""
        task = _current_task()
        inherited = self._current.get()
        if task is None:
            raise MaintenanceClosedError("ordinary operation requires an asyncio task")
        active = self._active_by_task.get(task)
        if active is not None:
            if inherited is active.lease:
                yield active.lease
                return
            raise MaintenanceClosedError("task already owns a maintenance operation")

        lease = self._insert_lease(kind, task, claim_state="active")
        try:
            authority = self._guard._register_application(
                lease.operation_id, lease.token_hash, lease.owner
            )
        except Exception:
            self._delete_live_row(lease)
            raise
        live = _LiveLease(lease, authority)
        self._active[lease.operation_id] = live
        self._active_by_task[task] = live
        marker = self._current.set(lease)
        try:
            with self._guard._application_scope(authority):
                yield lease
        finally:
            self._current.reset(marker)
            self._release(live)

    async def seed_adoption_sources(
        self,
        timezone: ZoneInfo,
        reset_weekday: int,
        reset_time: time,
        pinned_at: datetime,
    ) -> AdoptionSeedReport:
        """Seed the first source snapshot under a task-bound BLOCKED-only capability."""
        from .adoption.classification import (
            classify_source_snapshot,
            persist_source_snapshot,
            validate_seed_inputs,
        )

        _, instant, boss_week_start = validate_seed_inputs(
            timezone, reset_weekday, reset_time, pinned_at
        )
        task = _current_task()
        if task is None or task.done():
            raise MaintenanceStateError("adoption seeding requires an asyncio task")
        self._assert_no_pending_transaction()
        state = read_state(self._conn)
        if (
            state.mode is not MaintenanceMode.BLOCKED
            or state.adoption_state != "pending"
            or state.adoption_id is None
        ):
            raise MaintenanceClosedError("adoption seeding requires BLOCKED pending adoption")
        if self._adoption_seed_leases or self._adoption_resolution_leases:
            raise MaintenanceStateError("an adoption administrative lease is already active")

        lease = self._insert_adoption_seed_lease(task)
        self._adoption_seed_leases[lease.operation_id] = lease
        try:
            self._assert_adoption_seed_lease(lease)
            captured_at = to_iso(instant)
            with self._guard._adoption_seed_sources_scope():
                self._conn.execute("BEGIN IMMEDIATE")
                try:
                    self._assert_adoption_seed_lease(lease)
                    current = read_state(self._conn)
                    if current.adoption_id != lease.adoption_id:
                        raise MaintenanceStateError("pending adoption identity changed")
                    records = classify_source_snapshot(
                        self._conn,
                        current.adoption_id,
                        instant,
                        captured_at,
                        boss_week_start,
                    )
                    report = persist_source_snapshot(
                        self._conn,
                        current.adoption_id,
                        records,
                        captured_at,
                        boss_week_start,
                    )
                except BaseException:
                    if self._conn.in_transaction:
                        self._conn.execute("ROLLBACK")
                    raise
                else:
                    try:
                        self._conn.execute("COMMIT")
                    except BaseException:
                        if self._conn.in_transaction:
                            self._conn.execute("ROLLBACK")
                        raise
            return report
        finally:
            try:
                self._delete_adoption_seed_lease(lease)
            finally:
                self._adoption_seed_leases.pop(lease.operation_id, None)

    async def bind_adoption_group(
        self,
        family: SourceFamily | str,
        channel_id: str | int,
        message_id: str | int,
        observations: tuple[MessageObservation, ...],
        *,
        guild_id: str | int,
        bot_author_id: str | int,
        actor: str,
        reason: str,
        at: datetime,
    ) -> str:
        """Bind one synthetic whole-message group without performing a Discord fetch."""
        from .adoption.resolution import (
            bind_verified_group,
            prepare_message_evidence,
            validate_resolution_metadata,
        )

        task = _current_task()
        if task is None or task.done():
            raise MaintenanceStateError("adoption resolution requires an asyncio task")
        evidence = prepare_message_evidence(
            family, channel_id, message_id, guild_id, bot_author_id, observations
        )
        actor, reason, resolved_at = validate_resolution_metadata(actor, reason, at)
        lease = self._insert_adoption_resolution_lease(task)
        try:
            return self._adoption_resolution_transaction(
                lease,
                lambda: bind_verified_group(
                    self._conn,
                    self._guard,
                    lease.authority,
                    adoption_id=lease.adoption_id,
                    instance_id=self.instance_id,
                    evidence=evidence,
                    actor=actor,
                    reason=reason,
                    resolved_at=resolved_at,
                ),
            )
        finally:
            self._delete_adoption_resolution_lease(lease)

    async def retire_adoption_source(
        self, key: SourceKey, *, actor: str, reason: str, at: datetime
    ) -> None:
        """Retire exactly one pending source without fabricating a delivery attempt."""
        from .adoption.resolution import retire_pending_source, validate_resolution_metadata

        task = _current_task()
        if task is None or task.done():
            raise MaintenanceStateError("adoption resolution requires an asyncio task")
        if not isinstance(key, SourceKey):
            raise ValueError("adoption source key is required")
        actor, reason, resolved_at = validate_resolution_metadata(actor, reason, at)
        lease = self._insert_adoption_resolution_lease(task)
        try:
            self._adoption_resolution_transaction(
                lease,
                lambda: retire_pending_source(
                    self._conn,
                    self._guard,
                    lease.authority,
                    adoption_id=lease.adoption_id,
                    key=key,
                    actor=actor,
                    reason=reason,
                    resolved_at=resolved_at,
                ),
            )
        finally:
            self._delete_adoption_resolution_lease(lease)

    async def retire_adoption_attempt(
        self, attempt_id: str, *, actor: str, reason: str, at: datetime
    ) -> None:
        """Retire one exact adopted attempt and atomically release its full source group."""
        from .adoption.resolution import retire_adopted_attempt, validate_resolution_metadata

        task = _current_task()
        if task is None or task.done():
            raise MaintenanceStateError("adoption resolution requires an asyncio task")
        if not isinstance(attempt_id, str) or not attempt_id.strip():
            raise ValueError("adoption attempt id is required")
        actor, reason, resolved_at = validate_resolution_metadata(actor, reason, at)
        lease = self._insert_adoption_resolution_lease(task)
        try:
            self._adoption_resolution_transaction(
                lease,
                lambda: retire_adopted_attempt(
                    self._conn,
                    self._guard,
                    lease.authority,
                    adoption_id=lease.adoption_id,
                    attempt_id=attempt_id,
                    actor=actor,
                    reason=reason,
                    resolved_at=resolved_at,
                ),
            )
        finally:
            self._delete_adoption_resolution_lease(lease)

    def blocker_report(self) -> BlockerReport:
        """Read-only identities of every persistent blocker; never message content."""
        from .recovery.report import build_blocker_report

        self._assert_no_pending_transaction()
        return build_blocker_report(
            self._conn,
            live_leases=self._live_count(),
            persistent_blockers=self._has_persistent_blockers(),
        )

    def _runtime_recovery_transaction(
        self, action: Callable[[RecoveryLease], _ResolutionResult]
    ) -> _ResolutionResult:
        """Run one write under a fresh BLOCKED/FROZEN-only recovery lease (D8)."""
        task = _current_task()
        if task is None or task.done():
            raise MaintenanceStateError("runtime recovery requires an asyncio task")
        self._assert_no_pending_transaction()
        if self._live_count():
            raise MaintenanceClosedError("runtime recovery refuses while live work exists")
        leases = self._runtime_recovery_leases
        lease = leases.acquire(task)
        try:
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                leases.assert_live(lease)
                result = action(lease)
            except BaseException:
                if self._conn.in_transaction:
                    self._conn.execute("ROLLBACK")
                raise
            try:
                self._conn.execute("COMMIT")
            except BaseException:
                if self._conn.in_transaction:
                    self._conn.execute("ROLLBACK")
                raise
            return result
        finally:
            leases.release(lease)

    def reserve_child(self, kind: str) -> MaintenanceReservation:
        """Persist a reservation synchronously before the caller creates a task."""
        task = _current_task()
        if task is None:
            raise MaintenanceClosedError("child reservation requires an asyncio task")
        reservation = self._insert_lease(kind, task, claim_state="reserved")
        self._reservations[id(reservation)] = reservation
        return reservation

    def release_child(self, reservation: MaintenanceReservation) -> None:
        """Release an unclaimed reservation from its original owner explicitly."""
        task = _current_task()
        if not isinstance(reservation, MaintenanceReservation):
            raise MaintenanceStateError("child reservation is invalid")
        if self._reservations.get(id(reservation)) is not reservation:
            raise MaintenanceStateError("child reservation is not live")
        if task is not reservation.owner:
            raise MaintenanceStateError("child reservation belongs to another task")
        self._delete_reserved_row(reservation)
        self._reservations.pop(id(reservation), None)

    @contextlib.asynccontextmanager
    async def claim_child(
        self, reservation: MaintenanceReservation
    ) -> AsyncIterator[MaintenanceLease]:
        """Transfer one reservation exactly once to a different task."""
        task = _current_task()
        if task is None or not isinstance(reservation, MaintenanceReservation):
            raise MaintenanceStateError("child reservation is invalid")
        if self._reservations.get(id(reservation)) is not reservation:
            raise MaintenanceStateError("child reservation is not live")
        if task is reservation.owner:
            raise MaintenanceStateError("child lease requires a distinct child task")
        if task in self._active_by_task:
            raise MaintenanceStateError("task already owns a maintenance operation")

        claimed = self._claim_reserved_row(reservation, task)
        self._reservations.pop(id(reservation), None)
        try:
            authority = self._guard._register_application(
                claimed.operation_id, claimed.token_hash, claimed.owner
            )
        except Exception:
            self._delete_live_row(claimed)
            raise
        live = _LiveLease(claimed, authority)
        self._active[claimed.operation_id] = live
        self._active_by_task[task] = live
        marker = self._current.set(claimed)
        try:
            with self._guard._application_scope(authority):
                yield claimed
        finally:
            self._current.reset(marker)
            self._release(live)

    async def prepare(self, timeout: float | None = None) -> bool:
        """Close admission and drain accepted work without claiming an authority freeze."""
        task = _current_task()
        if task is None:
            raise MaintenanceStateError("prepare requires an asyncio task")
        if self._prepare_task is not None:
            raise MaintenanceStateError("maintenance prepare is already running")

        self._assert_no_pending_transaction()
        state = read_state(self._conn)
        if state.mode is MaintenanceMode.OPEN:
            expected = MaintenanceMode.OPEN
        elif state.mode is MaintenanceMode.BLOCKED:
            if self._has_persistent_blockers():
                return False
            if state.blocker_code in {"prepare_timeout", "prepare_cancelled"}:
                expected = MaintenanceMode.BLOCKED
            else:
                raise MaintenanceStateError(f"cannot prepare from {state.mode.value}")
        else:
            raise MaintenanceStateError(f"cannot prepare from {state.mode.value}")

        self._prepare_task = task
        try:
            self._transition(MaintenanceMode.PREPARING, expected=expected, blocker_code=None)
            deadline = None if timeout is None else asyncio.get_running_loop().time() + timeout
            while self._live_count():
                if deadline is not None and asyncio.get_running_loop().time() >= deadline:
                    self._transition(
                        MaintenanceMode.BLOCKED,
                        expected=MaintenanceMode.PREPARING,
                        blocker_code="prepare_timeout",
                    )
                    return False
                await asyncio.sleep(0)
            if self._has_persistent_blockers():
                self._transition(
                    MaintenanceMode.BLOCKED,
                    expected=MaintenanceMode.PREPARING,
                    blocker_code="persistent_blocker",
                )
                return False
            # A drained PREPARING state is quiescent, not frozen.  The later
            # reconciliation package owns checkpoint fields and FROZEN.
            return True
        except asyncio.CancelledError:
            self._transition(
                MaintenanceMode.BLOCKED,
                expected=MaintenanceMode.PREPARING,
                blocker_code="prepare_cancelled",
            )
            raise
        finally:
            self._prepare_task = None

    def retire_orphan(self, operation_id: str, actor: str, reason: str) -> None:
        """Retire an orphan while retaining its identity and evidence."""
        actor = _bounded_text(actor, "orphan actor", 128)
        reason = _bounded_text(reason, "orphan retirement reason", 512)
        if not isinstance(operation_id, str) or not operation_id.strip():
            raise ValueError("orphan operation id is required")
        if _current_task() is None:
            raise MaintenanceStateError("orphan retirement requires an asyncio task")
        self._assert_no_pending_transaction()
        with self._guard._retire_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                row = self._conn.execute(
                    "SELECT lifecycle FROM maintenance_leases WHERE operation_id = ?",
                    (operation_id,),
                ).fetchone()
                if row is None or row[0] != "orphaned":
                    raise MaintenanceStateError("maintenance operation is not an orphan")
                changed = self._conn.execute(
                    "UPDATE maintenance_leases SET lifecycle = 'retired', retired_at = ?, "
                    "retired_by = ?, retirement_reason = ? "
                    "WHERE operation_id = ? AND lifecycle = 'orphaned'",
                    (to_iso(datetime.now(UTC)), actor, reason, operation_id),
                ).rowcount
                if changed != 1:
                    raise MaintenanceStateError("orphan retirement lost its row")
            except Exception:
                self._conn.execute("ROLLBACK")
                raise
            else:
                self._conn.execute("COMMIT")

    def _insert_lease(
        self, kind: str, task: asyncio.Task[object], *, claim_state: str
    ) -> MaintenanceLease | MaintenanceReservation:
        kind = _bounded_text(kind, "operation kind", 128)
        if claim_state not in {"active", "reserved"}:
            raise MaintenanceStateError("invalid lease claim state")
        self._assert_no_pending_transaction()
        state = read_state(self._conn)
        if state.mode is not MaintenanceMode.OPEN or state.adoption_state != "complete":
            raise MaintenanceClosedError("maintenance is closed")
        token = secrets.token_urlsafe(32)
        token_hash = hashlib.sha256(token.encode()).hexdigest()
        operation_id = str(uuid.uuid4())
        with self._guard._admission_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                current = read_state(self._conn)
                if current.mode is not MaintenanceMode.OPEN or current.adoption_state != "complete":
                    raise MaintenanceClosedError("maintenance is closed")
                self._conn.execute(
                    "INSERT INTO maintenance_leases "
                    "(operation_id, instance_id, owner_token_hash, generation, operation_kind, "
                    "started_at, owner_task_id, lifecycle, claim_state) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, 'live', ?)",
                    (
                        operation_id,
                        self.instance_id,
                        token_hash,
                        current.generation,
                        kind,
                        to_iso(datetime.now(UTC)),
                        id(task),
                        claim_state,
                    ),
                )
            except Exception:
                self._conn.execute("ROLLBACK")
                raise
            else:
                self._conn.execute("COMMIT")
        if claim_state == "reserved":
            return MaintenanceReservation(
                operation_id, token, token_hash, current.generation, kind, task
            )
        return MaintenanceLease(operation_id, token, token_hash, current.generation, kind, task)

    def _claim_reserved_row(
        self, reservation: MaintenanceReservation, task: asyncio.Task[object]
    ) -> MaintenanceLease:
        self._assert_no_pending_transaction()
        with self._guard._claim_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                changed = self._conn.execute(
                    "UPDATE maintenance_leases SET owner_task_id = ?, claim_state = 'claimed' "
                    "WHERE operation_id = ? AND instance_id = ? AND owner_token_hash = ? "
                    "AND owner_task_id = ? AND lifecycle = 'live' AND claim_state = 'reserved'",
                    (
                        id(task),
                        reservation.operation_id,
                        self.instance_id,
                        reservation.token_hash,
                        id(reservation.owner),
                    ),
                ).rowcount
                if changed != 1:
                    raise MaintenanceStateError("child reservation is not claimable")
                row = self._conn.execute(
                    "SELECT generation, operation_kind FROM maintenance_leases "
                    "WHERE operation_id = ?",
                    (reservation.operation_id,),
                ).fetchone()
                if row is None:
                    raise MaintenanceStateError("child reservation disappeared")
            except Exception:
                self._conn.execute("ROLLBACK")
                raise
            else:
                self._conn.execute("COMMIT")
        return MaintenanceLease(
            reservation.operation_id,
            reservation.token,
            reservation.token_hash,
            int(row[0]),
            row[1],
            task,
            "claimed",
        )

    def _delete_reserved_row(self, reservation: MaintenanceReservation) -> None:
        self._assert_no_pending_transaction()
        with self._guard._release_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                changed = self._conn.execute(
                    "DELETE FROM maintenance_leases WHERE operation_id = ? AND instance_id = ? "
                    "AND owner_token_hash = ? AND owner_task_id = ? "
                    "AND lifecycle = 'live' AND claim_state = 'reserved'",
                    (
                        reservation.operation_id,
                        self.instance_id,
                        reservation.token_hash,
                        id(reservation.owner),
                    ),
                ).rowcount
                if changed != 1:
                    raise MaintenanceStateError("child reservation is not releasable")
            except Exception:
                self._conn.execute("ROLLBACK")
                raise
            else:
                self._conn.execute("COMMIT")

    def _delete_live_row(self, lease: MaintenanceLease) -> None:
        self._assert_no_pending_transaction()
        with self._guard._release_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                changed = self._conn.execute(
                    "DELETE FROM maintenance_leases WHERE operation_id = ? AND instance_id = ? "
                    "AND owner_token_hash = ? AND lifecycle = 'live' "
                    "AND claim_state IN ('active', 'claimed')",
                    (lease.operation_id, self.instance_id, lease.token_hash),
                ).rowcount
                if changed != 1:
                    raise MaintenanceStateError("maintenance lease is no longer persisted")
            except Exception:
                self._conn.execute("ROLLBACK")
                raise
            else:
                self._conn.execute("COMMIT")

    def _release(self, live: _LiveLease) -> None:
        lease = live.lease
        current = _current_task()
        if current is not lease.owner or self._active.get(lease.operation_id) is not live:
            raise MaintenanceStateError("maintenance lease release belongs to another task")
        self._delete_live_row(lease)
        self._guard._revoke_application(live.authority)
        self._active.pop(lease.operation_id, None)
        if self._active_by_task.get(current) is live:
            self._active_by_task.pop(current, None)

    def _live_count(self) -> int:
        return (
            len(self._active)
            + len(self._reservations)
            + len(self._adoption_seed_leases)
            + len(self._adoption_resolution_leases)
            + len(self._runtime_recovery_leases)
        )

    def has_live_work(self) -> bool:
        """Return actual runtime work, excluding preserved restart orphans."""
        return self._live_count() != 0

    def _assert_current_live_lease(self) -> MaintenanceLease:
        """Return authority only for this task's registered, persisted live lease."""
        task = _current_task()
        live = self._active_by_task.get(task) if task is not None else None
        if (
            task is None
            or live is None
            or live.lease.owner is not task
            or self._current.get() is not live.lease
            or self._active.get(live.lease.operation_id) is not live
            or task.done()
            or not self._guard._runtime_is_live(live.authority)
        ):
            raise MaintenanceClosedError("delivery requires this task's live maintenance lease")
        self._assert_no_pending_transaction()
        row = self._conn.execute(
            "SELECT instance_id, owner_token_hash, generation, owner_task_id, lifecycle, "
            "claim_state FROM maintenance_leases WHERE operation_id = ?",
            (live.lease.operation_id,),
        ).fetchone()
        if (
            row is None
            or row["instance_id"] != self.instance_id
            or row["owner_token_hash"] != live.lease.token_hash
            or row["generation"] != live.lease.generation
            or row["owner_task_id"] != id(task)
            or row["lifecycle"] != "live"
            or row["claim_state"] not in {"active", "claimed"}
        ):
            raise MaintenanceClosedError("delivery lease is no longer persisted as live")
        state = read_state(self._conn)
        if (
            state.mode
            not in {MaintenanceMode.OPEN, MaintenanceMode.PREPARING, MaintenanceMode.BLOCKED}
            or state.adoption_state != "complete"
        ):
            raise MaintenanceClosedError(
                "delivery lease is not valid in the current maintenance state"
            )
        return live.lease

    def _has_persistent_blockers(self) -> bool:
        state = read_state(self._conn)
        if state.adoption_state != "complete":
            return True
        if self._conn.execute(
            "SELECT 1 FROM maintenance_leases WHERE lifecycle = 'orphaned' LIMIT 1"
        ).fetchone():
            return True
        return (
            self._conn.execute(
                "SELECT 1 FROM delivery_attempts WHERE state IN ('intent', 'indeterminate') LIMIT 1"
            ).fetchone()
            is not None
        )

    def _transition(
        self,
        mode: MaintenanceMode,
        *,
        expected: MaintenanceMode,
        blocker_code: str | None,
    ) -> None:
        self._assert_no_pending_transaction()
        with self._guard._transition_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                changed = self._conn.execute(
                    "UPDATE maintenance_state SET mode = ?, state_revision = state_revision + 1, "
                    "blocker_code = ? WHERE id = 1 AND mode = ?",
                    (mode.value, blocker_code, expected.value),
                ).rowcount
                if changed != 1:
                    raise MaintenanceStateError(
                        f"maintenance state changed while transitioning from {expected.value}"
                    )
            except Exception:
                self._conn.execute("ROLLBACK")
                raise
            else:
                self._conn.execute("COMMIT")
        self._guard._set_mode(mode)

    def _assert_no_pending_transaction(self) -> None:
        if self._conn.in_transaction:
            raise MaintenanceTransactionError(
                "maintenance coordinator refuses an external pending transaction"
            )

    def _insert_adoption_seed_lease(self, task: asyncio.Task[object]) -> _AdoptionSeedLease:
        if _current_task() is not task or task.done():
            raise MaintenanceStateError("adoption seed lease must be created by its owner task")
        self._assert_no_pending_transaction()
        token_hash = hashlib.sha256(secrets.token_urlsafe(32).encode()).hexdigest()
        operation_id = str(uuid.uuid4())
        with self._guard._adoption_seed_lease_insert_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                state = read_state(self._conn)
                if (
                    state.mode is not MaintenanceMode.BLOCKED
                    or state.adoption_state != "pending"
                    or state.adoption_id is None
                ):
                    raise MaintenanceClosedError(
                        "adoption seeding requires BLOCKED pending adoption"
                    )
                self._conn.execute(
                    "INSERT INTO maintenance_leases "
                    "(operation_id, instance_id, owner_token_hash, generation, operation_kind, "
                    "started_at, owner_task_id, lifecycle, claim_state) "
                    "VALUES (?, ?, ?, ?, 'adoption_seed', ?, ?, 'live', 'active')",
                    (
                        operation_id,
                        self.instance_id,
                        token_hash,
                        state.generation,
                        to_iso(datetime.now(UTC)),
                        id(task),
                    ),
                )
            except BaseException:
                if self._conn.in_transaction:
                    self._conn.execute("ROLLBACK")
                raise
            else:
                try:
                    self._conn.execute("COMMIT")
                except BaseException:
                    if self._conn.in_transaction:
                        self._conn.execute("ROLLBACK")
                    raise
        return _AdoptionSeedLease(
            operation_id, token_hash, state.generation, state.adoption_id, task
        )

    def _assert_adoption_seed_lease(self, lease: _AdoptionSeedLease) -> None:
        task = _current_task()
        if (
            task is None
            or task is not lease.owner
            or task.done()
            or self._adoption_seed_leases.get(lease.operation_id) is not lease
        ):
            raise MaintenanceStateError("adoption seed lease is not registered to this task")
        state = read_state(self._conn)
        if (
            state.mode is not MaintenanceMode.BLOCKED
            or state.adoption_state != "pending"
            or state.adoption_id != lease.adoption_id
        ):
            raise MaintenanceClosedError("adoption seed lease is no longer valid")
        row = self._conn.execute(
            "SELECT instance_id, owner_token_hash, generation, operation_kind, owner_task_id, "
            "lifecycle, claim_state FROM maintenance_leases WHERE operation_id = ?",
            (lease.operation_id,),
        ).fetchone()
        if (
            row is None
            or row[0] != self.instance_id
            or row[1] != lease.token_hash
            or row[2] != lease.generation
            or row[3] != "adoption_seed"
            or row[4] != id(task)
            or row[5] != "live"
            or row[6] != "active"
        ):
            raise MaintenanceStateError("persisted adoption seed lease is not live")

    def _delete_adoption_seed_lease(self, lease: _AdoptionSeedLease) -> None:
        if (
            _current_task() is not lease.owner
            or self._adoption_seed_leases.get(lease.operation_id) is not lease
        ):
            raise MaintenanceStateError("adoption seed lease cleanup belongs to its owner task")
        self._assert_no_pending_transaction()
        with self._guard._adoption_seed_lease_delete_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                changed = self._conn.execute(
                    "DELETE FROM maintenance_leases WHERE operation_id = ? AND instance_id = ? "
                    "AND owner_token_hash = ? AND generation = ? AND owner_task_id = ? "
                    "AND operation_kind = 'adoption_seed' AND lifecycle = 'live' "
                    "AND claim_state = 'active'",
                    (
                        lease.operation_id,
                        self.instance_id,
                        lease.token_hash,
                        lease.generation,
                        id(lease.owner),
                    ),
                ).rowcount
                if changed != 1:
                    raise MaintenanceStateError("persisted adoption seed lease disappeared")
            except BaseException:
                if self._conn.in_transaction:
                    self._conn.execute("ROLLBACK")
                raise
            else:
                try:
                    self._conn.execute("COMMIT")
                except BaseException:
                    if self._conn.in_transaction:
                        self._conn.execute("ROLLBACK")
                    raise

    def _insert_adoption_resolution_lease(
        self, task: asyncio.Task[object]
    ) -> _AdoptionResolutionLease:
        if _current_task() is not task or task.done():
            raise MaintenanceStateError("adoption resolution lease must belong to its current task")
        self._assert_no_pending_transaction()
        if self._adoption_seed_leases or self._adoption_resolution_leases:
            raise MaintenanceStateError("an adoption administrative lease is already active")
        token_hash = hashlib.sha256(secrets.token_urlsafe(32).encode()).hexdigest()
        operation_id = str(uuid.uuid4())
        with self._guard._adoption_resolution_lease_insert_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                state = read_state(self._conn)
                if (
                    state.mode is not MaintenanceMode.BLOCKED
                    or state.adoption_state != "pending"
                    or state.adoption_id is None
                ):
                    raise MaintenanceClosedError(
                        "adoption resolution requires BLOCKED pending adoption"
                    )
                self._conn.execute(
                    "INSERT INTO maintenance_leases "
                    "(operation_id, instance_id, owner_token_hash, generation, operation_kind, "
                    "started_at, owner_task_id, lifecycle, claim_state) "
                    "VALUES (?, ?, ?, ?, 'adoption_resolution', ?, ?, 'live', 'active')",
                    (
                        operation_id,
                        self.instance_id,
                        token_hash,
                        state.generation,
                        to_iso(datetime.now(UTC)),
                        id(task),
                    ),
                )
            except BaseException:
                if self._conn.in_transaction:
                    self._conn.execute("ROLLBACK")
                raise
            else:
                try:
                    self._conn.execute("COMMIT")
                except BaseException:
                    if self._conn.in_transaction:
                        self._conn.execute("ROLLBACK")
                    raise
        try:
            authority = self._guard._register_adoption_resolution(
                operation_id,
                token_hash,
                task,
                instance_id=self.instance_id,
                generation=state.generation,
                adoption_id=state.adoption_id,
            )
        except BaseException:
            self._delete_unregistered_adoption_resolution_lease(
                operation_id, token_hash, state.generation, task
            )
            raise
        lease = _AdoptionResolutionLease(
            operation_id,
            token_hash,
            state.generation,
            state.adoption_id,
            task,
            authority,
        )
        self._adoption_resolution_leases[operation_id] = lease
        return lease

    def _assert_adoption_resolution_lease(self, lease: _AdoptionResolutionLease) -> None:
        task = _current_task()
        if (
            task is None
            or task is not lease.owner
            or task.done()
            or self._adoption_resolution_leases.get(lease.operation_id) is not lease
            or not self._guard._adoption_resolution_is_live(lease.authority)
        ):
            raise MaintenanceStateError("adoption resolution lease is not registered to this task")
        state = read_state(self._conn)
        if (
            state.mode is not MaintenanceMode.BLOCKED
            or state.adoption_state != "pending"
            or state.adoption_id != lease.adoption_id
        ):
            raise MaintenanceClosedError("adoption resolution lease is no longer valid")
        row = self._conn.execute(
            "SELECT instance_id, owner_token_hash, generation, operation_kind, owner_task_id, "
            "lifecycle, claim_state FROM maintenance_leases WHERE operation_id = ?",
            (lease.operation_id,),
        ).fetchone()
        if (
            row is None
            or row["instance_id"] != self.instance_id
            or row["owner_token_hash"] != lease.token_hash
            or row["generation"] != lease.generation
            or row["operation_kind"] != "adoption_resolution"
            or row["owner_task_id"] != id(task)
            or row["lifecycle"] != "live"
            or row["claim_state"] != "active"
        ):
            raise MaintenanceStateError("persisted adoption resolution lease is not live")

    def _adoption_resolution_transaction(
        self,
        lease: _AdoptionResolutionLease,
        action: Callable[[], _ResolutionResult],
    ) -> _ResolutionResult:
        self._assert_no_pending_transaction()
        self._conn.execute("BEGIN IMMEDIATE")
        try:
            self._assert_adoption_resolution_lease(lease)
            result = action()
        except BaseException:
            if self._conn.in_transaction:
                self._conn.execute("ROLLBACK")
            raise
        try:
            self._conn.execute("COMMIT")
        except BaseException:
            if self._conn.in_transaction:
                self._conn.execute("ROLLBACK")
            raise
        return result

    def _delete_adoption_resolution_lease(self, lease: _AdoptionResolutionLease) -> None:
        if (
            _current_task() is not lease.owner
            or self._adoption_resolution_leases.get(lease.operation_id) is not lease
        ):
            raise MaintenanceStateError("adoption resolution cleanup belongs to its owner task")
        try:
            self._assert_no_pending_transaction()
            with self._guard._adoption_resolution_lease_delete_scope():
                self._conn.execute("BEGIN IMMEDIATE")
                try:
                    changed = self._conn.execute(
                        "DELETE FROM maintenance_leases WHERE operation_id = ? "
                        "AND instance_id = ? AND owner_token_hash = ? AND generation = ? "
                        "AND owner_task_id = ? AND operation_kind = 'adoption_resolution' "
                        "AND lifecycle = 'live' AND claim_state = 'active'",
                        (
                            lease.operation_id,
                            self.instance_id,
                            lease.token_hash,
                            lease.generation,
                            id(lease.owner),
                        ),
                    ).rowcount
                    if changed != 1:
                        raise MaintenanceStateError(
                            "persisted adoption resolution lease disappeared"
                        )
                except BaseException:
                    if self._conn.in_transaction:
                        self._conn.execute("ROLLBACK")
                    raise
                else:
                    try:
                        self._conn.execute("COMMIT")
                    except BaseException:
                        if self._conn.in_transaction:
                            self._conn.execute("ROLLBACK")
                        raise
        finally:
            self._guard._revoke_adoption_resolution(lease.authority)
            self._adoption_resolution_leases.pop(lease.operation_id, None)

    def _delete_unregistered_adoption_resolution_lease(
        self,
        operation_id: str,
        token_hash: str,
        generation: int,
        task: asyncio.Task[object],
    ) -> None:
        with self._guard._adoption_resolution_lease_delete_scope():
            self._conn.execute("BEGIN IMMEDIATE")
            try:
                self._conn.execute(
                    "DELETE FROM maintenance_leases WHERE operation_id = ? AND instance_id = ? "
                    "AND owner_token_hash = ? AND generation = ? AND owner_task_id = ? "
                    "AND operation_kind = 'adoption_resolution' AND lifecycle = 'live' "
                    "AND claim_state = 'active'",
                    (operation_id, self.instance_id, token_hash, generation, id(task)),
                )
            except BaseException:
                if self._conn.in_transaction:
                    self._conn.execute("ROLLBACK")
                raise
            else:
                self._conn.execute("COMMIT")


def _bounded_text(value: str, label: str, limit: int) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{label} must be text")
    value = value.strip()
    if not value or len(value) > limit:
        raise ValueError(f"{label} must be nonempty and at most {limit} characters")
    return value
