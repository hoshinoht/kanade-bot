"""Security and consistency checks for pre-upgrade SQLite images."""

from __future__ import annotations

import os
import sqlite3
from pathlib import Path

import pytest

import bot.infrastructure.maintenance.snapshots.filesystem as filesystem_module
import bot.infrastructure.maintenance.snapshots.sqlite as sqlite_backup_module
from bot.infrastructure.maintenance.snapshot import SnapshotError, upgrade_snapshot


def _database(parent: Path, name: str = "source.sqlite") -> tuple[sqlite3.Connection, Path]:
    parent.mkdir(parents=True, exist_ok=True)
    path = parent / name
    connection = sqlite3.connect(path)
    connection.execute("PRAGMA journal_mode=WAL")
    connection.execute("CREATE TABLE records (value TEXT NOT NULL)")
    connection.execute("INSERT INTO records VALUES ('before-ddl')")
    connection.commit()
    return connection, path


def _read_rows(path: Path) -> list[tuple[str]]:
    reader = sqlite3.connect(f"{path.as_uri()}?mode=ro", uri=True)
    try:
        return reader.execute("SELECT value FROM records").fetchall()
    finally:
        reader.close()


def _published(directory: Path) -> list[Path]:
    return sorted(path for path in directory.iterdir() if not path.name.startswith("."))


def test_online_backup_is_wal_consistent_source_unchanged_and_private(tmp_path):
    connection, source = _database(tmp_path)
    try:
        source_bytes = source.read_bytes()
        output = upgrade_snapshot(connection, source)

        assert _read_rows(output) == [("before-ddl",)]
        assert output.name.startswith(f"{source.name}.pre-upgrade-")
        assert output.name.endswith(".sqlite")
        assert output.stat().st_mode & 0o777 == 0o600
        assert output.parent.stat().st_mode & 0o777 == 0o700
        assert _published(output.parent) == [output]
        assert source.read_bytes() == source_bytes
        assert connection.execute("PRAGMA journal_mode").fetchone()[0] == "wal"

        reader = sqlite3.connect(f"{output.as_uri()}?mode=ro", uri=True)
        try:
            assert reader.execute("PRAGMA journal_mode").fetchone()[0] == "delete"
            with pytest.raises(sqlite3.OperationalError):
                reader.execute("CREATE TABLE should_not_write (value TEXT)")
        finally:
            reader.close()
    finally:
        connection.close()


def test_repeated_snapshots_are_unique_valid_and_non_pruning(tmp_path):
    connection, source = _database(tmp_path)
    try:
        first = upgrade_snapshot(connection, source)
        second = upgrade_snapshot(connection, source)

        assert first != second
        assert {first, second} == set(_published(first.parent))
        assert _read_rows(first) == [("before-ddl",)]
        assert _read_rows(second) == [("before-ddl",)]
    finally:
        connection.close()


def test_valid_old_image_survives_caller_ddl_failure(tmp_path):
    connection, source = _database(tmp_path)
    try:
        old_image = upgrade_snapshot(connection, source)
        with pytest.raises(RuntimeError, match="simulated DDL failure"):
            raise RuntimeError("simulated DDL failure")
        assert old_image.exists()
        assert _read_rows(old_image) == [("before-ddl",)]
    finally:
        connection.close()


def test_source_parent_symlink_is_rejected_without_leaks(tmp_path):
    real_parent = tmp_path / "real"
    connection, source = _database(real_parent)
    alias = tmp_path / "alias"
    alias.symlink_to(real_parent, target_is_directory=True)
    try:
        with pytest.raises(SnapshotError) as failure:
            upgrade_snapshot(connection, alias / source.name)
        assert str(tmp_path) not in str(failure.value)
        assert not (real_parent / "maintenance-upgrades").exists()
    finally:
        connection.close()


def test_maintenance_directory_symlink_is_rejected_without_leaks(tmp_path):
    parent = tmp_path / "data"
    outside = tmp_path / "outside"
    connection, source = _database(parent)
    outside.mkdir()
    destination = parent / "maintenance-upgrades"
    destination.symlink_to(outside, target_is_directory=True)
    try:
        with pytest.raises(SnapshotError):
            upgrade_snapshot(connection, source)
        assert destination.is_symlink()
        assert list(outside.iterdir()) == []
    finally:
        connection.close()


def test_group_writable_source_parent_is_rejected_without_chmod(tmp_path):
    parent = tmp_path / "unsafe-parent"
    connection, source = _database(parent)
    original_mode = 0o770
    os.chmod(parent, original_mode)
    try:
        with pytest.raises(SnapshotError):
            upgrade_snapshot(connection, source)
        assert parent.stat().st_mode & 0o777 == original_mode
        assert not (parent / "maintenance-upgrades").exists()
    finally:
        os.chmod(parent, 0o700)
        connection.close()


