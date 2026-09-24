"""Repository ownership and process-bound SQLite behavior."""

from __future__ import annotations

import asyncio
import os
import sqlite3
import subprocess
import sys
import time
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest

import bot.infrastructure.db as db_module
from bot.health import check as health_check
from bot.infrastructure.db import SCHEMA_VERSION, Repo
from bot.infrastructure.maintenance import ownership

pytestmark = pytest.mark.skipif(
    ownership.fcntl is None or not hasattr(ownership.fcntl, "flock"),
    reason="repository ownership requires flock",
)


def _lock_path(database: Path, root: Path) -> Path:
    info = database.stat()
    return root / ownership.lock_filename((info.st_dev, info.st_ino))


def _subprocess_env() -> dict[str, str]:
    env = os.environ.copy()
    source_root = str(Path(__file__).resolve().parents[1])
    env["PYTHONPATH"] = os.pathsep.join(
        part for part in (source_root, env.get("PYTHONPATH")) if part
    )
    return env


def _wait_for_marker(marker: Path, process: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 5
    while not marker.exists():
        if process.poll() is not None:
            raise AssertionError(f"owner subprocess exited with {process.returncode}")
        if time.monotonic() >= deadline:
            process.kill()
            raise AssertionError("owner subprocess did not signal readiness")
        time.sleep(0.01)


def test_persistent_repo_requires_a_private_root_before_creating_the_database(tmp_path):
    database = tmp_path / "new" / "fresh.sqlite"
    missing_root = tmp_path / "missing-lock-root"

    with pytest.raises(RuntimeError, match="ownership unavailable") as failure:
        Repo(database, owner_lock_dir=missing_root)

    assert not database.exists()
    assert not database.parent.exists()
    assert str(tmp_path) not in str(failure.value)


def test_memory_repo_is_independent_of_the_owner_root():
    repo = Repo(":memory:")
    try:
        repo.set_config("works", "1")
        assert repo.get_config("works") == "1"
    finally:
        repo.close()


def test_owner_root_requires_absolute_private_existing_directory(tmp_path, owner_lock_dir):
    database = tmp_path / "database.sqlite"
    file_root = tmp_path / "file-root"
    file_root.write_text("not a directory")
    insecure_root = tmp_path / "insecure-root"
    insecure_root.mkdir()
    insecure_root.chmod(0o755)
    symlink_root = tmp_path / "symlink-root"
    symlink_root.symlink_to(owner_lock_dir, target_is_directory=True)
    fifo_root = tmp_path / "fifo-root"
    os.mkfifo(fifo_root)

    for bad_root in (
        tmp_path / "does-not-exist",
        Path("relative-owner-root"),
        file_root,
        insecure_root,
        symlink_root,
        fifo_root,
    ):
        with pytest.raises(RuntimeError, match="ownership unavailable") as failure:
            Repo(database, owner_lock_dir=bad_root)
        assert not database.exists()
        assert str(tmp_path) not in str(failure.value)


def test_owner_root_owner_mismatch_is_rejected(tmp_path, owner_lock_dir, monkeypatch):
    database = tmp_path / "database.sqlite"
    uid = os.getuid()
    monkeypatch.setattr(ownership.os, "getuid", lambda: uid + 1)

    with pytest.raises(RuntimeError, match="ownership unavailable"):
        Repo(database, owner_lock_dir=owner_lock_dir)
    assert not database.exists()


@pytest.mark.parametrize("node", ["symlink", "mode", "nonregular", "hardlink"])
def test_lockfile_nodes_are_private_single_link_regular_files(tmp_path, owner_lock_dir, node):
    database = tmp_path / f"{node}.sqlite"
    repo = Repo(database, owner_lock_dir=owner_lock_dir)
    repo.close()
    lock = _lock_path(database, owner_lock_dir)

    if node == "symlink":
        target = tmp_path / "lock-target"
        target.write_text("target")
        lock.unlink()
        lock.symlink_to(target)
    elif node == "mode":
        lock.chmod(0o640)
    elif node == "nonregular":
        lock.unlink()
        os.mkfifo(lock)
    else:
        alias = tmp_path / "lock-hardlink"
        alias.hardlink_to(lock)

    with pytest.raises(RuntimeError, match="ownership unavailable"):
        Repo(database, owner_lock_dir=owner_lock_dir)


def test_database_aliases_and_unrelated_readers_contend_without_database_inode_locks(
    tmp_path, owner_lock_dir
):
    database = tmp_path / "database.sqlite"
    repo = Repo(database, owner_lock_dir=owner_lock_dir)
    symlink = tmp_path / "symlink.sqlite"
    symlink.symlink_to(database)
    hardlink = tmp_path / "hardlink.sqlite"
    hardlink.hardlink_to(database)
    relative = Path(os.path.relpath(database, Path.cwd()))

    try:
        raw_fd = os.open(database, os.O_RDONLY)
        reader = sqlite3.connect(database)
        try:
            reader.execute("SELECT 1").fetchone()
        finally:
            reader.close()
            os.close(raw_fd)

        for alias in (database.absolute(), relative, symlink, hardlink):
            with pytest.raises(RuntimeError, match="ownership unavailable"):
                Repo(alias, owner_lock_dir=owner_lock_dir)

        repo.set_config("after_reader_close", "1")
        assert repo.get_config("after_reader_close") == "1"
    finally:
        repo.close()

    for alias in (database.absolute(), relative, symlink, hardlink):
        reopened = Repo(alias, owner_lock_dir=owner_lock_dir)
        reopened.close()


def test_repo_probe_preserves_reserved_characters_in_existing_database_path(
    tmp_path, owner_lock_dir
):
    database = tmp_path / "repo?version#100%.sqlite"
    repo = Repo(database, owner_lock_dir=owner_lock_dir)
    repo.close()

    reopened = Repo(database, owner_lock_dir=owner_lock_dir)
    try:
        assert (
            reopened._conn.execute("SELECT version FROM schema_version").fetchone()[0]
            == SCHEMA_VERSION
        )
    finally:
        reopened.close()
    assert not (tmp_path / "repo").exists()


def test_a_separate_process_remains_blocked_after_its_readonly_probe_closes(
    tmp_path, owner_lock_dir
):
    database = tmp_path / "database.sqlite"
    marker = tmp_path / "ready"
    uri = database.as_uri() + "?mode=ro"
    source = f"""
import sqlite3
import time
from pathlib import Path
from bot.infrastructure.db import Repo

repo = Repo({str(database)!r}, owner_lock_dir={str(owner_lock_dir)!r})
sqlite3.connect({uri!r}, uri=True).close()
Path({str(marker)!r}).touch()
time.sleep(30)
"""
    process = subprocess.Popen(
        [sys.executable, "-c", source],
        cwd=Path(__file__).resolve().parents[1],
        env=_subprocess_env(),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        _wait_for_marker(marker, process)
        with pytest.raises(RuntimeError, match="ownership unavailable"):
            Repo(database, owner_lock_dir=owner_lock_dir)
    finally:
        process.terminate()
        process.wait(timeout=5)


def test_process_death_releases_the_separate_owner_lock(tmp_path, owner_lock_dir):
    database = tmp_path / "database.sqlite"
    marker = tmp_path / "ready"
    source = f"""
import os
from pathlib import Path
from bot.infrastructure.db import Repo

Repo({str(database)!r}, owner_lock_dir={str(owner_lock_dir)!r})
Path({str(marker)!r}).touch()
os._exit(0)
"""
    process = subprocess.Popen(
        [sys.executable, "-c", source],
        cwd=Path(__file__).resolve().parents[1],
        env=_subprocess_env(),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    _wait_for_marker(marker, process)
    assert process.wait(timeout=5) == 0

    repo = Repo(database, owner_lock_dir=owner_lock_dir)
    repo.close()


def test_constructor_failures_release_owner_fds(tmp_path, owner_lock_dir, monkeypatch):
    database = tmp_path / "database.sqlite"
    connection = sqlite3.connect(database)
    connection.execute("CREATE TABLE schema_version (version INTEGER NOT NULL)")
    connection.execute("INSERT INTO schema_version VALUES (8)")
    connection.commit()
    connection.close()

    with pytest.raises(RuntimeError, match="upgrades from v9 only"):
        Repo(database, owner_lock_dir=owner_lock_dir)
    database.unlink()
    Repo(database, owner_lock_dir=owner_lock_dir).close()

    database.write_bytes(b"not sqlite")
    with pytest.raises(RuntimeError, match="metadata cannot be read"):
        Repo(database, owner_lock_dir=owner_lock_dir)
    database.unlink()
    Repo(database, owner_lock_dir=owner_lock_dir).close()

    original_guard = db_module.WriteGuard

    class FailingGuard:
        def __init__(self, *_args):
            raise RuntimeError("guard construction failed")

    monkeypatch.setattr(db_module, "WriteGuard", FailingGuard)
    with pytest.raises(RuntimeError, match="guard construction failed"):
        Repo(database, owner_lock_dir=owner_lock_dir)
    monkeypatch.setattr(db_module, "WriteGuard", original_guard)
    Repo(database, owner_lock_dir=owner_lock_dir).close()


def test_cursor_factory_cannot_bypass_the_process_guard(tmp_path, owner_lock_dir):
    class CustomCursor(sqlite3.Cursor):
        pass

    repo = Repo(tmp_path / "database.sqlite", owner_lock_dir=owner_lock_dir)
    try:
        for factory in (sqlite3.Cursor, CustomCursor):
            with pytest.raises(ValueError, match="cursor factory"):
                repo._conn.cursor(factory)
            with pytest.raises(ValueError, match="cursor factory"):
                repo._conn.cursor(factory=factory)
        cursor = repo._conn.execute("SELECT 1")
        assert isinstance(cursor, ownership.PIDGuardedCursor)
    finally:
        repo.close()


def test_database_path_replacement_is_denied_and_owner_cleanup_remains_possible(
    tmp_path, owner_lock_dir
):
    database = tmp_path / "database.sqlite"
    replacement = tmp_path / "replacement.sqlite"
    repo = Repo(database, owner_lock_dir=owner_lock_dir)
    sqlite3.connect(replacement).close()
    os.replace(database, replacement)

    try:
        with pytest.raises(RuntimeError, match="database ownership changed"):
            repo.get_config("replaced")
        with pytest.raises(RuntimeError, match="database ownership changed"):
            repo.close()
    finally:
        os.replace(replacement, database)

    reopened = Repo(database, owner_lock_dir=owner_lock_dir)
    reopened.close()


def test_forked_repository_cannot_query_or_close_and_does_not_release_parent_lock(
    tmp_path, owner_lock_dir
):
    database = tmp_path / "database.sqlite"
    repo = Repo(database, owner_lock_dir=owner_lock_dir)
    child = os.fork()
    if child == 0:
        failures = 0
        try:
            repo.get_config("fork")
        except RuntimeError:
            failures += 1
        try:
            repo._conn.execute("SELECT 1")
        except RuntimeError:
            failures += 1
        try:
            repo.close()
        except RuntimeError:
            failures += 1
        descriptors = (
            repo._ownership.identity_fd,
            repo._ownership.lock_fd,
            repo._ownership.root_fd,
        )
        os._exit(0 if failures == 3 and descriptors == (-1, -1, -1) else 1)

    _, status = os.waitpid(child, 0)
    assert os.waitstatus_to_exitcode(status) == 0
    with pytest.raises(RuntimeError, match="ownership unavailable"):
        Repo(database, owner_lock_dir=owner_lock_dir)
    repo.close()
    Repo(database, owner_lock_dir=owner_lock_dir).close()


def test_forked_pending_transaction_rejects_cursor_and_context_methods(tmp_path, owner_lock_dir):
    database = tmp_path / "database.sqlite"
    repo = Repo(database, owner_lock_dir=owner_lock_dir)
    repo._conn.execute("BEGIN IMMEDIATE")
    cursor = repo._conn.execute("SELECT 1")
    child = os.fork()
    if child == 0:
        failures = 0

        def expect_runtime(action):
            try:
                action()
            except RuntimeError:
                return True
            except BaseException:
                return False
            return False

        def use_context():
            with repo._conn:
                pass

        for action in (
            lambda: cursor.fetchone(),
            lambda: cursor.execute("SELECT 1"),
            lambda: repo._conn.execute("SELECT 1"),
            lambda: repo._conn.__enter__(),
            lambda: repo._conn.__exit__(None, None, None),
            lambda: repo._conn.commit(),
            lambda: repo._conn.rollback(),
            use_context,
        ):
            failures += not expect_runtime(action)
        os._exit(0 if failures == 0 else 1)

    _, status = os.waitpid(child, 0)
    assert os.waitstatus_to_exitcode(status) == 0
    repo._conn.rollback()
    repo.close()
    reopened = Repo(database, owner_lock_dir=owner_lock_dir)
    reopened.close()


def test_wal_write_checkpoint_and_canonical_readonly_health_path_work(tmp_path, owner_lock_dir):
    database = tmp_path / "database.sqlite"
    repo = Repo(database, owner_lock_dir=owner_lock_dir)
    try:
        stamp = datetime.now(UTC)
        repo.heartbeat(stamp)
        repo.set_config("wal_write", "1")
        repo._conn.execute("PRAGMA wal_checkpoint(PASSIVE)").fetchone()
        uri = database.as_uri() + "?mode=ro"
        reader = sqlite3.connect(uri, uri=True)
        try:
            assert reader.execute("PRAGMA journal_mode").fetchone()[0].lower() == "wal"
            assert reader.execute("SELECT value FROM config WHERE key = 'wal_write'").fetchone()[0]
        finally:
            reader.close()
        assert health_check(str(database), max_age=timedelta(minutes=1))[0]
    finally:
        repo.close()


def test_a_live_lease_cannot_be_normalized_by_a_second_repo(tmp_path, owner_lock_dir):
    database = tmp_path / "database.sqlite"
    repo = Repo(database, owner_lock_dir=owner_lock_dir)

    async def scenario():
        async with repo.maintenance.operation("write"):
            with pytest.raises(RuntimeError, match="ownership unavailable"):
                Repo(database, owner_lock_dir=owner_lock_dir)

    try:
        asyncio.run(scenario())
    finally:
        repo.close()
