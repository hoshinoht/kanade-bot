"""Container healthcheck: ``python -m bot.health``.

Two things have to be true for the container to be useful, and this checks both:

* the bot's tick loop wrote a ``heartbeat`` to SQLite in the last few minutes --
  that is the scheduler, and without it no reminder goes out;
* the portal/CLI API answers ``/healthz`` -- that is phase 3's control plane,
  and it lives in the same process, so a wedged API means a wedged loop.

Deliberately reads ``DB_PATH`` and ``API_PORT`` straight from the environment
rather than through :class:`bot.config.Settings`, so a missing ``DISCORD_TOKEN``
cannot make a healthy container look unhealthy.
"""

from __future__ import annotations

import os
import sqlite3
import sys
import urllib.error
import urllib.request
from datetime import timedelta

from .timeutil import from_iso, utcnow

MAX_AGE = timedelta(minutes=3)
#: How long to wait for the in-process API. It is on loopback and does no I/O
#: for `/healthz`, so anything slower than this means the loop is blocked.
API_TIMEOUT = 3.0


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


def check_api(port: int | None = None, timeout: float = API_TIMEOUT) -> tuple[bool, str]:
    """``GET /healthz`` on the loopback of *this* container.

    The API binds ``0.0.0.0`` inside the container (see
    :mod:`bot.api.server`), so its own loopback reaches it without depending on
    the published port mapping.
    """
    port = port or int(os.environ.get("API_PORT", "8080"))
    url = f"http://127.0.0.1:{port}/healthz"
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:  # noqa: S310
            body = response.read(16).decode("utf-8", "replace").strip()
    except urllib.error.URLError as exc:
        return False, f"api not answering on {port}: {exc.reason}"
    except OSError as exc:
        return False, f"api not answering on {port}: {exc}"
    if body != "ok":
        return False, f"api returned {body!r}"
    return True, "api ok"


def main() -> int:
    healthy, detail = check()
    if healthy:
        api_ok, api_detail = check_api()
        healthy, detail = api_ok, f"{detail}, {api_detail}"
    print(detail, file=sys.stdout if healthy else sys.stderr)
    return 0 if healthy else 1


if __name__ == "__main__":
    raise SystemExit(main())