def test_existing_unsafe_maintenance_directory_is_rejected_without_chmod(tmp_path):
    parent = tmp_path / "data"
    connection, source = _database(parent)
    destination = parent / "maintenance-upgrades"
    destination.mkdir(mode=0o750)
    try:
        with pytest.raises(SnapshotError):
            upgrade_snapshot(connection, source)
        assert destination.stat().st_mode & 0o777 == 0o750
        assert list(destination.iterdir()) == []
    finally:
        connection.close()


def test_final_name_collision_leaves_existing_file_unchanged(tmp_path, monkeypatch):
    connection, source = _database(tmp_path)
    destination = tmp_path / "maintenance-upgrades"
    destination.mkdir(mode=0o700)
    existing = destination / "source.sqlite.pre-upgrade-collision.sqlite"
    existing.write_bytes(b"keep this collision")
    os.chmod(existing, 0o600)
    monkeypatch.setattr(filesystem_module.secrets, "token_hex", lambda _: "collision")
    try:
        with pytest.raises(SnapshotError) as failure:
            upgrade_snapshot(connection, source)
        assert failure.value.code == "collision"
        assert existing.read_bytes() == b"keep this collision"
        assert _published(destination) == [existing]
    finally:
        connection.close()


def test_failure_before_publication_has_no_advertised_image(tmp_path, monkeypatch):
    connection, source = _database(tmp_path)
    monkeypatch.setattr(
        sqlite_backup_module,
        "validate_readonly_backup",
        lambda *_args: (_ for _ in ()).throw(SnapshotError("injected")),
    )
    try:
        with pytest.raises(SnapshotError):
            upgrade_snapshot(connection, source)
        destination = tmp_path / "maintenance-upgrades"
        assert destination.exists()
        assert _published(destination) == []
        assert list(destination.iterdir()) == []
    finally:
        connection.close()


def test_new_maintenance_directory_parent_sync_precedes_reservation_and_publication(
    tmp_path, monkeypatch
):
    connection, source = _database(tmp_path)
    events: list[str] = []
    real_sync = filesystem_module.sync_parent_directory
    real_reserve = filesystem_module.reserve_temporary
    real_publish = filesystem_module.publish

    def sync_parent(parent_fd):
        events.append("parent-sync")
        return real_sync(parent_fd)

    def reserve(*args, **kwargs):
        events.append("reserve")
        return real_reserve(*args, **kwargs)

    def publish(*args, **kwargs):
        events.append("publish")
        return real_publish(*args, **kwargs)

    monkeypatch.setattr(filesystem_module, "sync_parent_directory", sync_parent)
    monkeypatch.setattr(filesystem_module, "reserve_temporary", reserve)
    monkeypatch.setattr(filesystem_module, "publish", publish)
    try:
        output = upgrade_snapshot(connection, source)
        assert output.exists()
        assert events[:3] == ["parent-sync", "reserve", "publish"]
    finally:
        connection.close()


def test_new_maintenance_parent_sync_failure_stops_before_any_snapshot_file(tmp_path, monkeypatch):
    connection, source = _database(tmp_path)
    outside = tmp_path / "outside"
    outside.mkdir()
    outside_marker = outside / "untouched"
    outside_marker.write_text("keep")

    def fail_parent_sync(_parent_fd):
        raise SnapshotError("directory-sync-failed")

    def fail_if_backup_called(*_args, **_kwargs):
        pytest.fail("SQLite backup started before the new directory was durable")

    monkeypatch.setattr(filesystem_module, "sync_parent_directory", fail_parent_sync)
    monkeypatch.setattr(sqlite_backup_module, "write_online_backup", fail_if_backup_called)
    try:
        with pytest.raises(SnapshotError) as failure:
            upgrade_snapshot(connection, source)
        destination = tmp_path / "maintenance-upgrades"
        assert failure.value.code == "directory-sync-failed"
        assert destination.exists()
        assert list(destination.iterdir()) == []
        assert _published(destination) == []
        assert outside_marker.read_text() == "keep"
    finally:
        connection.close()


def test_retry_syncs_existing_directory_after_parent_sync_failure(tmp_path, monkeypatch):
    connection, source = _database(tmp_path)
    real_sync = filesystem_module.sync_parent_directory
    attempts = 0

    def fail_once(parent_fd):
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise SnapshotError("directory-sync-failed")
        return real_sync(parent_fd)

    monkeypatch.setattr(filesystem_module, "sync_parent_directory", fail_once)
    try:
        with pytest.raises(SnapshotError, match="directory-sync-failed"):
            upgrade_snapshot(connection, source)
        destination = tmp_path / "maintenance-upgrades"
        assert destination.is_dir()
        assert list(destination.iterdir()) == []
        output = upgrade_snapshot(connection, source)
        assert attempts == 2
        assert _read_rows(output) == [("before-ddl",)]
    finally:
        connection.close()


