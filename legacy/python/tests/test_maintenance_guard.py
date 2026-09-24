"""Fail-closed SQL-authorizer regressions for maintenance metadata."""

from __future__ import annotations

import asyncio
import sqlite3

import pytest

from bot.infrastructure.db import Repo


def test_metadata_and_privileged_sql_are_denied_while_open():
    repo = Repo(":memory:")
    try:
        for sql in (
            "UPDATE maintenance_state SET mode = 'BLOCKED'",
            "INSERT INTO maintenance_leases "
            "(operation_id, instance_id, owner_token_hash, generation, operation_kind, started_at, "
            "owner_task_id) "
            "VALUES ('forged', 'instance', 'hash', 0, 'work', 'now', 1)",
            "DELETE FROM delivery_attempts",
            "DROP TABLE maintenance_state",
            "ATTACH DATABASE ':memory:' AS forged",
            "VACUUM",
        ):
            with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                repo._conn.execute(sql)
        assert not hasattr(repo._guard, "administrative")
        assert not hasattr(repo._guard, "lease")
        assert asyncio.run(repo.maintenance.prepare())
        for sql in (
            "CREATE TABLE closed_forged (id INTEGER)",
            "PRAGMA user_version = 1",
        ):
            with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                repo._conn.execute(sql)
    finally:
        repo.close()


def test_revoked_copied_context_does_not_fall_through_to_open_writes():
    repo = Repo(":memory:")

    async def scenario():
        ready = asyncio.Event()
        run_child = asyncio.Event()

        async def child():
            await run_child.wait()
            with pytest.raises(sqlite3.DatabaseError, match="authorized"):
                repo.set_config("stale", "denied")
            ready.set()

        async with repo.maintenance.operation("parent"):
            child_task = asyncio.create_task(child())
        run_child.set()
        await ready.wait()
        await child_task

    try:
        asyncio.run(scenario())
        assert repo.get_config("stale") is None
    finally:
        repo.close()


def test_recovery_authority_is_constructor_only():
    repo = Repo(":memory:")
    try:
        with pytest.raises(RuntimeError, match="recovery authority revoked"):
            with repo._guard._recovery_scope():
                pass
    finally:
        repo.close()


def test_internal_metadata_authority_rejects_different_and_stale_tasks():
    repo = Repo(":memory:")

    async def scenario():
        release = asyncio.Event()

        async def child(wait_for_release: bool):
            if wait_for_release:
                await release.wait()
            with pytest.raises(sqlite3.DatabaseError, match="authoriz"):
                repo._conn.execute(
                    "UPDATE maintenance_leases SET lifecycle = 'retired' "
                    "WHERE operation_id = 'not-present'"
                )

        with repo._guard._retire_scope():
            different_task = asyncio.create_task(child(False))
            await different_task
            stale_task = asyncio.create_task(child(True))
        release.set()
        await stale_task

    try:
        asyncio.run(scenario())
    finally:
        repo.close()
