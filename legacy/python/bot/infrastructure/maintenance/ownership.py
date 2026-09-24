"""One writable repository owner per local SQLite inode.

The owner lock deliberately lives beside neither SQLite's database inode nor its
WAL files.  SQLite may close an otherwise unrelated descriptor for the database,
and Darwin treats a whole-file ``flock`` on that inode as a database lock.  A
separate, deterministic lockfile avoids both behaviours while the retained
database descriptor supplies the identity check.
"""

from __future__ import annotations

import os
import sqlite3
import stat
import sys
import threading
from dataclasses import dataclass
from pathlib import Path

try:
    import fcntl
except ImportError:  # pragma: no cover - supported production platforms provide flock
    fcntl = None

_Identity = tuple[int, int]
_PRIVATE_DIRECTORY_MODE = 0o700
_PRIVATE_FILE_MODE = 0o600
_OWNED: set[_Identity] = set()
_OWNERS: dict[_Identity, DatabaseOwnership] = {}
_REGISTRY_LOCK = threading.RLock()


class _OwnershipFailure(Exception):
    """An internal failure whose public error must not disclose path details."""


def _close_fd(fd: int) -> None:
    if fd < 0:
        return
    try:
        os.close(fd)
    except OSError:
        pass


def _private_directory_flags() -> int:
    if not hasattr(os, "O_DIRECTORY") or not hasattr(os, "O_NOFOLLOW"):
        raise _OwnershipFailure
    return os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0)


def _checked_private_directory(fd: int) -> None:
    try:
        info = os.fstat(fd)
    except OSError as exc:
        raise _OwnershipFailure from exc
    if (
        not stat.S_ISDIR(info.st_mode)
        or info.st_uid != os.getuid()
        or stat.S_IMODE(info.st_mode) != _PRIVATE_DIRECTORY_MODE
    ):
        raise _OwnershipFailure


def _open_owner_root(path: str | Path) -> int:
    """Open and validate the configured owner root without following components."""
    try:
        candidate = Path(path)
        if not candidate.is_absolute() or any(part == ".." for part in candidate.parts):
            raise _OwnershipFailure
        if sys.platform == "darwin" and candidate.parts[1:2] == ("var",):
            alias = Path("/var")
            if alias.is_symlink() and os.readlink(alias) in {"private/var", "/private/var"}:
                candidate = Path("/private/var") / Path(*candidate.parts[2:])
        descriptor = os.open("/", _private_directory_flags())
        try:
            for component in candidate.parts[1:]:
                next_descriptor = os.open(
                    component,
                    _private_directory_flags(),
                    dir_fd=descriptor,
                )
                _close_fd(descriptor)
                descriptor = next_descriptor
            _checked_private_directory(descriptor)
            return descriptor
        except BaseException:
            _close_fd(descriptor)
            raise
    except _OwnershipFailure:
        raise
    except (OSError, TypeError, ValueError) as exc:
        raise _OwnershipFailure from exc


def lock_filename(identity: _Identity) -> str:
    """Return the stable lockfile name for a database device/inode identity."""
    return f"kanade-{identity[0]:x}-{identity[1]:x}.lock"


def _open_database_identity(path: Path) -> tuple[int, _Identity]:
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_CLOEXEC", 0)
    descriptor = -1
    try:
        descriptor = os.open(path, flags, 0o600)
        info = os.fstat(descriptor)
    except OSError as exc:
        _close_fd(descriptor)
        raise _OwnershipFailure from exc
    if not stat.S_ISREG(info.st_mode):
        _close_fd(descriptor)
        raise _OwnershipFailure
    return descriptor, (info.st_dev, info.st_ino)


def _open_lockfile(root_fd: int, identity: _Identity) -> int:
    flags = os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0)
    descriptor = -1
    try:
        descriptor = os.open(lock_filename(identity), flags, _PRIVATE_FILE_MODE, dir_fd=root_fd)
        info = os.fstat(descriptor)
    except OSError as exc:
        _close_fd(descriptor)
        raise _OwnershipFailure from exc
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid != os.getuid()
        or stat.S_IMODE(info.st_mode) != _PRIVATE_FILE_MODE
        or info.st_nlink != 1
    ):
        _close_fd(descriptor)
        raise _OwnershipFailure
    return descriptor


