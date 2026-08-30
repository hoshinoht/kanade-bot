"""Schema migrations, and specifically v1 (integer ids) -> v2 (uuid4)."""

from __future__ import annotations

import sqlite3

import pytest

from bot.db import SCHEMA_VERSION, Repo

# The v1 shape, trimmed to what the migration has to reason about.
V1_SQL = """
CREATE TABLE schema_version (version INTEGER NOT NULL);
CREATE TABLE members (
    user_id TEXT PRIMARY KEY, display_name TEXT NOT NULL DEFAULT '', nickname TEXT,
    aliases TEXT NOT NULL DEFAULT '[]', has_role INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);
CREATE TABLE fixed_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT, owner_id TEXT NOT NULL, channel_id TEXT,
    bosses TEXT NOT NULL, weekday INTEGER NOT NULL, time TEXT NOT NULL,
    participants TEXT NOT NULL, note TEXT, created_at TEXT NOT NULL
);
CREATE TABLE runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT, fixed_run_id INTEGER, channel_id TEXT,
    week_start TEXT NOT NULL, bosses TEXT NOT NULL, datetime TEXT NOT NULL,
    participants TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'planned',
    source TEXT NOT NULL DEFAULT 'fixed', created_at TEXT NOT NULL
);
CREATE TABLE rsvps (
    run_id INTEGER NOT NULL, user_id TEXT NOT NULL, state TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'reaction', at TEXT NOT NULL,
    PRIMARY KEY (run_id, user_id)
);
CREATE TABLE reminders (
    id INTEGER PRIMARY KEY AUTOINCREMENT, run_id INTEGER NOT NULL, fire_at TEXT NOT NULL,
    kind TEXT NOT NULL, sent_at TEXT, message_id TEXT, UNIQUE (run_id, kind)
);
CREATE TABLE messages (
    id TEXT PRIMARY KEY, channel_id TEXT NOT NULL, author_id TEXT NOT NULL,
    created_at TEXT NOT NULL, content TEXT NOT NULL, processed_at TEXT
);
CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"""


@pytest.fixture
def v1_db(tmp_path):
    """A populated v1 database on disk."""
    path = tmp_path / "v1.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(V1_SQL)
    conn.execute("INSERT INTO schema_version (version) VALUES (1)")
    conn.execute(
        "INSERT INTO members VALUES ('7', 'harbour4417', 'MY', '[\"MY\"]', 1,"
        " '2026-08-30T00:00:00+00:00')"
    )
    conn.execute(
        "INSERT INTO messages VALUES ('55', '900', '7', '2026-08-30T00:00:00+00:00', 'hi', NULL)"
    )
    conn.execute("INSERT INTO config VALUES ('day_of_ping_time', '08:30')")
    conn.execute(
        "INSERT INTO fixed_runs (owner_id, channel_id, bosses, weekday, time, participants,"
        " created_at) VALUES ('7', '900', '[\"HStar\"]', 0, '21:30', '[\"7\"]', 'x')"
    )
    conn.execute(
        "INSERT INTO runs (fixed_run_id, channel_id, week_start, bosses, datetime, participants,"
        " created_at) VALUES (1, '900', 'w', '[\"HStar\"]',"
        " '2026-08-31T13:30:00+00:00', '[\"7\"]', 'x')"
    )
    conn.execute("INSERT INTO reminders (run_id, fire_at, kind) VALUES (1, 'x', 'day_of')")
    conn.execute("INSERT INTO rsvps VALUES (1, '7', 'yes', 'reaction', 'x')")
    conn.commit()
    conn.close()
    return path


def test_opening_a_v1_database_migrates_it(v1_db):
    repo = Repo(v1_db)
    version = repo._conn.execute("SELECT version FROM schema_version").fetchone()["version"]
    assert version == SCHEMA_VERSION == 2
    repo.close()


def test_real_state_is_preserved(v1_db):
    repo = Repo(v1_db)
    member = repo.get_member("7")
    assert member["display_name"] == "harbour4417"
    assert member["aliases"] == ["MY"]
    assert member["has_role"] is True
    assert repo._conn.execute("SELECT COUNT(*) FROM messages").fetchone()[0] == 1
    # Runtime config the owner changed must survive.
    assert repo.get_config("day_of_ping_time") == "08:30"
    repo.close()


def test_hand_entered_baselines_are_carried_across(v1_db):
    # Fixed runs are configuration someone typed in; losing them would mean
    # re-entering bosses, day, time and participants by hand.
    import uuid

    repo = Repo(v1_db)
    fixed = repo.list_fixed_runs()
    assert len(fixed) == 1
    assert fixed[0]["bosses"] == ["HStar"]
    assert fixed[0]["weekday"] == 0
    assert fixed[0]["time"] == "21:30"
    assert fixed[0]["participants"] == ["7"]
    assert fixed[0]["channel_id"] == "900"
    assert uuid.UUID(fixed[0]["id"]).version == 4  # and it has a new uuid
    repo.close()


def test_derived_rows_are_rebuilt_empty(v1_db):
    # runs/reminders regenerate from the baselines on the next materialisation.
    repo = Repo(v1_db)
    assert repo.list_runs() == []
    assert repo._conn.execute("SELECT COUNT(*) FROM reminders").fetchone()[0] == 0
    assert repo._conn.execute("SELECT COUNT(*) FROM rsvps").fetchone()[0] == 0
    repo.close()


def test_the_carried_baselines_rematerialise(v1_db):
    from datetime import time

    from bot.materialise import materialise_week
    from bot.weeks import current_week_start

    from .conftest import COUNTDOWNS, RESET_TIME, RESET_WEEKDAY, TZ

    repo = Repo(v1_db)
    week = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME)
    created = materialise_week(repo, week, TZ, time(9, 0), COUNTDOWNS)
    assert len(created) == 1
    run = repo.get_run(created[0])
    assert run["bosses"] == ["HStar"]
    assert run["channel_id"] == "900"
    repo.close()


def test_new_rows_get_uuid_ids_after_migrating(v1_db):
    import uuid

    repo = Repo(v1_db)
    fixed_id = repo.add_fixed_run("7", ["HStar"], 0, "21:30", ["7"], channel_id=900)
    assert uuid.UUID(fixed_id).version == 4
    repo.close()


def test_migrating_is_idempotent(v1_db):
    first = Repo(v1_db)
    ids = [f["id"] for f in first.list_fixed_runs()]
    first.close()

    repo = Repo(v1_db)  # second open must be a no-op
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()["version"] == 2
    assert repo.get_member("7") is not None
    assert [f["id"] for f in repo.list_fixed_runs()] == ids  # not duplicated or re-keyed
    repo.close()


def test_a_fresh_database_starts_at_the_latest_version(tmp_path):
    repo = Repo(tmp_path / "fresh.sqlite")
    rows = repo._conn.execute("SELECT version FROM schema_version").fetchall()
    assert [r["version"] for r in rows] == [SCHEMA_VERSION]
    repo.close()


def test_a_database_from_a_newer_bot_is_refused(tmp_path):
    path = tmp_path / "future.sqlite"
    repo = Repo(path)
    repo._conn.execute("UPDATE schema_version SET version = ?", (SCHEMA_VERSION + 1,))
    repo.close()
    with pytest.raises(RuntimeError, match="newer bot"):
        Repo(path)
