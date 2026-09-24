"""SQLite authorizer for fail-closed maintenance write admission.

The authorizer is deliberately a small policy boundary.  It never consults
SQLite from its callback: all authority state is held in memory and checked
against the process, thread, and current asyncio task.
"""

from __future__ import annotations

import asyncio
import contextlib
import contextvars
import os
import sqlite3
import threading
from collections.abc import Iterator
from dataclasses import dataclass
from typing import Final

from .state import MaintenanceMode

_DML_ACTIONS: Final = frozenset(
    {
        sqlite3.SQLITE_INSERT,
        sqlite3.SQLITE_UPDATE,
        sqlite3.SQLITE_DELETE,
    }
)
_SCHEMA_ACTIONS: Final = frozenset(
    {
        sqlite3.SQLITE_CREATE_INDEX,
        sqlite3.SQLITE_CREATE_TABLE,
        sqlite3.SQLITE_CREATE_TEMP_INDEX,
        sqlite3.SQLITE_CREATE_TEMP_TABLE,
        sqlite3.SQLITE_CREATE_TEMP_TRIGGER,
        sqlite3.SQLITE_CREATE_TEMP_VIEW,
        sqlite3.SQLITE_CREATE_TRIGGER,
        sqlite3.SQLITE_CREATE_VIEW,
        sqlite3.SQLITE_DROP_INDEX,
        sqlite3.SQLITE_DROP_TABLE,
        sqlite3.SQLITE_DROP_TEMP_INDEX,
        sqlite3.SQLITE_DROP_TEMP_TABLE,
        sqlite3.SQLITE_DROP_TEMP_TRIGGER,
        sqlite3.SQLITE_DROP_TEMP_VIEW,
        sqlite3.SQLITE_DROP_TRIGGER,
        sqlite3.SQLITE_DROP_VIEW,
        sqlite3.SQLITE_ALTER_TABLE,
        sqlite3.SQLITE_REINDEX,
        sqlite3.SQLITE_ANALYZE,
        sqlite3.SQLITE_ATTACH,
        sqlite3.SQLITE_DETACH,
        sqlite3.SQLITE_CREATE_VTABLE,
        sqlite3.SQLITE_DROP_VTABLE,
    }
)
_TRANSACTION_ACTIONS: Final = frozenset(
    {
        sqlite3.SQLITE_TRANSACTION,
        sqlite3.SQLITE_SAVEPOINT,
        sqlite3.SQLITE_RECURSIVE,
    }
)
_MAINTENANCE_TABLES: Final = frozenset(
    {
        "maintenance_state",
        "maintenance_leases",
        "delivery_attempts",
        "delivery_attempt_targets",
        "adoption_sources",
    }
)
_MAINTENANCE_INDEXES: Final = frozenset(
    {
        "maintenance_leases_generation_started",
        "delivery_attempts_active_dedupe",
        "delivery_attempts_state_intended",
        "delivery_attempts_message",
        "delivery_attempts_operation",
        "delivery_attempt_targets_active_key",
        "adoption_sources_pending",
        "adoption_sources_grouping",
    }
)
_SCHEMA_ACTIONS_WITH_TABLE_ARG2: Final = frozenset(
    {
        sqlite3.SQLITE_CREATE_INDEX,
        sqlite3.SQLITE_CREATE_TEMP_INDEX,
        sqlite3.SQLITE_CREATE_TRIGGER,
        sqlite3.SQLITE_CREATE_TEMP_TRIGGER,
        sqlite3.SQLITE_DROP_TRIGGER,
        sqlite3.SQLITE_DROP_TEMP_TRIGGER,
        sqlite3.SQLITE_ALTER_TABLE,
    }
)
# These PRAGMAs accept a table/index argument while remaining read-only.
_READ_PRAGMAS_WITH_ARGUMENT: Final = frozenset(
    {"table_info", "index_list", "index_info", "foreign_key_list"}
)
_READ_PRAGMAS_WITHOUT_ARGUMENT: Final = frozenset(
    {
        "application_id",
        "cache_size",
        "compile_options",
        "database_list",
        "encoding",
        "foreign_keys",
        "integrity_check",
        "journal_mode",
        "quick_check",
        "schema_version",
        "synchronous",
        "user_version",
    }
)


