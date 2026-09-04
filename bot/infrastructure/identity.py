"""The bot's own avatar and banner, kept on disk for the portal to serve.

Discord will hand these over, but only over the network and only while the
gateway is up, and the portal is a page somebody opens on a phone at 9pm --
often the moment something is already wrong.  So they are fetched once at start
(:meth:`bot.agent.client.BossBot.cache_identity`) and written next to the database,
and every read after that is a file read.

Everything here is cosmetic and everything here is allowed to fail.  A refresh
that cannot reach Discord leaves the previous copy exactly where it was, so the
sign-in page keeps its artwork through an outage and a first run with no network
simply has none.  Nothing in this module may raise into a start-up path.

Written atomically -- temp file then :func:`os.replace` in the same directory --
because the alternative is a browser fetching a half-written PNG and caching the
truncated bytes for a day.
"""

from __future__ import annotations

import logging
import os
from pathlib import Path

log = logging.getLogger(__name__)

#: Beside the database file, like ``backups`` -- so it follows ``DB_PATH`` and
#: needs no second setting to keep in step with the mount.
IDENTITY_DIR_NAME = "identity"

AVATAR_NAME = "avatar.png"
BANNER_NAME = "banner.png"

#: The formats Discord can answer with. The cache filename is always ``.png``
#: (an animated avatar arrives as a GIF under it), so the content type is read
#: from the first bytes rather than from the suffix.
_MAGIC = (
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"GIF8", "image/gif"),
    (b"\xff\xd8\xff", "image/jpeg"),
)


def identity_dir(db_path: str | Path) -> Path | None:
    """Where this deployment's identity art belongs, or ``None`` for ``:memory:``."""
    if str(db_path) == ":memory:":
        return None
    return Path(db_path).parent / IDENTITY_DIR_NAME


def cached(db_path: str | Path, name: str) -> Path | None:
    """The cached file, if one has ever been written."""
    directory = identity_dir(db_path)
    if directory is None:
        return None
    path = directory / name
    return path if path.is_file() else None


def media_type(path: Path) -> str:
    """What the cached bytes actually are, read off their first few."""
    try:
        with path.open("rb") as handle:
            head = handle.read(12)
    except OSError:  # pragma: no cover - the caller has just stat'd it
        return "image/png"
    for magic, kind in _MAGIC:
        if head.startswith(magic):
            return kind
    if head.startswith(b"RIFF") and head[8:12] == b"WEBP":
        return "image/webp"
    return "image/png"


def write_atomic(path: Path, data: bytes) -> None:
    """Write ``data`` so a reader only ever sees the old file or the whole new one."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_name(f"{path.name}.part")
    temp.write_bytes(data)
    os.replace(temp, path)  # same directory, so the swap is atomic


async def _store(path: Path, asset: object) -> bool:
    """Download one ``discord.Asset`` into ``path``; ``False`` if anything stopped it."""
    if asset is None:
        return False
    try:
        data = await asset.read()  # type: ignore[attr-defined]
    except Exception:
        log.debug("could not download %s", path.name, exc_info=True)
        return False
    if not data:
        return False
    try:
        write_atomic(path, data)
    except OSError:
        log.warning("could not write %s", path, exc_info=True)
        return False
    return True


async def refresh(client: object) -> list[str]:
    """Re-download the bot's avatar and banner; returns what was written.

    The avatar is on the client already -- ``client.user`` is filled from the
    gateway's READY payload.  The **banner is not**: discord.py only populates
    ``User.banner`` from a REST fetch of the full user, so it costs one
    ``fetch_user`` of the bot's own id.  That call is the one part of this that
    talks to the network on its own account, so it is caught separately: a bot
    with no banner and a bot Discord would not answer about look the same from
    here, and neither is worth a log line above DEBUG.
    """
    settings = getattr(client, "settings", None)
    user = getattr(client, "user", None)
    directory = identity_dir(getattr(settings, "db_path", ":memory:"))
    if directory is None or user is None:
        return []

    written: list[str] = []
    avatar = getattr(user, "display_avatar", None) or getattr(user, "avatar", None)
    if await _store(directory / AVATAR_NAME, avatar):
        written.append(AVATAR_NAME)

    banner = None
    try:
        full = await client.fetch_user(user.id)  # type: ignore[attr-defined]
        banner = getattr(full, "banner", None)
    except Exception:
        log.debug("could not fetch the bot's own profile for its banner", exc_info=True)
    if await _store(directory / BANNER_NAME, banner):
        written.append(BANNER_NAME)
    return written


__all__ = [
    "AVATAR_NAME",
    "BANNER_NAME",
    "IDENTITY_DIR_NAME",
    "cached",
    "identity_dir",
    "media_type",
    "refresh",
    "write_atomic",
]