@dataclass
class DatabaseOwnership:
    identity_fd: int
    lock_fd: int
    root_fd: int
    identity: _Identity
    pid: int
    database_path: str

    @property
    def fd(self) -> int:
        """Compatibility alias for the retained, unlocked database descriptor."""
        return self.identity_fd

    def _close_inherited(self) -> None:
        # Never LOCK_UN in a fork child: the lock belongs to the parent's
        # open-file description.  The child must not retain any owner fd.
        for field in ("lock_fd", "identity_fd", "root_fd"):
            descriptor = getattr(self, field)
            _close_fd(descriptor)
            setattr(self, field, -1)

    def assert_usable(self) -> None:
        if self.pid != os.getpid():
            self._close_inherited()
            raise RuntimeError("repository inherited across fork is unusable")
        if self.identity_fd < 0 or self.lock_fd < 0 or self.root_fd < 0:
            raise RuntimeError("database ownership unavailable")

    def verify_path(self, path: str | Path | None = None) -> None:
        self.assert_usable()
        candidate = self.database_path if path is None else path
        try:
            retained = os.fstat(self.identity_fd)
            current = os.stat(candidate, follow_symlinks=True)
        except OSError as exc:
            raise RuntimeError("database ownership changed") from exc
        if (
            not stat.S_ISREG(retained.st_mode)
            or (retained.st_dev, retained.st_ino) != self.identity
            or (current.st_dev, current.st_ino) != self.identity
        ):
            raise RuntimeError("database ownership changed")

    def close(self) -> None:
        if self.pid != os.getpid():
            self._close_inherited()
            raise RuntimeError("repository inherited across fork is unusable")
        with _REGISTRY_LOCK:
            if self.identity_fd < 0 and self.lock_fd < 0 and self.root_fd < 0:
                return
            try:
                if self.lock_fd >= 0 and fcntl is not None:
                    fcntl.flock(self.lock_fd, fcntl.LOCK_UN)
            except OSError:
                pass
            finally:
                _close_fd(self.lock_fd)
                _close_fd(self.identity_fd)
                _close_fd(self.root_fd)
                self.lock_fd = self.identity_fd = self.root_fd = -1
                if _OWNERS.get(self.identity) is self:
                    _OWNERS.pop(self.identity, None)
                _OWNED.discard(self.identity)


def _after_fork_child() -> None:
    """Drop inherited owner descriptors without touching SQLite connections."""
    global _REGISTRY_LOCK
    for owner in tuple(_OWNERS.values()):
        owner._close_inherited()
    _OWNERS.clear()
    _OWNED.clear()
    _REGISTRY_LOCK = threading.RLock()


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork_child)


def acquire(path: str | Path, owner_lock_dir: str | Path | None) -> DatabaseOwnership | None:
    """Acquire a separate private lock for one persistent database inode."""
    if str(path) == ":memory:":
        return None
    if fcntl is None or not hasattr(fcntl, "flock"):
        raise RuntimeError("database ownership lock unsupported")

    root_fd = identity_fd = lock_fd = -1
    owner: DatabaseOwnership | None = None
    registered = False
    try:
        if owner_lock_dir is None:
            raise _OwnershipFailure
        root_fd = _open_owner_root(owner_lock_dir)
        candidate = Path(path)
        candidate.parent.mkdir(parents=True, exist_ok=True)
        identity_fd, identity = _open_database_identity(candidate)
        with _REGISTRY_LOCK:
            if identity in _OWNED:
                raise _OwnershipFailure
            _checked_private_directory(root_fd)
            lock_fd = _open_lockfile(root_fd, identity)
            try:
                fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as exc:
                raise _OwnershipFailure from exc
            candidate_owner = DatabaseOwnership(
                identity_fd=identity_fd,
                lock_fd=lock_fd,
                root_fd=root_fd,
                identity=identity,
                pid=os.getpid(),
                database_path=str(candidate.absolute()),
            )
            candidate_owner.verify_path(candidate)
            owner = candidate_owner
            _OWNERS[identity] = owner
            _OWNED.add(identity)
            registered = True
        return owner
    except _OwnershipFailure:
        raise RuntimeError("database ownership unavailable") from None
    except (OSError, TypeError, ValueError):
        raise RuntimeError("database ownership unavailable") from None
    finally:
        if not registered:
            if owner is not None:
                owner.close()
            else:
                _close_fd(lock_fd)
                _close_fd(identity_fd)
                _close_fd(root_fd)


