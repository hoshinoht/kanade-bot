"""Descriptor-anchored traversal, identity checks, and publication."""

from __future__ import annotations

import os
import secrets
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Final, NoReturn

MAINTENANCE_DIRECTORY: Final = "maintenance-upgrades"
_TEMP_ATTEMPTS: Final = 32
_PRIVATE_DIRECTORY_MODE: Final = 0o700
_PRIVATE_FILE_MODE: Final = 0o600
_WRITE_BY_OTHER: Final = stat.S_IWGRP | stat.S_IWOTH
_STICKY: Final = stat.S_ISVTX
Identity = tuple[int, int]
_DIR_FD_SUPPORTED: Final = all(
    function in getattr(os, "supports_dir_fd", ())
    for function in (os.open, os.mkdir, os.stat, os.link, os.unlink)
)
_LINK_FOLLOW_SUPPORTED: Final = os.link in getattr(os, "supports_follow_symlinks", ())


class SnapshotError(RuntimeError):
    """A redacted, typed failure from the upgrade-snapshot boundary."""

    def __init__(self, code: str = "failed") -> None:
        self.code = code
        super().__init__(f"maintenance upgrade snapshot failed ({code})")


@dataclass(frozen=True)
class MaintenanceDirectory:
    fd: int
    identity: Identity
    created: bool


@dataclass(frozen=True)
class TemporaryFile:
    name: str
    final_name: str
    fd: int
    identity: Identity


def fail(code: str) -> NoReturn:
    """Raise a path-redacted snapshot error without preserving OS text."""
    raise SnapshotError(code) from None


def require_platform() -> None:
    if (
        not hasattr(os, "O_DIRECTORY")
        or not hasattr(os, "O_NOFOLLOW")
        or not hasattr(os, "getuid")
        or not _DIR_FD_SUPPORTED
        or not _LINK_FOLLOW_SUPPORTED
    ):
        fail("unsupported-platform")


def source_paths(db_path: str | Path) -> tuple[Path, Path]:
    try:
        if str(db_path) == ":memory:":
            fail("unsupported-source")
        source = Path(db_path)
        if not source.is_absolute():
            source = Path.cwd() / source
        if any(part == ".." for part in source.parts) or not source.name:
            fail("unsafe-parent")
        source = _canonicalize_standard_alias(source)
        return source, source.parent
    except SnapshotError:
        raise
    except (OSError, TypeError, ValueError):
        fail("unsafe-parent")


def _canonicalize_standard_alias(source: Path) -> Path:
    """Map only Darwin's trusted root aliases before no-follow traversal."""
    if len(source.parts) < 2 or source.parts[0] != "/":
        return source
    alias = source.parts[1]
    if alias not in {"tmp", "var"}:
        return source
    alias_path = Path("/") / alias
    try:
        alias_info = os.lstat(alias_path)
        target = os.readlink(alias_path)
    except OSError:
        return source
    if (
        not stat.S_ISLNK(alias_info.st_mode)
        or alias_info.st_uid != 0
        or target not in {f"private/{alias}", f"/private/{alias}"}
    ):
        fail("unsafe-parent")
    return Path("/private") / alias / Path(*source.parts[2:])


def open_source_parent(path: Path) -> int:
    descriptor = _open_directory("/", None, "unsafe-parent")
    try:
        components = path.parts[1:]
        for index, component in enumerate(components):
            next_descriptor = _open_directory(component, descriptor, "unsafe-parent")
            close_quietly(descriptor)
            descriptor = next_descriptor
            _validate_parent_component(
                descriptor,
                final=index == len(components) - 1,
            )
        if not components:
            _validate_parent_component(descriptor, final=True)
        return descriptor
    except BaseException:
        close_quietly(descriptor)
        raise


def _validate_parent_component(descriptor: int, *, final: bool) -> None:
    try:
        info = os.fstat(descriptor)
    except OSError:
        fail("unsafe-parent")
    if not stat.S_ISDIR(info.st_mode):
        fail("unsafe-parent")
    if final:
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & _WRITE_BY_OTHER:
            fail("unsafe-parent")
        return

    # Root-owned path infrastructure (including a platform's standard sticky
    # temporary root) is trusted for traversal. User-owned ancestors must be
    # private enough that another UID cannot replace a later component.
    if info.st_uid not in {os.getuid(), 0}:
        fail("unsafe-parent")
    if info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) & _WRITE_BY_OTHER:
        fail("unsafe-parent")
    if info.st_uid == 0 and stat.S_IMODE(info.st_mode) & _WRITE_BY_OTHER:
        if not info.st_mode & _STICKY:
            fail("unsafe-parent")


def _open_directory(name: str, dir_fd: int | None, code: str) -> int:
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    try:
        return os.open(name, flags, dir_fd=dir_fd)
    except OSError:
        fail(code)


def open_maintenance_directory(parent_fd: int) -> MaintenanceDirectory:
    created = False
    try:
        os.mkdir(
            MAINTENANCE_DIRECTORY,
            _PRIVATE_DIRECTORY_MODE,
            dir_fd=parent_fd,
        )
        created = True
    except FileExistsError:
        pass
    except OSError:
        fail("unsafe-directory")

    descriptor = _open_directory(MAINTENANCE_DIRECTORY, parent_fd, "unsafe-directory")
    try:
        identity = checked_private_directory(descriptor)
        return MaintenanceDirectory(descriptor, identity, created)
    except BaseException:
        close_quietly(descriptor)
        raise


