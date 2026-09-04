"""Supported database creation and the deployment's v9 -> v10 migration."""

from __future__ import annotations

import sqlite3

import pytest

from bot.infrastructure.db import SCHEMA_VERSION, Repo


def v9_database(path) -> None:
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE schema_version (version INTEGER NOT NULL);
        INSERT INTO schema_version VALUES (9);
        CREATE TABLE members (
            user_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL DEFAULT '',
            nickname TEXT,
            aliases TEXT NOT NULL DEFAULT '[]',
            has_role INTEGER NOT NULL DEFAULT 0,
            ping_level TEXT NOT NULL DEFAULT 'essential',
            updated_at TEXT NOT NULL
        );
        INSERT INTO members VALUES (
            '7', 'harbour4417', 'MY', '["MY"]', 1, 'off',
            '2026-08-30T00:00:00+00:00'
        );
        CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO config VALUES ('persona', 'persona.md');
        """
    )
    conn.commit()
    conn.close()


def test_a_fresh_database_starts_at_v10_with_reply_styles(tmp_path):
    repo = Repo(tmp_path / "fresh.sqlite")
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 10
    repo.upsert_member(7, "harbour4417", "MY", True)
    assert repo.get_reply_style(7) is None
    repo.close()


def test_v9_migrates_to_v10_without_losing_member_state(tmp_path):
    path = tmp_path / "v9.sqlite"
    v9_database(path)

    repo = Repo(path)

    assert SCHEMA_VERSION == 10
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 10
    member = repo.get_member(7)
    assert member["display_name"] == "harbour4417"
    assert member["aliases"] == ["MY"]
    assert member["ping_level"] == "off"
    assert member["reply_style"] is None
    assert repo.get_config("persona") == "persona.md"
    repo.close()


def test_reply_style_survives_reopening(tmp_path):
    path = tmp_path / "v9.sqlite"
    v9_database(path)
    repo = Repo(path)
    repo.set_reply_style(7, "concise")
    repo.close()

    reopened = Repo(path)
    assert reopened.get_reply_style(7) == "concise"
    reopened.close()


def test_v9_to_v10_is_idempotent(tmp_path):
    path = tmp_path / "v9.sqlite"
    v9_database(path)
    Repo(path).close()
    Repo(path).close()

    conn = sqlite3.connect(path)
    columns = [row[1] for row in conn.execute("PRAGMA table_info(members)")]
    assert columns.count("reply_style") == 1
    conn.close()


def test_pre_v9_database_is_refused_with_upgrade_direction(tmp_path):
    path = tmp_path / "old.sqlite"
    conn = sqlite3.connect(path)
    conn.executescript(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);"
        "INSERT INTO schema_version VALUES (8);"
    )
    conn.close()

    with pytest.raises(RuntimeError, match="supports upgrades from v9 only"):
        Repo(path)


def test_unversioned_existing_database_is_not_mislabeled_v10(tmp_path):
    path = tmp_path / "unversioned.sqlite"
    conn = sqlite3.connect(path)
    conn.execute("CREATE TABLE members (user_id TEXT PRIMARY KEY)")
    conn.close()

    with pytest.raises(RuntimeError, match="has no schema version"):
        Repo(path)

    conn = sqlite3.connect(path)
    columns = [row[1] for row in conn.execute("PRAGMA table_info(members)")]
    assert columns == ["user_id"]
    assert conn.execute(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'"
    ).fetchone()[0] == 0
    conn.close()


def test_a_database_from_a_newer_bot_is_refused(tmp_path):
    path = tmp_path / "future.sqlite"
    repo = Repo(path)
    repo._conn.execute("UPDATE schema_version SET version = ?", (SCHEMA_VERSION + 1,))
    repo.close()
    with pytest.raises(RuntimeError, match="newer bot"):
        Repo(path)
