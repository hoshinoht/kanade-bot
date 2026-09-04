"""Boss-week arithmetic.

A *boss week* starts at the configured reset instant (weekday + wall-clock time
in the guild timezone, e.g. Thu 00:00 Asia/Kuala_Lumpur) and runs for exactly
seven days.  All functions here are pure so they can be unit tested without a
database or a Discord connection.
"""

from __future__ import annotations

import re
from datetime import datetime, time, timedelta
from zoneinfo import ZoneInfo

from .timeutil import utcnow

#: Python's ``date.weekday()`` numbering: Monday == 0.
WEEKDAYS: dict[str, int] = {
    "mon": 0,
    "tue": 1,
    "wed": 2,
    "thu": 3,
    "fri": 4,
    "sat": 5,
    "sun": 6,
}

WEEKDAY_NAMES: list[str] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]

_ALIASES: dict[str, int] = {
    **WEEKDAYS,
    "monday": 0,
    "tuesday": 1,
    "wednesday": 2,
    "weds": 2,
    "thursday": 3,
    "thurs": 3,
    "friday": 4,
    "saturday": 5,
    "sunday": 6,
}


def parse_weekday(value: str | int) -> int:
    """Turn ``"thu"`` / ``"Thursday"`` / ``3`` into a Monday-zero weekday index."""
    if isinstance(value, int):
        if 0 <= value <= 6:
            return value
        raise ValueError(f"weekday out of range: {value}")
    key = value.strip().lower()
    if key.isdigit():
        return parse_weekday(int(key))
    try:
        return _ALIASES[key]
    except KeyError:
        raise ValueError(
            f"unknown weekday {value!r}; expected one of {', '.join(WEEKDAYS)}"
        ) from None


_HHMM_RE = re.compile(r"^(\d{1,2})(?:[:.](\d{2}))?\s*(am|pm|a|p)?$", re.IGNORECASE)
_COMPACT_RE = re.compile(r"^(\d{3,4})\s*(am|pm|a|p)?$", re.IGNORECASE)


def parse_hhmm(value: str) -> time:
    """Parse a clock time the way people actually type it.

    Accepts ``09:00``, ``9:05``, ``9.30``, 24-hour compact ``2130`` / ``930``,
    and 12-hour forms ``9pm``, ``9:30pm``, ``930pm``, ``12am``. Compact digits
    are read as 24-hour (``2359`` is 23:59, ``930`` is 09:30).
    """
    text = value.strip()
    hint = f"expected a time like 21:30, 2130 or 9:30pm, got {value!r}"
    match = _COMPACT_RE.match(text)
    if match:
        digits, suffix = match.group(1), match.group(2)
        hour, minute = int(digits[:-2]), int(digits[-2:])
    else:
        match = _HHMM_RE.match(text)
        if not match:
            raise ValueError(hint)
        hour, minute, suffix = int(match.group(1)), int(match.group(2) or 0), match.group(3)
    if suffix:
        if not 1 <= hour <= 12:
            raise ValueError(hint)
        meridiem = suffix.lower()[0]
        if meridiem == "p" and hour != 12:
            hour += 12
        elif meridiem == "a" and hour == 12:
            hour = 0
    if not (0 <= hour <= 23 and 0 <= minute <= 59):
        raise ValueError(hint)
    return time(hour, minute)


def _localise(naive: datetime, tz: ZoneInfo) -> datetime:
    return naive.replace(tzinfo=tz)


def week_start(dt: datetime, tz: ZoneInfo, reset_weekday: int, reset_time: time) -> datetime:
    """The most recent reset instant at or before ``dt``.

    ``dt`` must be timezone-aware.  The result is aware and expressed in ``tz``.
    A ``dt`` sitting exactly on a reset instant belongs to the week that reset
    starts, so it is returned unchanged.
    """
    if dt.tzinfo is None:
        raise ValueError("week_start() needs an aware datetime")
    local = dt.astimezone(tz)
    days_back = (local.weekday() - reset_weekday) % 7
    candidate = _localise(
        datetime.combine(local.date() - timedelta(days=days_back), reset_time), tz
    )
    if candidate > local:
        candidate = _localise(
            candidate.replace(tzinfo=None) - timedelta(days=7),
            tz,
        )
    return candidate


def week_end(ws: datetime, tz: ZoneInfo) -> datetime:
    """The exclusive end of the boss week starting at ``ws`` (== next reset)."""
    return _localise(ws.astimezone(tz).replace(tzinfo=None) + timedelta(days=7), tz)


def current_week_start(
    tz: ZoneInfo, reset_weekday: int, reset_time: time, now: datetime | None = None
) -> datetime:
    """Start of the boss week containing ``now`` (defaults to the real now)."""
    return week_start(now or utcnow(), tz, reset_weekday, reset_time)


def next_week_start(
    tz: ZoneInfo, reset_weekday: int, reset_time: time, now: datetime | None = None
) -> datetime:
    """Start of the boss week after the one containing ``now``."""
    return week_end(current_week_start(tz, reset_weekday, reset_time, now), tz)


def slot_in_week(ws: datetime, tz: ZoneInfo, weekday: int, at: time) -> datetime:
    """The instant inside the week ``ws`` matching ``weekday`` at ``at``.

    Used to place a fixed run (e.g. "Mon 21:30") into a concrete week.  If the
    naive placement lands before the reset it is pushed forward a week so the
    result always lies in ``[ws, week_end(ws))``.
    """
    local_ws = ws.astimezone(tz)
    days = (weekday - local_ws.weekday()) % 7
    candidate = _localise(datetime.combine(local_ws.date() + timedelta(days=days), at), tz)
    if candidate < local_ws:
        candidate = _localise(candidate.replace(tzinfo=None) + timedelta(days=7), tz)
    return candidate