def sync_parent_directory(parent_fd: int) -> None:
    """Durably record a newly created maintenance directory entry."""
    try:
        os.fsync(parent_fd)
    except OSError:
        fail("directory-sync-failed")


def checked_private_directory(descriptor: int) -> Identity:
    try:
        info = os.fstat(descriptor)
    except OSError:
        fail("unsafe-directory")
    if (
        not stat.S_ISDIR(info.st_mode)
        or info.st_uid != os.getuid()
        or stat.S_IMODE(info.st_mode) != _PRIVATE_DIRECTORY_MODE
    ):
        fail("unsafe-directory")
    return info.st_dev, info.st_ino


def reserve_temporary(parent_fd: int, source_name: str) -> TemporaryFile:
    """Reserve a private backup path with a schema-version-neutral suffix."""
    flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    for _ in range(_TEMP_ATTEMPTS):
        token = secrets.token_hex(16)
        final_name = f"{source_name}.pre-upgrade-{token}.sqlite"
        temporary_name = f".{final_name}.tmp"
        try:
            descriptor = os.open(
                temporary_name,
                flags,
                _PRIVATE_FILE_MODE,
                dir_fd=parent_fd,
            )
        except FileExistsError:
            continue
        except OSError:
            fail("backup-failed")
        try:
            identity = checked_private_file(descriptor, "backup-failed")
            return TemporaryFile(temporary_name, final_name, descriptor, identity)
        except BaseException:
            close_quietly(descriptor)
            raise
    fail("collision")


def publish(
    directory_fd: int,
    temporary_name: str,
    final_name: str,
    temporary_identity: Identity,
) -> None:
    """No-replace publish and directory sync for one known temporary inode."""
    final_linked = False
    try:
        try:
            os.link(
                temporary_name,
                final_name,
                src_dir_fd=directory_fd,
                dst_dir_fd=directory_fd,
                follow_symlinks=False,
            )
        except FileExistsError:
            fail("collision")
        except OSError:
            fail("publication-failed")
        final_linked = True
        verify_entry(
            directory_fd,
            final_name,
            temporary_identity,
            _PRIVATE_FILE_MODE,
            "publication-failed",
        )
        if not unlink_known(directory_fd, temporary_name, temporary_identity):
            fail("publication-failed")
        try:
            os.fsync(directory_fd)
        except OSError:
            fail("publication-failed")
    except SnapshotError:
        if final_linked:
            unlink_known(directory_fd, final_name, temporary_identity)
        raise


def verify_layout(
    parent_path: Path,
    expected_parent: Identity,
    expected_maintenance: Identity,
    parent_fd: int,
    maintenance_fd: int,
) -> None:
    if checked_identity(parent_fd, "unsafe-parent") != expected_parent:
        fail("unsafe-parent")
    if checked_private_directory(maintenance_fd) != expected_maintenance:
        fail("unsafe-directory")

    current_parent = open_source_parent(parent_path)
    current_maintenance = -1
    try:
        if checked_identity(current_parent, "unsafe-parent") != expected_parent:
            fail("unsafe-parent")
        current_maintenance = _open_directory(
            MAINTENANCE_DIRECTORY,
            current_parent,
            "unsafe-directory",
        )
        if checked_private_directory(current_maintenance) != expected_maintenance:
            fail("unsafe-directory")
    finally:
        close_quietly(current_maintenance)
        close_quietly(current_parent)


def checked_identity(descriptor: int, code: str) -> Identity:
    try:
        info = os.fstat(descriptor)
    except OSError:
        fail(code)
    return info.st_dev, info.st_ino


def checked_private_file(descriptor: int, code: str) -> Identity:
    try:
        info = os.fstat(descriptor)
    except OSError:
        fail(code)
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid != os.getuid()
        or stat.S_IMODE(info.st_mode) != _PRIVATE_FILE_MODE
    ):
        fail(code)
    return info.st_dev, info.st_ino


def verify_entry(
    directory_fd: int,
    name: str,
    expected: Identity,
    mode: int,
    code: str,
) -> None:
    try:
        info = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
    except OSError:
        fail(code)
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid != os.getuid()
        or stat.S_IMODE(info.st_mode) != mode
        or (info.st_dev, info.st_ino) != expected
    ):
        fail(code)


def unlink_known(directory_fd: int, name: str, expected: Identity) -> bool:
    if directory_fd < 0:
        return False
    try:
        info = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
    except FileNotFoundError:
        return True
    except OSError:
        return False
    if (info.st_dev, info.st_ino) != expected or not stat.S_ISREG(info.st_mode):
        return False
    try:
        os.unlink(name, dir_fd=directory_fd)
    except FileNotFoundError:
        return True
    except OSError:
        return False
    try:
        os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
    except FileNotFoundError:
        return True
    except OSError:
        return False
    # A replacement appeared after the unlink; do not attempt to remove it.
    return False


def close_quietly(descriptor: int) -> None:
    if descriptor >= 0:
        try:
            os.close(descriptor)
        except OSError:
            pass
