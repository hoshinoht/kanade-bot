"""Explicitly approved private archive publication with anchored POSIX operations."""

import errno
import os
import secrets
import stat
from dataclasses import dataclass
from pathlib import Path

from .archive import DEFAULT_LIMITS
from .errors import BundleError

_DIR_FD_SUPPORTED = {os.open, os.link, os.unlink} <= os.supports_dir_fd


@dataclass(frozen=True)
class PrivateOutputApproval:
    root: Path
    approved: bool


def publish(path: Path, data: bytes, *, approval: PrivateOutputApproval) -> Path:
    """Atomically publish one new archive under an approved private root."""
    if not approval.approved or len(data) > DEFAULT_LIMITS.max_input_bytes:
        raise BundleError("output-not-approved")
    _require_safe_platform()
    root = _absolute(approval.root)
    candidate = _absolute(path)
    try:
        relative = candidate.relative_to(root)
    except ValueError as error:
        raise BundleError("unsafe-output-path") from error
    if not relative.parts or any(part in {"", ".", ".."} for part in relative.parts):
        raise BundleError("unsafe-output-path")
    root_fd = _open_approved_root(root)
    parent_fd = root_fd
    temporary: str | None = None
    try:
        for part in relative.parts[:-1]:
            next_fd = _open_directory(part, dir_fd=parent_fd, private=True)
            if parent_fd != root_fd:
                os.close(parent_fd)
            parent_fd = next_fd
        target = relative.name
        temporary = f".{target}.tmp-{secrets.token_hex(16)}"
        descriptor = os.open(
            temporary,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | _no_follow_flag(),
            0o600,
            dir_fd=parent_fd,
        )
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
            os.fchmod(stream.fileno(), 0o600)
        try:
            # link(2) creates the final name only if it does not already exist.
            os.link(temporary, target, src_dir_fd=parent_fd, dst_dir_fd=parent_fd)
        except FileExistsError as error:
            raise BundleError("output-exists") from error
        os.unlink(temporary, dir_fd=parent_fd)
        temporary = None
        return candidate
    except BundleError:
        raise
    except OSError as error:
        if error.errno == errno.EEXIST:
            raise BundleError("output-exists") from error
        raise BundleError("output-write-failed") from error
    finally:
        if temporary is not None:
            try:
                os.unlink(temporary, dir_fd=parent_fd)
            except FileNotFoundError:
                pass
        if parent_fd != root_fd:
            os.close(parent_fd)
        os.close(root_fd)


def _require_safe_platform() -> None:
    if not _DIR_FD_SUPPORTED or not hasattr(os, "O_DIRECTORY"):
        raise BundleError("output-unsupported")
    if not hasattr(os, "O_NOFOLLOW"):
        raise BundleError("output-unsupported")


def _no_follow_flag() -> int:
    return os.O_NOFOLLOW


def _absolute(path: Path) -> Path:
    absolute = path if path.is_absolute() else Path.cwd() / path
    if ".." in absolute.parts:
        raise BundleError("unsafe-output-path")
    return absolute


def _open_approved_root(path: Path) -> int:
    descriptor = _open_directory("/", private=False)
    try:
        for part in path.parts[1:]:
            next_fd = _open_directory(part, dir_fd=descriptor, private=False)
            os.close(descriptor)
            descriptor = next_fd
        _private_directory(descriptor)
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def _open_directory(path: str | Path, *, dir_fd: int | None = None, private: bool = False) -> int:
    flags = os.O_RDONLY | os.O_DIRECTORY | _no_follow_flag()
    try:
        descriptor = os.open(path, flags, dir_fd=dir_fd)
    except OSError as error:
        raise BundleError("unsafe-output-path") from error
    if not stat.S_ISDIR(os.fstat(descriptor).st_mode):
        os.close(descriptor)
        raise BundleError("unsafe-output-path")
    if private:
        try:
            _private_directory(descriptor)
        except BaseException:
            os.close(descriptor)
            raise
    return descriptor


def _private_directory(descriptor: int) -> None:
    info = os.fstat(descriptor)
    if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise BundleError("unsafe-output-path")
