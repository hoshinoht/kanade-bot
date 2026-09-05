"""Supported database creation and the deployment's v9 -> v11 migrations."""

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


def test_a_fresh_database_starts_at_v11_with_reply_styles(tmp_path):
    repo = Repo(tmp_path / "fresh.sqlite")
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 11
    repo.upsert_member(7, "harbour4417", "MY", True)
    assert repo.get_reply_style(7) is None
    repo.close()


def test_v9_migrates_to_v11_without_losing_member_state(tmp_path):
    path = tmp_path / "v9.sqlite"
    v9_database(path)

    repo = Repo(path)

    assert SCHEMA_VERSION == 11
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 11
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


def test_v9_to_v11_is_idempotent(tmp_path):
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


def test_unversioned_existing_database_is_not_mislabeled_v11(tmp_path):
    path = tmp_path / "unversioned.sqlite"
    conn = sqlite3.connect(path)
    conn.execute("CREATE TABLE members (user_id TEXT PRIMARY KEY)")
    conn.close()

    with pytest.raises(RuntimeError, match="has no schema version"):
        Repo(path)

    conn = sqlite3.connect(path)
    columns = [row[1] for row in conn.execute("PRAGMA table_info(members)")]
    assert columns == ["user_id"]
    assert (
        conn.execute(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'"
        ).fetchone()[0]
        == 0
    )
    conn.close()


def test_a_database_from_a_newer_bot_is_refused(tmp_path):
    path = tmp_path / "future.sqlite"
    repo = Repo(path)
    repo._conn.execute("UPDATE schema_version SET version = ?", (SCHEMA_VERSION + 1,))
    repo.close()
    with pytest.raises(RuntimeError, match="newer bot"):
        Repo(path)


def v10_database_with_star_tokens(path) -> None:
    """A v10 database holding the old `NStar`/`HStar` tokens, with full tables."""
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE schema_version (version INTEGER NOT NULL);
        INSERT INTO schema_version VALUES (10);
        CREATE TABLE runs (
            id TEXT PRIMARY KEY,
            fixed_run_id TEXT,
            channel_id TEXT,
            week_start TEXT NOT NULL,
            bosses TEXT NOT NULL,
            datetime TEXT NOT NULL,
            participants TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'planned',
            source TEXT NOT NULL DEFAULT 'fixed',
            created_at TEXT NOT NULL
        );
        INSERT INTO runs VALUES (
            'r1', NULL, '900', '2026-08-27T00:00:00+08:00', '["HStar", "HFA"]',
            '2026-08-31T21:30:00+08:00', '["1"]', 'planned', 'fixed',
            '2026-08-27T00:00:00+08:00'
        );
        INSERT INTO runs VALUES (
            'r2', NULL, '900', '2026-08-27T00:00:00+08:00', '["XKalos"]',
            '2026-09-01T23:00:00+08:00', '["1"]', 'planned', 'fixed',
            '2026-08-27T00:00:00+08:00'
        );
        CREATE TABLE fixed_runs (
            id TEXT PRIMARY KEY,
            owner_id TEXT NOT NULL,
            channel_id TEXT,
            bosses TEXT NOT NULL,
            weekday INTEGER NOT NULL,
            time TEXT NOT NULL,
            participants TEXT NOT NULL,
            note TEXT,
            created_at TEXT NOT NULL
        );
        INSERT INTO fixed_runs VALUES (
            'f1', '1', '900', '["NStar"]', 0, '21:30', '["1"]', NULL,
            '2026-08-27T00:00:00+08:00'
        );
        CREATE TABLE amendments (
            id TEXT PRIMARY KEY,
            week_start TEXT NOT NULL,
            kind TEXT NOT NULL,
            bosses TEXT NOT NULL DEFAULT '[]',
            run_id TEXT,
            new_datetime TEXT,
            participants TEXT NOT NULL DEFAULT '[]',
            status TEXT NOT NULL DEFAULT 'proposed',
            confidence REAL,
            evidence_msg_ids TEXT NOT NULL DEFAULT '[]',
            proposal_message_id TEXT,
            created_at TEXT NOT NULL,
            channel_id TEXT,
            is_question INTEGER NOT NULL DEFAULT 0,
            rsvp TEXT,
            day_ref TEXT,
            time_ref TEXT,
            summary TEXT,
            payload TEXT NOT NULL DEFAULT '{}'
        );
        INSERT INTO amendments VALUES (
            'a1', '2026-08-27T00:00:00+08:00', 'move', '["HStar"]', 'r1', NULL,
            '[]', 'proposed', 0.9, '[]', NULL, '2026-08-30T00:00:00+08:00',
            '900', 0, NULL, NULL, NULL, NULL, '{"bosses": ["HStar", "HFA"]}'
        );
        INSERT INTO amendments VALUES (
            'a2', '2026-08-27T00:00:00+08:00', 'cancel', '["XKalos"]', 'r2', NULL,
            '[]', 'proposed', 0.9, '[]', NULL, '2026-08-30T00:00:00+08:00',
            '900', 0, NULL, NULL, NULL, NULL, '{}'
        );
        """
    )
    conn.commit()
    conn.close()


def test_v10_rewrites_stored_star_tokens_to_maleficstar(tmp_path):
    """Stored `NStar`/`HStar` become `NMaleficStar`/`HMaleficStar`; the rest is untouched."""
    import json

    path = tmp_path / "v10.sqlite"
    v10_database_with_star_tokens(path)

    repo = Repo(path)
    assert repo._conn.execute("SELECT version FROM schema_version").fetchone()[0] == 11

    def bosses(table: str, row_id: str) -> list:
        return json.loads(
            repo._conn.execute(f"SELECT bosses FROM {table} WHERE id = ?", (row_id,)).fetchone()[0]
        )

    assert bosses("runs", "r1") == ["HMaleficStar", "HFA"]
    assert bosses("runs", "r2") == ["XKalos"]
    assert bosses("fixed_runs", "f1") == ["NMaleficStar"]
    assert bosses("amendments", "a1") == ["HMaleficStar"]
    assert json.loads(
        repo._conn.execute("SELECT payload FROM amendments WHERE id = 'a1'").fetchone()[0]
    ) == {"bosses": ["HMaleficStar", "HFA"]}
    assert bosses("amendments", "a2") == ["XKalos"]
    repo.close()


def test_v10_star_rewrite_is_idempotent(tmp_path):
    path = tmp_path / "v10.sqlite"
    v10_database_with_star_tokens(path)
    Repo(path).close()
    Repo(path).close()

    conn = sqlite3.connect(path)
    assert conn.execute("SELECT version FROM schema_version").fetchone()[0] == 11
    assert conn.execute("SELECT bosses FROM runs WHERE id = 'r1'").fetchone()[0] == (
        '["HMaleficStar", "HFA"]'
    )
    conn.close()