def test_pathname_replacement_after_sqlite_open_is_detected_and_not_deleted(tmp_path, monkeypatch):
    connection, source = _database(tmp_path)
    real_connect = sqlite_backup_module.sqlite3.connect
    replaced: dict[str, Path] = {}

    def replace_after_open(database, *args, **kwargs):
        if not kwargs.get("uri") and not replaced:
            path = Path(database)
            if path.parent.name == "maintenance-upgrades":
                path.unlink()
                replacement = real_connect(path)
                replacement.close()
                os.chmod(path, 0o600)
                replaced["path"] = path
        return real_connect(database, *args, **kwargs)

    monkeypatch.setattr(sqlite_backup_module.sqlite3, "connect", replace_after_open)
    try:
        with pytest.raises(SnapshotError) as failure:
            upgrade_snapshot(connection, source)
        assert failure.value.code == "backup-failed"
        assert replaced["path"].exists()
        assert _published(replaced["path"].parent) == []
        assert str(tmp_path) not in str(failure.value)
    finally:
        connection.close()


def test_inode_replacement_after_backup_is_not_deleted(tmp_path, monkeypatch):
    connection, source = _database(tmp_path)
    real_verify = filesystem_module.verify_entry
    calls = 0
    replacement: dict[str, Path] = {}

    def replace_after_backup(directory_fd, name, expected, mode, code):
        nonlocal calls
        calls += 1
        real_verify(directory_fd, name, expected, mode, code)
        if calls == 3:
            filesystem_module.os.unlink(name, dir_fd=directory_fd)
            descriptor = filesystem_module.os.open(
                name,
                filesystem_module.os.O_WRONLY
                | filesystem_module.os.O_CREAT
                | filesystem_module.os.O_EXCL
                | filesystem_module.os.O_NOFOLLOW,
                0o600,
                dir_fd=directory_fd,
            )
            try:
                filesystem_module.os.write(descriptor, b"unknown replacement")
            finally:
                filesystem_module.os.close(descriptor)
            replacement["path"] = tmp_path / "maintenance-upgrades" / name

    monkeypatch.setattr(filesystem_module, "verify_entry", replace_after_backup)
    try:
        with pytest.raises(SnapshotError) as failure:
            upgrade_snapshot(connection, source)
        assert failure.value.code == "backup-failed"
        assert replacement["path"].read_bytes() == b"unknown replacement"
        assert _published(replacement["path"].parent) == []
    finally:
        connection.close()


def test_directory_replacement_is_detected_without_deleting_replacement(tmp_path, monkeypatch):
    parent = tmp_path / "data"
    connection, source = _database(parent)
    real_verify = filesystem_module.verify_layout
    calls = 0
    moved = tmp_path / "moved-data"

    def replace_before_final_check(*args):
        nonlocal calls
        calls += 1
        if calls == 3:
            parent.rename(moved)
            parent.mkdir(mode=0o700)
        return real_verify(*args)

    monkeypatch.setattr(filesystem_module, "verify_layout", replace_before_final_check)
    try:
        with pytest.raises(SnapshotError) as failure:
            upgrade_snapshot(connection, source)
        assert failure.value.code == "unsafe-parent"
        assert parent.exists()
        assert list(parent.iterdir()) == []
        assert moved.exists()
    finally:
        connection.close()


def test_temporary_descriptor_remains_open_through_sqlite_open_and_publish(tmp_path, monkeypatch):
    connection, source = _database(tmp_path)
    real_connect = sqlite_backup_module.sqlite3.connect
    real_reserve = filesystem_module.reserve_temporary
    real_verify = filesystem_module.verify_entry
    retained: dict[str, int | bool] = {}

    def reserve(*args, **kwargs):
        temporary = real_reserve(*args, **kwargs)
        retained["fd"] = temporary.fd
        return temporary

    def check_connect(database, *args, **kwargs):
        if not kwargs.get("uri") and "maintenance-upgrades" in str(database):
            os.fstat(retained["fd"])
            retained["at_connect"] = True
        return real_connect(database, *args, **kwargs)

    def check_verify(directory_fd, name, expected, mode, code):
        if not name.startswith("."):
            os.fstat(retained["fd"])
            retained["at_publish"] = True
        return real_verify(directory_fd, name, expected, mode, code)

    monkeypatch.setattr(filesystem_module, "reserve_temporary", reserve)
    monkeypatch.setattr(sqlite_backup_module.sqlite3, "connect", check_connect)
    monkeypatch.setattr(filesystem_module, "verify_entry", check_verify)
    try:
        output = upgrade_snapshot(connection, source)
        assert output.exists()
        assert retained["at_connect"] is True
        assert retained["at_publish"] is True
    finally:
        connection.close()
