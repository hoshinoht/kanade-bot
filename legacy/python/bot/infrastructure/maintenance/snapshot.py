"""Private, non-pruning snapshots used only before an upgrade writes DDL."""

from __future__ import annotations

import sqlite3
from pathlib import Path

from .snapshots import filesystem as fs
from .snapshots import sqlite as sqlite_backup

SnapshotError = fs.SnapshotError


def upgrade_snapshot(conn: sqlite3.Connection, db_path: str | Path) -> Path:
    """Publish one private online-backup image before caller-owned upgrade DDL.

    Published images use a schema-version-neutral ``.pre-upgrade-`` suffix.
    The caller must invoke this while ``conn`` still represents the pre-DDL
    database. This function does not prune older images. Filesystem checks
    exclude symlinked path components and require the source parent to be owned
    by this UID without group/world write bits. On Darwin, the standard
    root-owned ``/var`` and ``/tmp`` aliases are accepted only when they point
    to their exact ``/private`` targets, which are then traversed directly.
    Root-owned ancestor directories are trusted only as traversable path
    infrastructure; a same-UID or root adversary can still race pathname
    checks, which this portable Python/SQLite API cannot close with an FD-only
    SQLite open.
    """
    fs.require_platform()
    source, parent_path = fs.source_paths(db_path)
    parent_fd = -1
    maintenance: fs.MaintenanceDirectory | None = None
    temporary: fs.TemporaryFile | None = None
    published = False
    completed = False

    try:
        parent_fd = fs.open_source_parent(parent_path)
        expected_parent = fs.checked_identity(parent_fd, "unsafe-parent")
        maintenance = fs.open_maintenance_directory(parent_fd)
        # Existing directories may remain after an earlier failed sync.
        # Confirm entry durability before reserving any snapshot file.
        fs.sync_parent_directory(parent_fd)
        expected_maintenance = maintenance.identity
        fs.verify_layout(
            parent_path,
            expected_parent,
            expected_maintenance,
            parent_fd,
            maintenance.fd,
        )

        temporary = fs.reserve_temporary(maintenance.fd, source.name)
        temporary_path = parent_path / fs.MAINTENANCE_DIRECTORY / temporary.name
        sqlite_backup.write_online_backup(
            conn,
            temporary_path,
            temporary.fd,
            maintenance.fd,
            temporary.name,
        )
        sqlite_backup.validate_readonly_backup(
            temporary_path,
            temporary.fd,
            maintenance.fd,
            temporary.name,
        )
        fs.verify_layout(
            parent_path,
            expected_parent,
            expected_maintenance,
            parent_fd,
            maintenance.fd,
        )
        fs.publish(
            maintenance.fd,
            temporary.name,
            temporary.final_name,
            temporary.identity,
        )
        published = True
        fs.verify_layout(
            parent_path,
            expected_parent,
            expected_maintenance,
            parent_fd,
            maintenance.fd,
        )
        completed = True
        return parent_path / fs.MAINTENANCE_DIRECTORY / temporary.final_name
    except fs.SnapshotError:
        raise
    except Exception:
        # Do not expose SQLite or OS exception text: it can contain the live
        # database pathname or private deployment details.
        raise fs.SnapshotError("backup-failed") from None
    finally:
        if not completed:
            if published and maintenance is not None and temporary is not None:
                fs.unlink_known(maintenance.fd, temporary.final_name, temporary.identity)
            if maintenance is not None and temporary is not None:
                fs.unlink_known(maintenance.fd, temporary.name, temporary.identity)
        if temporary is not None:
            fs.close_quietly(temporary.fd)
        if maintenance is not None:
            fs.close_quietly(maintenance.fd)
        fs.close_quietly(parent_fd)
