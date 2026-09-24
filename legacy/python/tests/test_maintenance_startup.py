"""Closed maintenance stores survive startup with zero DML and a reachable API."""

from __future__ import annotations

import asyncio
import sqlite3
from pathlib import Path

import httpx
import pytest

from bot import __main__ as entrypoint
from bot.agent.client import CFG_EXTRACT, BossBot
from bot.infrastructure.db import Repo
from bot.infrastructure.maintenance.coordinator import MaintenanceCoordinator

from .fake_bot import make_settings

CLOSED_STATES = {
    "blocked": "UPDATE maintenance_state SET mode = 'BLOCKED'",
    "frozen": "UPDATE maintenance_state SET mode = 'FROZEN'",
    "pending_adoption": (
        "UPDATE maintenance_state SET adoption_state = 'pending', adoption_id = 'adoption-1'"
    ),
}

STARTUP_WORK = (
    "sync_roster",
    "materialise_weeks",
    "cache_identity",
    "backfill_all",
    "post_week_digest",
    "expire_proposals",
    "dispatch_reminders",
    "back_up",
)


def _store(tmp_path, owner_lock_dir, update: str | None):
    path = tmp_path / "bot.sqlite"
    Repo(path, owner_lock_dir=owner_lock_dir).close()
    if update is not None:
        seed = sqlite3.connect(path)
        seed.execute(update)
        seed.commit()
        seed.close()
    return path


def _dump(path) -> dict[str, list[tuple]]:
    conn = sqlite3.connect(path.absolute().as_uri() + "?mode=ro", uri=True)
    try:
        tables = [
            row[0]
            for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
            )
        ]
        return {
            t: sorted(conn.execute(f'SELECT * FROM "{t}"').fetchall(), key=repr) for t in tables
        }
    finally:
        conn.close()


def _spy(bot: BossBot) -> list[str]:
    calls: list[str] = []

    def record(name):
        async def async_stub(*_a, **_k):
            calls.append(name)

        def sync_stub(*_a, **_k):
            calls.append(name)

        return sync_stub if name in {"materialise_weeks", "back_up"} else async_stub

    for name in STARTUP_WORK:
        setattr(bot, name, record(name))

    async def no_sync(**_k):
        return []

    bot.tree.sync = no_sync
    return calls


async def _start(settings, monkeypatch, bosses):
    """Run build_repo and bot setup through API startup, then one on_ready and tick."""
    opened: list[tuple[Repo, int, dict]] = []

    def opening(*args, **kwargs):
        repo = Repo(*args, **kwargs)
        opened.append((repo, repo._conn.total_changes, _dump(Path(settings.db_path))))
        return repo

    monkeypatch.setattr(entrypoint, "Repo", opening)
    repo = await entrypoint.build_repo(settings)
    bot = BossBot(settings, repo, bosses)
    calls = _spy(bot)
    try:
        await bot.setup_hook()
        while not bot.api.server.started:
            await asyncio.sleep(0.01)
        port = bot.api.server.servers[0].sockets[0].getsockname()[1]
        async with httpx.AsyncClient() as client:
            health = await client.get(f"http://127.0.0.1:{port}/healthz")
        rescans_running = bot.rescans.running
        await bot.on_ready()
        await bot.tick.coro(bot)
    finally:
        bot.tick.cancel()
        await bot.api.stop()
        await bot.rescans.stop()
    return bot, calls, health, rescans_running, opened[0]


def _settings(path, owner_lock_dir):
    return make_settings(
        db_path=str(path),
        db_owner_lock_dir=str(owner_lock_dir),
        api_host="127.0.0.1",
        api_port=0,
        tick_seconds=600,
    )


@pytest.mark.parametrize("state", sorted(CLOSED_STATES))
def test_closed_store_starts_with_zero_dml_and_a_reachable_api(
    tmp_path, owner_lock_dir, monkeypatch, bosses, state
):
    path = _store(tmp_path, owner_lock_dir, CLOSED_STATES[state])
    before = _dump(path)
    settings = _settings(path, owner_lock_dir)

    bot, calls, health, rescans_running, (repo, changes, bootstrapped) = asyncio.run(
        _start(settings, monkeypatch, bosses)
    )
    try:
        assert health.status_code == 200 and health.text.strip() == "ok"
        assert calls == []
        assert not rescans_running
        assert bot.reconciliation_needed
        # Nothing after Repo bootstrap wrote, including seeding and the heartbeat.
        assert repo._conn.total_changes == changes
        assert _dump(path) == bootstrapped
        assert repo.get_config(CFG_EXTRACT) is None
        assert repo.get_config("heartbeat") is None
        assert repo._conn.execute("SELECT count(*) FROM maintenance_leases").fetchone()[0] == 0
        if state != "pending_adoption":
            # Bootstrap has nothing to normalize here, so the file is untouched.
            assert _dump(path) == before
    finally:
        repo.close()


def test_open_store_seeds_config_inside_a_released_operation_and_runs_startup(
    tmp_path, owner_lock_dir, monkeypatch, bosses
):
    path = _store(tmp_path, owner_lock_dir, None)
    settings = _settings(path, owner_lock_dir)
    kinds: list[str] = []
    original = MaintenanceCoordinator.operation

    def recording(self, kind):
        kinds.append(kind)
        return original(self, kind)

    monkeypatch.setattr(MaintenanceCoordinator, "operation", recording)

    bot, calls, health, rescans_running, (repo, _changes, _snapshot) = asyncio.run(
        _start(settings, monkeypatch, bosses)
    )
    try:
        assert health.status_code == 200
        assert kinds == ["startup_config_seed"]
        assert repo.get_config(CFG_EXTRACT) == ("1" if settings.extract_enabled else "0")
        assert repo._conn.execute("SELECT count(*) FROM maintenance_leases").fetchone()[0] == 0
        assert rescans_running
        assert not bot.reconciliation_needed
        assert calls == [
            "sync_roster",
            "materialise_weeks",
            "cache_identity",
            "backfill_all",
            # The stubbed on_ready stamped no week, so the tick sees a rollover.
            "materialise_weeks",
            "post_week_digest",
            "expire_proposals",
            "dispatch_reminders",
            "back_up",
        ]
        assert repo.get_config("heartbeat") is not None
    finally:
        repo.close()


def test_closed_tick_does_nothing_and_logs_once(tmp_path, owner_lock_dir, bosses, caplog):
    path = _store(tmp_path, owner_lock_dir, CLOSED_STATES["frozen"])
    repo = Repo(path, owner_lock_dir=owner_lock_dir)
    try:
        bot = BossBot(_settings(path, owner_lock_dir), repo, bosses)
        calls = _spy(bot)

        async def ticks():
            for _ in range(3):
                await bot.tick.coro(bot)

        with caplog.at_level("WARNING", logger="bot.agent.client"):
            asyncio.run(ticks())
        assert calls == []
        assert bot.reconciliation_needed
        assert sum("maintenance is FROZEN" in r.message for r in caplog.records) == 1
        assert repo.get_config("heartbeat") is None
    finally:
        repo.close()
