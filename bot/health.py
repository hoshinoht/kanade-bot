"""Container healthcheck: ``python -m bot.health``.

Exits 0 when the SQLite database opens and the bot's tick loop wrote a
``heartbeat`` within the last few minutes, 1 otherwise.  Deliberately reads
``DB_PATH`` straight from the environment rather than through
:class:`bot.config.Settings`, so a missing ``DISCORD_TOKEN`` cannot make a
healthy container look unhealthy.
"""

from __future__ import annotations

import os
import sqlite3
import sys
from datetime import timedelta

from .timeutil import from_iso, utcnow

MAX_AGE = timedelta(minutes=3)


def check(db_path: str | None = None, max_age: timedelta = MAX_AGE) -> tuple[bool, str]:
    path = db_path or os.environ.get("DB_PATH", "data/bot.sqlite")
    try:
        conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.Error as exc:
        return False, f"cannot open {path}: {exc}"
    try:
        row = conn.execute("SELECT value FROM config WHERE key = 'heartbeat'").fetchone()
    except sqlite3.Error as exc:
        return False, f"cannot read config: {exc}"
    finally:
        conn.close()
    if row is None:
        return False, "no heartbeat recorded yet"
    try:
        age = utcnow() - from_iso(row[0])
    except ValueError as exc:
        return False, f"unparseable heartbeat: {exc}"
    if age > max_age:
        return False, f"heartbeat is {age.total_seconds():.0f}s old"
    return True, f"ok ({age.total_seconds():.0f}s)"


def main() -> int:
    healthy, detail = check()
    print(detail, file=sys.stdout if healthy else sys.stderr)
    return 0 if healthy else 1


if __name__ == "__main__":
    raise SystemExit(main())