class PIDGuardedCursor(sqlite3.Cursor):
    """Cursor that rejects use by a process forked from its repository owner."""

    def bind_process(
        self, pid: int, owner: DatabaseOwnership | None, database_path: str | Path
    ) -> None:
        object.__setattr__(self, "_process_pid", pid)
        object.__setattr__(self, "_owner", owner)
        object.__setattr__(self, "_database_path", str(database_path))

    def _assert_sqlite_use(self, *, verify_path: bool = True) -> None:
        data = object.__getattribute__(self, "__dict__")
        pid = data.get("_process_pid")
        if pid is None:
            return
        owner = data.get("_owner")
        if pid != os.getpid():
            if owner is not None:
                owner.assert_usable()
            raise RuntimeError("repository inherited across fork is unusable")
        if owner is not None:
            owner.assert_usable()
            if verify_path:
                owner.verify_path(data["_database_path"])

    def __getattribute__(self, name: str):  # noqa: ANN001
        if name not in {"_assert_sqlite_use", "bind_process", "__dict__", "__class__"}:
            self._assert_sqlite_use()
        return super().__getattribute__(name)

    def execute(self, sql, parameters=(), /):  # noqa: ANN001
        self._assert_sqlite_use()
        return super().execute(sql, parameters)

    def executemany(self, sql, parameters, /):  # noqa: ANN001
        self._assert_sqlite_use()
        return super().executemany(sql, parameters)

    def executescript(self, sql_script, /):  # noqa: ANN001
        self._assert_sqlite_use()
        return super().executescript(sql_script)

    def fetchone(self):
        self._assert_sqlite_use()
        return super().fetchone()

    def fetchmany(self, size=1, /):
        self._assert_sqlite_use()
        return super().fetchmany(size)

    def fetchall(self):
        self._assert_sqlite_use()
        return super().fetchall()

    def __iter__(self):
        self._assert_sqlite_use()
        return super().__iter__()

    def __next__(self):
        self._assert_sqlite_use()
        return super().__next__()

    def close(self):
        self._assert_sqlite_use(verify_path=False)
        return super().close()


class PIDGuardedConnection(sqlite3.Connection):
    """SQLite connection whose public operations are process-bound."""

    def bind_process(
        self, pid: int, owner: DatabaseOwnership | None, database_path: str | Path
    ) -> None:
        object.__setattr__(self, "_process_pid", pid)
        object.__setattr__(self, "_owner", owner)
        object.__setattr__(self, "_database_path", str(database_path))

    def _assert_sqlite_use(self, *, verify_path: bool = True) -> None:
        data = object.__getattribute__(self, "__dict__")
        pid = data.get("_process_pid")
        if pid is None:
            return
        owner = data.get("_owner")
        if pid != os.getpid():
            if owner is not None:
                owner.assert_usable()
            raise RuntimeError("repository inherited across fork is unusable")
        if owner is not None:
            owner.assert_usable()
            if verify_path:
                owner.verify_path(data["_database_path"])

    def __getattribute__(self, name: str):  # noqa: ANN001
        if name not in {
            "_assert_sqlite_use",
            "bind_process",
            "cursor",
            "__dict__",
            "__class__",
        }:
            self._assert_sqlite_use(verify_path=False)
        return super().__getattribute__(name)

    def cursor(self, factory=None):  # noqa: ANN001
        if factory is not None and factory is not PIDGuardedCursor:
            raise ValueError("cursor factory must be PIDGuardedCursor")
        self._assert_sqlite_use()
        cursor = super().cursor(PIDGuardedCursor)
        if isinstance(cursor, PIDGuardedCursor):
            data = object.__getattribute__(self, "__dict__")
            cursor.bind_process(
                data.get("_process_pid", os.getpid()),
                data.get("_owner"),
                data.get("_database_path", ":memory:"),
            )
        return cursor

    def execute(self, sql, parameters=(), /):  # noqa: ANN001
        self._assert_sqlite_use()
        return self.cursor().execute(sql, parameters)

    def executemany(self, sql, parameters, /):  # noqa: ANN001
        self._assert_sqlite_use()
        return self.cursor().executemany(sql, parameters)

    def executescript(self, sql_script, /):  # noqa: ANN001
        self._assert_sqlite_use()
        return self.cursor().executescript(sql_script)

    def backup(self, target, /, **kwargs):  # noqa: ANN001
        self._assert_sqlite_use()
        return super().backup(target, **kwargs)

    def commit(self):
        self._assert_sqlite_use()
        return super().commit()

    def rollback(self):
        self._assert_sqlite_use()
        return super().rollback()

    def close(self):
        self._assert_sqlite_use(verify_path=False)
        return super().close()

    def __enter__(self):
        self._assert_sqlite_use()
        return super().__enter__()

    def __exit__(self, exc_type, exc_value, traceback):  # noqa: ANN001
        self._assert_sqlite_use()
        return super().__exit__(exc_type, exc_value, traceback)
