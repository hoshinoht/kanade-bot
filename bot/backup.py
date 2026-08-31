"""Daily snapshots of the SQLite database, onto the host.

The live database lives in a Docker *named volume* (see ``compose.yaml``):
SQLite over macOS VirtioFS has weak fsync guarantees and a hard kill mid-write
corrupted it once.  That makes the volume the only live copy, and a volume is
exactly the thing ``ls`` on the host cannot see -- so a snapshot goes to
``/app/data/backups``, a bind mount nested inside it, and lands in the repo's
``data/backups`` on the Mac.

Snapshots go through SQLite's **online backup API**, never a file copy: with
``journal_mode=WAL`` the ``.sqlite`` file alone is only whatever was last
checkpointed, and copying it while the bot is writing can produce a file that
does not open.  :meth:`bot.db.Repo.backup_to` is the one page-consistent way to
do it while the connection stays open.
"""

from __future__ import annotations

import logging
import re
from datetime import datetime, time, timedelta
from pathlib import Path

log = logging.getLogger(__name__)

#: Where snapshots go, relative to the database file itself -- so it follows
#: ``DB_PATH`` and needs no second setting to keep in step with the mount.
BACKUP_DIR_NAME = "backups"

#: ``bot.sqlite.20260831-040000``: the database's name plus a local timestamp.
BACKUP_PREFIX = "bot.sqlite."
BACKUP_STAMP = "%Y%m%d-%H%M%S"

#: Only files this module wrote are ever pruned.  A hand-made copy kept for a
#: reason -- ``bot.sqlite.corrupt-20260831``, an export, a note -- does not match
#: and is never touched.
BACKUP_RE = re.compile(r"^bot\.sqlite\.\d{8}-\d{6}$")

#: How many snapshots to keep: two weeks of daily copies, ~2 MB each at this
#: scale.  Long enough to notice damage that happened while nobody was looking.
KEEP = 14

#: The local hour a snapshot is taken at -- quiet, and after the boss week's
#: latest runs have finished.  Nothing is scheduled *at* it (see
#: :func:`due_day`); the tick asks "has today been backed up?", so a host that
#: was asleep at 04:00 takes its snapshot when it wakes instead of skipping.
BACKUP_AT = time(4, 0)


def backup_dir(db_path: str | Path) -> Path | None:
    """Where this database's snapshots belong, or ``None`` for ``:memory:``."""
    if str(db_path) == ":memory:":
        return None
    return Path(db_path).parent / BACKUP_DIR_NAME


def due_day(last: str | None, local_now: datetime) -> str | None:
    """The local calendar day a snapshot is owed for, or ``None`` if none is.

    ``last`` is the ``YYYY-MM-DD`` of the last successful snapshot.  ISO dates
    sort chronologically as strings, so the comparisons need no parsing.

    * already done today -> nothing;
    * never done, or a whole day missed -> **now**, whatever the hour: the host
      was off, and waiting for 04:00 to come round again would leave the gap
      open for another day;
    * done yesterday -> at :data:`BACKUP_AT`.
    """
    today = local_now.date()
    if last == today.isoformat():
        return None
    if last is None or last < (today - timedelta(days=1)).isoformat():
        return today.isoformat()
    return today.isoformat() if local_now.time() >= BACKUP_AT else None


def snapshot_path(directory: Path, local_now: datetime) -> Path:
    return directory / f"{BACKUP_PREFIX}{local_now.strftime(BACKUP_STAMP)}"


def prune(directory: Path, keep: int = KEEP) -> list[Path]:
    """Delete all but the newest ``keep`` snapshots; returns what went.

    Newest is decided by name, which sorts chronologically by construction, so
    a clock that jumped cannot reorder the directory.
    """
    ours = sorted(
        (p for p in directory.iterdir() if p.is_file() and BACKUP_RE.match(p.name)),
        key=lambda p: p.name,
        reverse=True,
    )
    removed: list[Path] = []
    for path in ours[keep:]:
        path.unlink()
        removed.append(path)
    return removed


def take(repo, directory: Path, local_now: datetime, keep: int = KEEP) -> tuple[Path, list[Path]]:
    """Write one snapshot and prune the old ones; returns ``(new, removed)``."""
    directory.mkdir(parents=True, exist_ok=True)
    path = snapshot_path(directory, local_now)
    repo.backup_to(path)
    return path, prune(directory, keep)
