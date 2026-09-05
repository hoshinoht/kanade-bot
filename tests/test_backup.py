"""The daily database snapshot (DESIGN.md §4, "Runtime on this machine").

The live database moved into a Docker named volume after a hard kill over a
bind mount corrupted it, which left the schedule with exactly one copy, in the
one place the host cannot see.  These cover the way back out: a consistent
snapshot per local day, onto a bind mount nested inside the volume, pruned to a
fortnight, and never able to take the tick loop down with it.
"""

from __future__ import annotations

import sqlite3
from datetime import UTC, datetime, timedelta

import pytest

from bot.agent.client import CFG_LAST_BACKUP, BossBot
from bot.infrastructure import backup
from bot.infrastructure.db import Repo

from .conftest import TZ, kl


@pytest.fixture
def db(tmp_path) -> Repo:
    """A file-backed database -- ``:memory:`` has nothing to snapshot."""
    repo = Repo(tmp_path / "data" / "bot.sqlite")
    yield repo
    repo.close()


@pytest.fixture
def bot(db: Repo):
    """A client with only what :meth:`BossBot.back_up` reaches for."""
    client = BossBot.__new__(BossBot)
    client.repo = db
    client.tz = TZ
    client._backup_failed_on = None
    return client


def backups(bot) -> list[str]:
    directory = backup.backup_dir(bot.repo.path)
    return sorted(p.name for p in directory.iterdir()) if directory.exists() else []


# --- where snapshots go -----------------------------------------------------


def test_the_directory_follows_the_database(tmp_path):
    assert backup.backup_dir(tmp_path / "bot.sqlite") == tmp_path / "backups"


def test_an_in_memory_database_has_nothing_to_back_up():
    assert backup.backup_dir(":memory:") is None


# --- when one is due --------------------------------------------------------


def test_the_first_ever_snapshot_is_taken_immediately():
    """Whatever the hour: a database with no backup at all is the urgent case."""
    assert backup.due_day(None, kl(2026, 8, 31, 23, 12)) == "2026-08-31"


def test_a_day_already_backed_up_is_left_alone():
    assert backup.due_day("2026-08-31", kl(2026, 8, 31, 23, 59)) is None


def test_the_daily_snapshot_waits_for_the_quiet_hour():
    assert backup.due_day("2026-08-30", kl(2026, 8, 31, 3, 59)) is None
    assert backup.due_day("2026-08-30", kl(2026, 8, 31, 4, 0)) == "2026-08-31"


def test_a_host_that_slept_through_the_quiet_hour_catches_up_on_wake():
    """The Mac was shut for the night; 11am is when it finds out."""
    assert backup.due_day("2026-08-30", kl(2026, 8, 31, 11, 0)) == "2026-08-31"


def test_a_whole_missed_day_does_not_wait_for_04_00_again():
    """Back from a week away at 01:00: the gap is already days wide."""
    assert backup.due_day("2026-08-24", kl(2026, 8, 31, 1, 0)) == "2026-08-31"


# --- writing one ------------------------------------------------------------


def test_a_snapshot_is_a_database_you_can_read_back(bot):
    bot.repo.set_config("day_of_ping_time", "21:30")
    path = bot.back_up(kl(2026, 8, 31, 4, 0))

    assert path.name == "bot.sqlite.20260831-040000"
    copy = sqlite3.connect(path)
    try:
        row = copy.execute("SELECT value FROM config WHERE key = 'day_of_ping_time'").fetchone()
    finally:
        copy.close()
    assert row[0] == "21:30"


def test_the_snapshot_holds_writes_the_wal_had_not_checkpointed(bot):
    """Why the backup API and not `cp`: in WAL mode the .sqlite file alone is
    only whatever was last checkpointed."""
    run = bot.repo.create_run(kl(2026, 8, 27), ["HMaleficStar"], kl(2026, 8, 31, 21, 30), ["1"])
    path = bot.back_up(kl(2026, 8, 31, 4, 0))

    copy = Repo(path)
    try:
        assert copy.get_run(run) is not None
    finally:
        copy.close()


def test_a_snapshot_is_one_self_contained_file(bot):
    """It inherits WAL mode otherwise, so just opening one to check it leaves
    `-wal`/`-shm` siblings in the backups directory that nothing prunes."""
    path = bot.back_up(kl(2026, 8, 31, 4, 0))

    copy = sqlite3.connect(path)
    try:
        assert copy.execute("PRAGMA journal_mode").fetchone()[0] == "delete"
        copy.execute("SELECT count(*) FROM runs").fetchone()
    finally:
        copy.close()
    assert backups(bot) == [path.name]


