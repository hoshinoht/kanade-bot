"""SQLite online-backup and read-only validation primitives."""

from __future__ import annotations

import os
import sqlite3
from pathlib import Path

from . import filesystem as fs


def write_online_backup(
    source: sqlite3.Connection,
    temporary_path: Path,
    temporary_fd: int,
    directory_fd: int,
    temporary_name: str,
) -> None:
    expected = fs.checked_private_file(temporary_fd, "backup-failed")
    fs.verify_entry(
        directory_fd,
        temporary_name,
        expected,
        0o600,
        "backup-failed",
    )
    destination: sqlite3.Connection | None = None
    try:
        destination = sqlite3.connect(str(temporary_path))
        # This check is deliberately immediately after SQLite's pathname open.
        fs.verify_entry(
            directory_fd,
            temporary_name,
            fs.checked_private_file(temporary_fd, "backup-failed"),
            0o600,
            "backup-failed",
        )
        with destination:
            source.backup(destination)
        fs.verify_entry(
            directory_fd,
            temporary_name,
            fs.checked_private_file(temporary_fd, "backup-failed"),
            0o600,
            "backup-failed",
        )
        journal_mode = destination.execute("PRAGMA journal_mode=DELETE").fetchone()
        if not journal_mode or str(journal_mode[0]).lower() != "delete":
            fs.fail("backup-failed")
        destination.commit()
        fs.verify_entry(
            directory_fd,
            temporary_name,
            fs.checked_private_file(temporary_fd, "backup-failed"),
            0o600,
            "backup-failed",
        )
    finally:
        if destination is not None:
            destination.close()

    fs.verify_entry(
        directory_fd,
        temporary_name,
        fs.checked_private_file(temporary_fd, "backup-failed"),
        0o600,
        "backup-failed",
    )
    try:
        os.fsync(temporary_fd)
    except OSError:
        fs.fail("backup-failed")
    fs.verify_entry(
        directory_fd,
        temporary_name,
        fs.checked_private_file(temporary_fd, "backup-failed"),
        0o600,
        "backup-failed",
    )


def validate_readonly_backup(
    temporary_path: Path,
    temporary_fd: int,
    directory_fd: int,
    temporary_name: str,
) -> None:
    expected = fs.checked_private_file(temporary_fd, "backup-failed")
    fs.verify_entry(directory_fd, temporary_name, expected, 0o600, "backup-failed")
    reader: sqlite3.Connection | None = None
    try:
        reader = sqlite3.connect(f"{temporary_path.as_uri()}?mode=ro", uri=True)
        # This check is deliberately immediately after the read-only pathname open.
        fs.verify_entry(directory_fd, temporary_name, expected, 0o600, "backup-failed")
        journal_mode = reader.execute("PRAGMA journal_mode").fetchone()
        integrity = reader.execute("PRAGMA integrity_check").fetchone()
        if (
            not journal_mode
            or str(journal_mode[0]).lower() != "delete"
            or not integrity
            or str(integrity[0]).lower() != "ok"
        ):
            fs.fail("backup-failed")
    finally:
        if reader is not None:
            reader.close()
    fs.verify_entry(directory_fd, temporary_name, expected, 0o600, "backup-failed")
