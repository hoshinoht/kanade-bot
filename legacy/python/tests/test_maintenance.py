"""Durable maintenance foundation behavior on synthetic SQLite stores."""

from __future__ import annotations

import asyncio
import sqlite3

import pytest

from bot.infrastructure.db import Repo
from bot.infrastructure.maintenance.coordinator import (
    MaintenanceClosedError,
    MaintenanceStateError,
)


def test_prepare_stays_preparing_and_denies_direct_application_writes(tmp_path, owner_lock_dir):
    repo = Repo(tmp_path / "fresh.sqlite", owner_lock_dir=owner_lock_dir)

    assert asyncio.run(repo.maintenance.prepare())
    assert repo._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "PREPARING"
    with pytest.raises(sqlite3.DatabaseError, match="not authorized"):
        repo.set_config("paused", "1")
    with pytest.raises(sqlite3.DatabaseError, match="not authorized"):
        repo._conn.execute("CREATE TABLE unauthorized (id INTEGER)")
    with pytest.raises(sqlite3.DatabaseError, match="not authorized"):
        repo._conn.execute("ATTACH DATABASE ':memory:' AS unauthorized")
    with pytest.raises(sqlite3.DatabaseError, match="not authorized"):
        repo._conn.execute("PRAGMA user_version = 1")
    repo.close()


def test_persistent_alias_cannot_open_a_second_writable_repo(tmp_path, owner_lock_dir):
    path = tmp_path / "fresh.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    alias = tmp_path / "alias.sqlite"
    alias.symlink_to(path)
    with pytest.raises(RuntimeError, match="ownership unavailable"):
        Repo(alias, owner_lock_dir=owner_lock_dir)
    repo.close()
    reopened = Repo(alias, owner_lock_dir=owner_lock_dir)
    reopened.close()


def test_nested_operation_reuses_lease_and_finishes_after_prepare_timeout(tmp_path, owner_lock_dir):
    repo = Repo(tmp_path / "fresh.sqlite", owner_lock_dir=owner_lock_dir)

    async def scenario():
        entered = asyncio.Event()
        release = asyncio.Event()

        async def worker():
            async with repo.maintenance.operation("write") as outer:
                async with repo.maintenance.operation("nested") as inner:
                    assert outer.operation_id == inner.operation_id
                repo.set_config("before", "1")
                entered.set()
                await release.wait()
                repo.set_config("after", "1")

        task = asyncio.create_task(worker())
        await entered.wait()
        assert not await repo.maintenance.prepare(timeout=0)
        release.set()
        await task

    asyncio.run(scenario())
    assert repo.get_config("after") == "1"
    assert repo._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
    repo.close()


def test_copied_task_context_cannot_use_parent_lease(tmp_path, owner_lock_dir):
    repo = Repo(tmp_path / "fresh.sqlite", owner_lock_dir=owner_lock_dir)

    async def scenario():
        async with repo.maintenance.operation("parent"):
            assert not await repo.maintenance.prepare(timeout=0)

            async def child():
                with pytest.raises(MaintenanceClosedError):
                    async with repo.maintenance.operation("child"):
                        pass
                with pytest.raises(sqlite3.DatabaseError, match="not authorized"):
                    repo.set_config("copied", "no")

            await asyncio.create_task(child())

    asyncio.run(scenario())
    repo.close()


def test_restart_orphan_blocks_and_requires_reasoned_retirement(tmp_path, owner_lock_dir):
    path = tmp_path / "fresh.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    repo.close()
    seed = sqlite3.connect(path)
    seed.execute(
        "INSERT INTO maintenance_leases "
        "(operation_id, instance_id, owner_token_hash, generation, operation_kind, "
        "started_at, owner_task_id) "
        "VALUES ('orphan', 'old', ?, 0, 'write', '2026-09-22T00:00:00+00:00', 1)",
        ("a" * 64,),
    )
    seed.commit()
    seed.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    assert reopened._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
    orphaned_at = reopened._conn.execute(
        "SELECT orphaned_at FROM maintenance_leases WHERE operation_id = 'orphan'"
    ).fetchone()[0]
    assert "+00:00" in orphaned_at
    reopened.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    assert (
        reopened._conn.execute(
            "SELECT orphaned_at FROM maintenance_leases WHERE operation_id = 'orphan'"
        ).fetchone()[0]
        == orphaned_at
    )
    with pytest.raises(ValueError):
        reopened.maintenance.retire_orphan("orphan", "", "reason")
    before_taskless_retirement = tuple(
        reopened._conn.execute(
            "SELECT lifecycle, orphaned_at, retired_at, retired_by, retirement_reason "
            "FROM maintenance_leases WHERE operation_id = 'orphan'"
        ).fetchone()
    )
    with pytest.raises(MaintenanceStateError):
        reopened.maintenance.retire_orphan("orphan", "operator", "checked restart")
    assert (
        tuple(
            reopened._conn.execute(
                "SELECT lifecycle, orphaned_at, retired_at, retired_by, retirement_reason "
                "FROM maintenance_leases WHERE operation_id = 'orphan'"
            ).fetchone()
        )
        == before_taskless_retirement
    )

    async def retire():
        reopened.maintenance.retire_orphan("orphan", "operator", "checked restart")

    asyncio.run(retire())
    assert tuple(
        reopened._conn.execute(
            "SELECT lifecycle, retired_by, retirement_reason FROM maintenance_leases"
        ).fetchone()
    ) == ("retired", "operator", "checked restart")
    reopened.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    assert tuple(
        reopened._conn.execute(
            "SELECT lifecycle, orphaned_at, retired_by, retirement_reason "
            "FROM maintenance_leases WHERE operation_id = 'orphan'"
        ).fetchone()
    ) == ("retired", orphaned_at, "operator", "checked restart")
    reopened.close()


@pytest.mark.parametrize("blocker", ["pending_adoption", "indeterminate_delivery"])
def test_prepare_cannot_report_success_with_persistent_blockers(tmp_path, owner_lock_dir, blocker):
    path = tmp_path / f"{blocker}.sqlite"
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    repo.close()
    seed = sqlite3.connect(path)
    if blocker == "pending_adoption":
        seed.execute(
            "UPDATE maintenance_state SET mode = 'BLOCKED', adoption_state = 'pending', "
            "adoption_id = 'adoption-1'"
        )
    else:
        seed.execute(
            "INSERT INTO delivery_attempts "
            "(attempt_id, operation_id, effect_ordinal, owner_instance_id, origin, effect_kind, "
            "dedupe_scope, dedupe_key, state, destination_kind, fingerprint_version, "
            "request_fingerprint, intended_at) "
            "VALUES ('attempt-1', 'operation-1', 0, 'instance-1', 'runtime', 'reminder', "
            "'native', ?, 'indeterminate', 'channel', 1, ?, ?)",
            (
                "a" * 64,
                "b" * 64,
                "2026-09-22T00:00:00+00:00",
            ),
        )
    seed.commit()
    seed.close()

    reopened = Repo(path, owner_lock_dir=owner_lock_dir)
    assert reopened._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
    assert not asyncio.run(reopened.maintenance.prepare(timeout=0))
    assert reopened._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
    reopened.close()