@dataclass(frozen=True, slots=True)
class _Authority:
    """Opaque, in-memory SQL capability.

    Runtime capabilities are registered by identity in :class:`WriteGuard`;
    copying this value through a ``ContextVar`` therefore cannot grant a child
    task access.  Metadata capabilities are constructed only by fixed-purpose
    methods below and never accept a caller-selected table set.
    """

    purpose: str
    actions: frozenset[int]
    tables: frozenset[str] | None
    owner: asyncio.Task[object] | None
    pid: int
    thread_id: int
    operation_id: str | None = None
    token_hash: str | None = None
    instance_id: str | None = None
    generation: int | None = None
    adoption_id: str | None = None


class WriteGuard:
    """Install a deny-by-default authorizer before recovery or migration DML."""

    def __init__(self, conn: sqlite3.Connection, mode: MaintenanceMode):
        self._conn = conn
        self._mode = mode
        self._pid = os.getpid()
        self._thread_id = threading.get_ident()
        self._bootstrap_live = True
        self._bootstrap_authority = _Authority(
            "bootstrap",
            _DML_ACTIONS | _SCHEMA_ACTIONS | frozenset({sqlite3.SQLITE_PRAGMA}),
            None,
            None,
            self._pid,
            self._thread_id,
        )
        self._authority: contextvars.ContextVar[_Authority | None] = contextvars.ContextVar(
            "maintenance_sql_authority", default=None
        )
        # Identity-keyed registries prevent an equal, caller-constructed
        # dataclass value from being accepted as the live capability.
        self._runtime: dict[int, _Authority] = {}
        self._adoption_resolution: dict[int, _Authority] = {}
        self._runtime_recovery: dict[int, _Authority] = {}
        self._internal: dict[int, _Authority] = {}
        conn.set_authorizer(self._authorize)

    @property
    def mode(self) -> MaintenanceMode:
        return self._mode

    def _set_mode(self, mode: MaintenanceMode) -> None:
        self._mode = mode

    @contextlib.contextmanager
    def _bootstrap_scope(self) -> Iterator[None]:
        authority = self._bootstrap_authority
        if not self._bootstrap_live:
            raise RuntimeError("bootstrap authority revoked")
        if os.getpid() != authority.pid or threading.get_ident() != authority.thread_id:
            raise RuntimeError("bootstrap authority belongs to another process or thread")
        marker = self._authority.set(authority)
        try:
            yield
        finally:
            self._authority.reset(marker)

    def _revoke_bootstrap(self) -> None:
        self._bootstrap_live = False

    def _new_internal_authority(
        self, purpose: str, actions: frozenset[int], tables: frozenset[str]
    ) -> _Authority:
        task = _current_task()
        if purpose == "recovery":
            if not self._bootstrap_live:
                raise RuntimeError("recovery authority revoked")
        elif task is None:
            raise RuntimeError("internal maintenance authority requires an asyncio task")
        authority = _Authority(
            purpose,
            actions,
            tables,
            task,
            os.getpid(),
            threading.get_ident(),
        )
        self._internal[id(authority)] = authority
        return authority

    @contextlib.contextmanager
    def _internal_scope(
        self, purpose: str, actions: frozenset[int], tables: frozenset[str]
    ) -> Iterator[None]:
        authority = self._new_internal_authority(purpose, actions, tables)
        marker = self._authority.set(authority)
        try:
            yield
        finally:
            self._authority.reset(marker)
            self._internal.pop(id(authority), None)

    # Each method is a separate fixed-purpose capability.  There is
    # intentionally no generic metadata context manager or caller-supplied
    # table/action collection.
    def _admission_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "admit", frozenset({sqlite3.SQLITE_INSERT}), frozenset({"maintenance_leases"})
        )

    def _release_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "release", frozenset({sqlite3.SQLITE_DELETE}), frozenset({"maintenance_leases"})
        )

    def _claim_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "claim", frozenset({sqlite3.SQLITE_UPDATE}), frozenset({"maintenance_leases"})
        )

    def _transition_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "transition", frozenset({sqlite3.SQLITE_UPDATE}), frozenset({"maintenance_state"})
        )

    def _retire_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "retire", frozenset({sqlite3.SQLITE_UPDATE}), frozenset({"maintenance_leases"})
        )

    def _adoption_seed_lease_insert_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "adoption_seed_lease_insert",
            frozenset({sqlite3.SQLITE_INSERT}),
            frozenset({"maintenance_leases"}),
        )

    def _adoption_seed_sources_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "adoption_seed_sources",
            frozenset({sqlite3.SQLITE_INSERT}),
            frozenset({"adoption_sources"}),
        )

    def _adoption_seed_lease_delete_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "adoption_seed_lease_delete",
            frozenset({sqlite3.SQLITE_DELETE}),
            frozenset({"maintenance_leases"}),
        )

    def _adoption_resolution_lease_insert_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "adoption_resolution_lease_insert",
            frozenset({sqlite3.SQLITE_INSERT}),
            frozenset({"maintenance_leases"}),
        )

    def _adoption_resolution_lease_delete_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "adoption_resolution_lease_delete",
            frozenset({sqlite3.SQLITE_DELETE}),
            frozenset({"maintenance_leases"}),
        )

    @contextlib.contextmanager
    def _adoption_group_insert_scope(self, authority: _Authority) -> Iterator[None]:
        if not self._adoption_resolution_is_persisted(authority):
            raise RuntimeError("adoption resolution authority is not live")
        with self._internal_scope(
            "adoption_group_insert",
            frozenset({sqlite3.SQLITE_INSERT}),
            frozenset({"delivery_attempts", "delivery_attempt_targets"}),
        ):
            yield

    @contextlib.contextmanager
    def _adoption_source_bind_scope(self, authority: _Authority) -> Iterator[None]:
        if not self._adoption_resolution_is_persisted(authority):
            raise RuntimeError("adoption resolution authority is not live")
        with self._internal_scope(
            "adoption_source_bind",
            frozenset({sqlite3.SQLITE_UPDATE}),
            frozenset({"adoption_sources"}),
        ):
            yield

    @contextlib.contextmanager
    def _adoption_source_retire_scope(self, authority: _Authority) -> Iterator[None]:
        if not self._adoption_resolution_is_persisted(authority):
            raise RuntimeError("adoption resolution authority is not live")
        with self._internal_scope(
            "adoption_source_retire",
            frozenset({sqlite3.SQLITE_UPDATE}),
            frozenset({"adoption_sources"}),
        ):
            yield

    @contextlib.contextmanager
    def _adoption_attempt_retire_scope(self, authority: _Authority) -> Iterator[None]:
        if not self._adoption_resolution_is_persisted(authority):
            raise RuntimeError("adoption resolution authority is not live")
        with self._internal_scope(
            "adoption_attempt_retire",
            frozenset({sqlite3.SQLITE_UPDATE}),
            frozenset({"delivery_attempts", "delivery_attempt_targets"}),
        ):
            yield

    def _runtime_recovery_lease_insert_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "runtime_recovery_lease_insert",
            frozenset({sqlite3.SQLITE_INSERT}),
            frozenset({"maintenance_leases"}),
        )

    def _runtime_recovery_lease_delete_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "runtime_recovery_lease_delete",
            frozenset({sqlite3.SQLITE_DELETE}),
            frozenset({"maintenance_leases"}),
        )

    @contextlib.contextmanager
    def _runtime_recovery_bind_scope(self, authority: _Authority) -> Iterator[None]:
        if not self._runtime_recovery_is_persisted(authority):
            raise RuntimeError("runtime recovery authority is not live")
        with self._internal_scope(
            "runtime_recovery_bind",
            frozenset({sqlite3.SQLITE_INSERT, sqlite3.SQLITE_UPDATE}),
            frozenset(
                {
                    "delivery_attempts",
                    "delivery_attempt_targets",
                    "reminders",
                    "weekly_digests",
                    "decline_notices",
                    "amendments",
                    "config",
                    "maintenance_state",
                }
            ),
        ):
            yield

    @contextlib.contextmanager
    def _runtime_recovery_retire_scope(self, authority: _Authority) -> Iterator[None]:
        if not self._runtime_recovery_is_persisted(authority):
            raise RuntimeError("runtime recovery authority is not live")
        with self._internal_scope(
            "runtime_recovery_retire",
            frozenset({sqlite3.SQLITE_INSERT, sqlite3.SQLITE_UPDATE}),
            frozenset(
                {
                    "delivery_attempts",
                    "delivery_attempt_targets",
                    "reminders",
                    "decline_notices",
                    "config",
                    "maintenance_state",
                }
            ),
        ):
            yield

    def _recovery_scope(self) -> Iterator[None]:
        # Recovery is constructor-only and dies with the bootstrap window.
        return self._internal_scope(
            "recovery",
            frozenset({sqlite3.SQLITE_UPDATE}),
            _MAINTENANCE_TABLES,
        )

    def _delivery_intent_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "delivery_intent",
            frozenset({sqlite3.SQLITE_INSERT}),
            frozenset({"delivery_attempts", "delivery_attempt_targets"}),
        )

    def _delivery_finalize_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "delivery_finalize",
            frozenset({sqlite3.SQLITE_INSERT, sqlite3.SQLITE_UPDATE}),
            frozenset(
                {
                    "delivery_attempts",
                    "delivery_attempt_targets",
                    "reminders",
                    "weekly_digests",
                    "decline_notices",
                    "amendments",
                    "debug_messages",
                }
            ),
        )

    def _delivery_indeterminate_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "delivery_indeterminate",
            frozenset({sqlite3.SQLITE_UPDATE}),
            frozenset({"delivery_attempts"}),
        )

    def _digest_replacement_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "digest_replacement",
            frozenset({sqlite3.SQLITE_UPDATE}),
            frozenset({"delivery_attempts", "delivery_attempt_targets", "weekly_digests"}),
        )

    def _debug_cleanup_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "debug_cleanup",
            frozenset({sqlite3.SQLITE_DELETE, sqlite3.SQLITE_UPDATE}),
            frozenset({"debug_messages", "delivery_attempts", "delivery_attempt_targets"}),
        )

    def _decline_retraction_scope(self) -> Iterator[None]:
        return self._internal_scope(
            "decline_retraction",
            frozenset({sqlite3.SQLITE_UPDATE}),
            frozenset({"decline_notices", "delivery_attempts", "delivery_attempt_targets"}),
        )

    def _register_application(
        self, operation_id: str, token_hash: str, owner: asyncio.Task[object]
    ) -> _Authority:
        current = _current_task()
        if current is not owner or owner.done() or os.getpid() != self._pid:
            raise RuntimeError("application authority must be registered by its owner task")
        authority = _Authority(
            "application",
            _DML_ACTIONS,
            None,
            owner,
            self._pid,
            threading.get_ident(),
            operation_id,
            token_hash,
        )
        self._runtime[id(authority)] = authority
        return authority

    def _revoke_application(self, authority: _Authority) -> None:
        if self._runtime.get(id(authority)) is authority:
            self._runtime.pop(id(authority), None)

    def _register_adoption_resolution(
        self,
        operation_id: str,
        token_hash: str,
        owner: asyncio.Task[object],
        *,
        instance_id: str,
        generation: int,
        adoption_id: str,
    ) -> _Authority:
        current = _current_task()
        if current is not owner or owner.done() or os.getpid() != self._pid:
            raise RuntimeError("adoption resolution authority must be registered by its owner task")
        authority = _Authority(
            "adoption_resolution",
            frozenset(),
            frozenset(),
            owner,
            self._pid,
            threading.get_ident(),
            operation_id,
            token_hash,
            instance_id,
            generation,
            adoption_id,
        )
        self._adoption_resolution[id(authority)] = authority
        return authority

    def _revoke_adoption_resolution(self, authority: _Authority) -> None:
        if self._adoption_resolution.get(id(authority)) is authority:
            self._adoption_resolution.pop(id(authority), None)

    def _adoption_resolution_is_live(self, authority: _Authority) -> bool:
        if self._adoption_resolution.get(id(authority)) is not authority:
            return False
        if os.getpid() != authority.pid or threading.get_ident() != authority.thread_id:
            return False
        current = _current_task()
        return current is authority.owner and not current.done()

    def _adoption_resolution_is_persisted(self, authority: _Authority) -> bool:
        if (
            not self._adoption_resolution_is_live(authority)
            or authority.operation_id is None
            or authority.token_hash is None
            or authority.instance_id is None
            or authority.generation is None
            or authority.adoption_id is None
        ):
            return False
        try:
            row = self._conn.execute(
                "SELECT l.instance_id, l.owner_token_hash, l.generation, l.operation_kind, "
                "l.owner_task_id, l.lifecycle, l.claim_state, s.mode, s.adoption_state, "
                "s.adoption_id, s.generation FROM maintenance_leases AS l "
                "JOIN maintenance_state AS s ON s.id = 1 WHERE l.operation_id = ?",
                (authority.operation_id,),
            ).fetchone()
        except sqlite3.Error:
            return False
        task = _current_task()
        return (
            row is not None
            and row[0] == authority.instance_id
            and row[1] == authority.token_hash
            and row[2] == authority.generation
            and row[3] == "adoption_resolution"
            and row[4] == id(task)
            and row[5] == "live"
            and row[6] == "active"
            and row[7] == MaintenanceMode.BLOCKED.value
            and row[8] == "pending"
            and row[9] == authority.adoption_id
            and row[10] == authority.generation
        )

    def _register_runtime_recovery(
        self,
        operation_id: str,
        token_hash: str,
        owner: asyncio.Task[object],
        *,
        instance_id: str,
        generation: int,
    ) -> _Authority:
        current = _current_task()
        if current is not owner or owner.done() or os.getpid() != self._pid:
            raise RuntimeError("runtime recovery authority must be registered by its owner task")
        authority = _Authority(
            "runtime_recovery",
            frozenset(),
            frozenset(),
            owner,
            self._pid,
            threading.get_ident(),
            operation_id,
            token_hash,
            instance_id,
            generation,
        )
        self._runtime_recovery[id(authority)] = authority
        return authority

    def _revoke_runtime_recovery(self, authority: _Authority) -> None:
        if self._runtime_recovery.get(id(authority)) is authority:
            self._runtime_recovery.pop(id(authority), None)

    def _runtime_recovery_is_live(self, authority: _Authority) -> bool:
        if self._runtime_recovery.get(id(authority)) is not authority:
            return False
        if os.getpid() != authority.pid or threading.get_ident() != authority.thread_id:
            return False
        current = _current_task()
        return current is authority.owner and not current.done()

    def _runtime_recovery_is_persisted(self, authority: _Authority) -> bool:
        if (
            not self._runtime_recovery_is_live(authority)
            or authority.operation_id is None
            or authority.token_hash is None
            or authority.instance_id is None
            or authority.generation is None
        ):
            return False
        try:
            row = self._conn.execute(
                "SELECT l.instance_id, l.owner_token_hash, l.generation, l.operation_kind, "
                "l.owner_task_id, l.lifecycle, l.claim_state, s.mode, s.generation "
                "FROM maintenance_leases AS l JOIN maintenance_state AS s ON s.id = 1 "
                "WHERE l.operation_id = ?",
                (authority.operation_id,),
            ).fetchone()
        except sqlite3.Error:
            return False
        task = _current_task()
        return (
            row is not None
            and row[0] == authority.instance_id
            and row[1] == authority.token_hash
            and row[2] == authority.generation
            and row[3] == "runtime_recovery"
            and row[4] == id(task)
            and row[5] == "live"
            and row[6] == "active"
            and row[7] in {MaintenanceMode.BLOCKED.value, MaintenanceMode.FROZEN.value}
            and row[8] == authority.generation
        )

    @contextlib.contextmanager
    def _application_scope(self, authority: _Authority) -> Iterator[None]:
        if not self._runtime_is_live(authority):
            raise RuntimeError("application authority is not live")
        marker = self._authority.set(authority)
        try:
            yield
        finally:
            self._authority.reset(marker)

    def _runtime_is_live(self, authority: _Authority) -> bool:
        if self._runtime.get(id(authority)) is not authority:
            return False
        if os.getpid() != authority.pid or threading.get_ident() != authority.thread_id:
            return False
        current = _current_task()
        return current is authority.owner and not current.done()

    def _authority_is_live(self, authority: _Authority | None) -> bool:
        if authority is None:
            return False
        if os.getpid() != authority.pid or threading.get_ident() != authority.thread_id:
            return False
        if authority.purpose == "bootstrap":
            return self._bootstrap_live and authority is self._bootstrap_authority
        if authority.purpose == "application":
            return self._runtime_is_live(authority)
        if authority.purpose == "adoption_resolution":
            return self._adoption_resolution_is_live(authority)
        if authority.purpose == "runtime_recovery":
            return self._runtime_recovery_is_live(authority)
        if self._internal.get(id(authority)) is not authority:
            return False
        current = _current_task()
        if authority.purpose == "recovery":
            return self._bootstrap_live and current is authority.owner
        return authority.owner is not None and current is authority.owner and not current.done()

    def _authorize(
        self, action: int, arg1: str | None, arg2: str | None, database: str | None, _source: str
    ) -> int:
        """Authorize compilation using memory-only state; never execute SQL here."""
        if database not in {"main", "temp", None}:
            return sqlite3.SQLITE_DENY

        if action == sqlite3.SQLITE_PRAGMA:
            if arg2 is None and arg1 in _READ_PRAGMAS_WITHOUT_ARGUMENT:
                return sqlite3.SQLITE_OK
            if arg2 is not None and arg1 in _READ_PRAGMAS_WITH_ARGUMENT:
                return sqlite3.SQLITE_OK
            # WAL checkpointing is an existing OPEN-mode maintenance operation.
            # It is denied once admission is closed and is never part of a
            # runtime lease capability.
            if arg1 == "wal_checkpoint" and self._mode is MaintenanceMode.OPEN:
                return sqlite3.SQLITE_OK
            # Integrity-check toggles are never part of a closed-state lease.
            if arg1 == "ignore_check_constraints" and self._mode is MaintenanceMode.OPEN:
                return sqlite3.SQLITE_OK
            return self._write_allowed(action, None)

        if action in {sqlite3.SQLITE_READ, sqlite3.SQLITE_SELECT, *_TRANSACTION_ACTIONS}:
            return sqlite3.SQLITE_OK
        if action in _DML_ACTIONS or action in _SCHEMA_ACTIONS:
            target = (
                arg2
                if action in _SCHEMA_ACTIONS_WITH_TABLE_ARG2
                else arg1
                if action in _SCHEMA_ACTIONS
                else None
            )
            return self._write_allowed(action, arg1, target_table=target)
        return sqlite3.SQLITE_OK

    def _write_allowed(
        self, action: int, table: str | None, *, target_table: str | None = None
    ) -> int:
        authority = self._authority.get()
        if self._authority_is_live(authority):
            assert authority is not None
            if action not in authority.actions:
                return sqlite3.SQLITE_DENY
            if authority.purpose == "bootstrap":
                return sqlite3.SQLITE_OK
            if authority.purpose == "application":
                # Application leases never reach the maintenance metadata.
                return (
                    sqlite3.SQLITE_OK
                    if table is not None
                    and table not in _MAINTENANCE_TABLES
                    and not table.startswith("sqlite_")
                    else sqlite3.SQLITE_DENY
                )
            return (
                sqlite3.SQLITE_OK
                if table is not None and authority.tables is not None and table in authority.tables
                else sqlite3.SQLITE_DENY
            )

        # A copied ContextVar may carry a revoked capability into another task.
        # It must not fall through to OPEN's transitional ordinary-write rule.
        if authority is not None:
            return sqlite3.SQLITE_DENY

        # Transitional OPEN behavior keeps existing repository methods working.
        # Once admission closes, only an admitted operation can perform ordinary
        # DML; schema/attachment/PRAGMA writes never belong to that capability.
        if self._mode is MaintenanceMode.OPEN:
            if action in _DML_ACTIONS:
                return (
                    sqlite3.SQLITE_OK
                    if table is not None and table not in _MAINTENANCE_TABLES
                    else sqlite3.SQLITE_DENY
                )
            if action in _SCHEMA_ACTIONS:
                if action in {sqlite3.SQLITE_ATTACH, sqlite3.SQLITE_DETACH}:
                    return sqlite3.SQLITE_DENY
                return (
                    sqlite3.SQLITE_OK
                    if target_table not in _MAINTENANCE_TABLES | _MAINTENANCE_INDEXES
                    else sqlite3.SQLITE_DENY
                )
        return sqlite3.SQLITE_DENY


def _current_task() -> asyncio.Task[object] | None:
    try:
        return asyncio.current_task()
    except RuntimeError:
        return None