def test_only_one_snapshot_a_day_however_often_the_tick_runs(bot):
    bot.back_up(kl(2026, 8, 31, 4, 0))
    for minute in range(1, 6):
        assert bot.back_up(kl(2026, 8, 31, 4, minute)) is None
    assert len(backups(bot)) == 1


def test_the_next_day_gets_its_own(bot):
    bot.back_up(kl(2026, 8, 31, 4, 0))
    bot.back_up(kl(2026, 9, 1, 4, 0))
    assert backups(bot) == ["bot.sqlite.20260831-040000", "bot.sqlite.20260901-040000"]


def test_the_day_is_the_guilds_day_not_utc(bot):
    """00:30 on the 1st in Kuala Lumpur is still the 31st in UTC (+8)."""
    path = bot.back_up(datetime(2026, 8, 31, 16, 30, tzinfo=UTC))
    assert path.name == "bot.sqlite.20260901-003000"
    assert bot.repo.get_config(CFG_LAST_BACKUP) == "2026-09-01"


# --- pruning ----------------------------------------------------------------


def test_only_the_newest_fortnight_is_kept(bot):
    day = kl(2026, 8, 1, 4, 0)
    for _ in range(backup.KEEP + 3):
        bot.back_up(day)
        day += timedelta(days=1)

    kept = backups(bot)
    assert len(kept) == backup.KEEP
    assert kept[0] == "bot.sqlite.20260804-040000"  # the first three are gone


def test_files_the_bot_did_not_write_are_never_pruned(bot):
    """`data/backups` also holds hand-made copies -- including the evidence from
    the night the database was corrupted. Pruning is not a licence to tidy."""
    directory = backup.backup_dir(bot.repo.path)
    directory.mkdir(parents=True, exist_ok=True)
    for name in ("bot.sqlite.corrupt-20260831", "notes.md", "bot.sqlite.bak"):
        (directory / name).write_text("keep me")

    day = kl(2026, 8, 1, 4, 0)
    for _ in range(backup.KEEP + 3):
        bot.back_up(day)
        day += timedelta(days=1)

    assert {"bot.sqlite.corrupt-20260831", "notes.md", "bot.sqlite.bak"} <= set(backups(bot))


def test_pruning_reports_what_it_removed(tmp_path):
    directory = tmp_path / "backups"
    directory.mkdir()
    for day in range(1, 5):
        (directory / f"bot.sqlite.202608{day:02d}-040000").write_text("x")

    removed = backup.prune(directory, keep=2)
    assert [p.name for p in removed] == ["bot.sqlite.20260802-040000", "bot.sqlite.20260801-040000"]


# --- failure is not the tick's problem --------------------------------------


def test_a_failing_backup_does_not_raise(bot, monkeypatch):
    monkeypatch.setattr(
        bot.repo, "backup_to", lambda path: (_ for _ in ()).throw(OSError("read-only fs"))
    )
    assert bot.back_up(kl(2026, 8, 31, 4, 0)) is None
    assert bot.repo.get_config(CFG_LAST_BACKUP) is None


def test_a_failing_backup_is_not_retried_every_thirty_seconds(bot, monkeypatch):
    """It would otherwise log a traceback ~2800 times before midnight."""
    calls: list[str] = []

    def explode(path):
        calls.append(str(path))
        raise OSError("read-only fs")

    monkeypatch.setattr(bot.repo, "backup_to", explode)
    for minute in range(5):
        bot.back_up(kl(2026, 8, 31, 4, minute))
    assert len(calls) == 1


def test_a_failed_day_is_tried_again_the_next_day(bot, monkeypatch):
    monkeypatch.setattr(
        bot.repo, "backup_to", lambda path: (_ for _ in ()).throw(OSError("read-only fs"))
    )
    bot.back_up(kl(2026, 8, 31, 4, 0))
    monkeypatch.undo()

    assert bot.back_up(kl(2026, 9, 1, 4, 0)) is not None


def test_an_in_memory_database_is_skipped_rather_than_failing(repo):
    client = BossBot.__new__(BossBot)
    client.repo = repo
    client.tz = TZ
    client._backup_failed_on = None
    assert client.back_up(kl(2026, 8, 31, 4, 0)) is None
