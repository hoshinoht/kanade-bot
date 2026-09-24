"""Read-only health checks use exact, percent-encoded SQLite paths."""

from __future__ import annotations

import sqlite3
from datetime import UTC, datetime, timedelta

import pytest

from bot.health import check


def _database_with_heartbeat(path):
    connection = sqlite3.connect(path)
    try:
        connection.execute("CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        connection.execute(
            "INSERT INTO config VALUES ('heartbeat', ?)", (datetime.now(UTC).isoformat(),)
        )
        connection.commit()
    finally:
        connection.close()


def test_health_reads_a_database_with_uri_reserved_characters(tmp_path):
    path = tmp_path / "heartbeat?report#100%.sqlite"
    _database_with_heartbeat(path)

    healthy, detail = check(str(path), max_age=timedelta(minutes=1))

    assert healthy
    assert detail.startswith("ok (")
    assert not (tmp_path / "heartbeat").exists()


def test_health_readonly_probe_does_not_create_a_nonexistent_reserved_path(tmp_path):
    path = tmp_path / "missing?report#100%.sqlite"

    healthy, detail = check(str(path))

    assert not healthy
    assert detail.startswith("cannot open")
    assert not path.exists()
    assert list(tmp_path.iterdir()) == []


def _store(path, owner_lock_dir, update=None):
    from bot.infrastructure.db import Repo

    Repo(path, owner_lock_dir=owner_lock_dir).close()
    if update is not None:
        connection = sqlite3.connect(path)
        connection.execute(update)
        connection.commit()
        connection.close()


@pytest.mark.parametrize(
    ("update", "label"),
    [
        ("UPDATE maintenance_state SET mode = 'BLOCKED'", "BLOCKED"),
        ("UPDATE maintenance_state SET mode = 'FROZEN'", "FROZEN"),
        (
            "UPDATE maintenance_state SET adoption_state = 'pending', adoption_id = 'a-1'",
            "OPEN (adoption pending)",
        ),
    ],
)
def test_closed_maintenance_is_healthy_without_a_heartbeat(tmp_path, owner_lock_dir, update, label):
    path = tmp_path / "bot.sqlite"
    _store(path, owner_lock_dir, update)
    before = path.read_bytes()

    healthy, detail = check(str(path))

    assert healthy
    assert detail == f"maintenance {label}"
    assert path.read_bytes() == before


def test_open_store_still_requires_a_fresh_heartbeat(tmp_path, owner_lock_dir):
    path = tmp_path / "bot.sqlite"
    _store(path, owner_lock_dir)

    assert check(str(path)) == (False, "no heartbeat recorded yet")

    connection = sqlite3.connect(path)
    stale = (datetime.now(UTC) - timedelta(minutes=10)).isoformat()
    connection.execute("INSERT INTO config VALUES ('heartbeat', ?)", (stale,))
    connection.commit()
    connection.close()

    healthy, detail = check(str(path))
    assert not healthy
    assert detail.endswith("s old")


def test_closed_health_still_requires_the_api(tmp_path, owner_lock_dir, monkeypatch, capsys):
    from bot import health

    path = tmp_path / "bot.sqlite"
    _store(path, owner_lock_dir, "UPDATE maintenance_state SET mode = 'FROZEN'")
    monkeypatch.setenv("DB_PATH", str(path))
    monkeypatch.setattr(health, "check_api", lambda: (False, "api not answering on 8080: down"))

    assert health.main() == 1
    assert "maintenance FROZEN" in capsys.readouterr().err
