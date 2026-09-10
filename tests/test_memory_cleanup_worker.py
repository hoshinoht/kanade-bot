"""The reminder worker runs governed-memory retention independently of chat."""

from __future__ import annotations

import asyncio
from datetime import datetime, timedelta
from types import SimpleNamespace

from bot.agent import client as client_module
from bot.agent.client import BossBot

from .conftest import kl


class CleanupRepo:
    def __init__(self) -> None:
        self.cleanup_calls: list[datetime] = []
        self.heartbeat_calls: list[datetime] = []
        self.successful_results: list[dict[str, int]] = []
        self.failures = 1

    def heartbeat(self, now: datetime) -> None:
        self.heartbeat_calls.append(now)

    def cleanup_memories(self, *, now: datetime) -> dict[str, int]:
        self.cleanup_calls.append(now)
        if self.failures:
            self.failures -= 1
            raise RuntimeError("temporary database failure")
        result = {"expired": 1, "purged": 2}
        self.successful_results.append(result)
        return result


def test_memory_cleanup_is_disabled_switch_independent_bounded_and_retryable(monkeypatch, caplog):
    repo = CleanupRepo()
    client = BossBot.__new__(BossBot)
    client.repo = repo
    client.settings = SimpleNamespace(chat_memory_enabled=False)
    client._last_memory_cleanup = None
    client._week_rolled_over = lambda _now: False
    reminder_ticks: list[datetime] = []

    async def record_reminder_tick(now: datetime) -> None:
        reminder_ticks.append(now)

    async def no_op(_now: datetime) -> None:
        pass

    client.post_week_digest = no_op
    client.expire_proposals = no_op
    client.dispatch_reminders = record_reminder_tick
    client.back_up = lambda _now: None
    monkeypatch.setattr(client_module, "mark_done", lambda _repo, _now: None)

    start = kl(2026, 9, 10, 12)
    rolled_back = start - timedelta(days=1)
    clock = iter(
        (
            start,
            start + timedelta(minutes=30),
            rolled_back,
            rolled_back + timedelta(minutes=5),
            rolled_back + timedelta(minutes=10),
        )
    )
    monkeypatch.setattr(client_module, "utcnow", lambda: next(clock))
    monotonic_clock = iter((100.0, 1900.0, 3700.0, 5500.0, 7300.0))
    monkeypatch.setattr(client_module, "monotonic", lambda: next(monotonic_clock))

    async def run_ticks() -> None:
        for _ in range(5):
            await BossBot.tick.coro(client)

    asyncio.run(run_ticks())

    assert client.settings.chat_memory_enabled is False
    assert repo.cleanup_calls == [
        start,
        rolled_back,
        rolled_back + timedelta(minutes=10),
    ]
    assert all(timestamp.tzinfo is not None for timestamp in repo.cleanup_calls)
    assert repo.successful_results == [{"expired": 1, "purged": 2}] * 2
    assert reminder_ticks == [
        start,
        start + timedelta(minutes=30),
        rolled_back,
        rolled_back + timedelta(minutes=5),
        rolled_back + timedelta(minutes=10),
    ]
    assert "scheduled memory cleanup failed" in caplog.text

    fresh_repo = CleanupRepo()
    fresh_repo.failures = 0
    fresh = BossBot.__new__(BossBot)
    fresh.repo = fresh_repo
    fresh.settings = SimpleNamespace(chat_memory_enabled=False)
    fresh._last_memory_cleanup = None
    fresh._week_rolled_over = lambda _now: False
    fresh.post_week_digest = no_op
    fresh.expire_proposals = no_op
    fresh.dispatch_reminders = no_op
    fresh.back_up = lambda _now: None
    fresh_wall = rolled_back + timedelta(minutes=20)
    monkeypatch.setattr(client_module, "utcnow", lambda: fresh_wall)
    monkeypatch.setattr(client_module, "monotonic", lambda: 9000.0)
    asyncio.run(BossBot.tick.coro(fresh))
    assert fresh_repo.cleanup_calls == [fresh_wall]
