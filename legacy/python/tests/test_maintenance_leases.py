"""Task ownership, cancellation, and child-reservation regressions."""

from __future__ import annotations

import asyncio
from dataclasses import replace

import pytest

from bot.infrastructure.db import Repo
from bot.infrastructure.maintenance.coordinator import (
    MaintenanceClosedError,
    MaintenanceStateError,
    MaintenanceTransactionError,
)


def test_prepare_cancellation_blocks_without_revoking_accepted_work():
    repo = Repo(":memory:")

    async def scenario():
        entered = asyncio.Event()
        release = asyncio.Event()

        async def worker():
            async with repo.maintenance.operation("write"):
                entered.set()
                await release.wait()
                repo.set_config("completed", "yes")

        worker_task = asyncio.create_task(worker())
        await entered.wait()
        prepare_task = asyncio.create_task(repo.maintenance.prepare())
        await asyncio.sleep(0)
        prepare_task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await prepare_task
        assert repo._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "BLOCKED"
        with pytest.raises(MaintenanceClosedError):
            async with repo.maintenance.operation("new"):
                pass
        release.set()
        await worker_task

    try:
        asyncio.run(scenario())
        assert repo.get_config("completed") == "yes"
    finally:
        repo.close()


def test_reserved_child_claims_after_admission_closes_and_drains():
    repo = Repo(":memory:")

    async def scenario():
        reservation = repo.maintenance.reserve_child("refresh")
        prepare_task = asyncio.create_task(repo.maintenance.prepare())
        await asyncio.sleep(0)
        assert not prepare_task.done()
        entered = asyncio.Event()
        release = asyncio.Event()

        async def child():
            async with repo.maintenance.claim_child(reservation) as lease:
                assert lease.claim_state == "claimed"
                repo.set_config("child", "yes")
                entered.set()
                await release.wait()

        child_task = asyncio.create_task(child())
        await entered.wait()
        assert not prepare_task.done()
        release.set()
        await child_task
        assert await prepare_task
        assert repo._conn.execute("SELECT mode FROM maintenance_state").fetchone()[0] == "PREPARING"

    try:
        asyncio.run(scenario())
        assert repo.get_config("child") == "yes"
    finally:
        repo.close()


def test_reservation_transfer_is_single_use_and_parent_is_not_authoritative():
    repo = Repo(":memory:")

    async def scenario():
        reservation = repo.maintenance.reserve_child("refresh")
        with pytest.raises(MaintenanceStateError):
            async with repo.maintenance.claim_child(reservation):
                pass

        async def child(candidate):
            async with repo.maintenance.claim_child(candidate):
                pass

        first = asyncio.create_task(child(reservation))
        await first
        with pytest.raises(MaintenanceStateError):
            await asyncio.create_task(child(reservation))
        with pytest.raises(MaintenanceStateError):
            await asyncio.create_task(child(replace(reservation)))
        with pytest.raises(MaintenanceStateError):
            repo.maintenance.release_child(reservation)

    try:
        asyncio.run(scenario())
    finally:
        repo.close()


def test_unclaimed_reservation_requires_explicit_owner_release():
    repo = Repo(":memory:")

    async def scenario():
        reservation = repo.maintenance.reserve_child("refresh")
        assert repo.maintenance.has_live_work()

        async def wrong_owner():
            with pytest.raises(MaintenanceStateError):
                repo.maintenance.release_child(reservation)

        await asyncio.create_task(wrong_owner())
        repo.maintenance.release_child(reservation)
        assert not repo.maintenance.has_live_work()
        assert repo._conn.execute("SELECT COUNT(*) FROM maintenance_leases").fetchone()[0] == 0

    try:
        asyncio.run(scenario())
    finally:
        repo.close()


def test_admission_rejects_an_external_pending_transaction():
    repo = Repo(":memory:")
    repo._conn.execute("BEGIN")
    try:

        async def scenario():
            with pytest.raises(MaintenanceTransactionError):
                async with repo.maintenance.operation("write"):
                    pass

        asyncio.run(scenario())
        assert repo._conn.in_transaction
    finally:
        repo._conn.rollback()
        repo.close()
