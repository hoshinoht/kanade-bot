"""How much history a rescan reads, and how it is cut into bursts.

Pure functions: no database, no Discord, no model.  A rescan covers a whole
boss week by default, which is far more chat than one prompt should carry, so
the messages are grouped back into the conversations they came from -- a gap of
:data:`BURST_GAP` ends one -- and each group becomes its own model call, oldest
first.  That is the same shape the live pipeline produces from its debounce, so
the prompt the model sees during a rescan looks like the prompt it sees live.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from datetime import datetime, time, timedelta
from typing import Any
from zoneinfo import ZoneInfo

from ..timeutil import utcnow
from ..weeks import current_week_start, week_start

#: Silence that ends a burst when replaying history. The live debounce is 90 s;
#: history is replayed with a wider gap so a whole evening's planning arrives as
#: one burst rather than a dozen single messages.
BURST_GAP = timedelta(minutes=15)

#: What `/rescan window:` offers, longest first.
WINDOWS: tuple[str, ...] = ("week", "2weeks", "48h", "24h")

DEFAULT_WINDOW = "week"

#: Fixed-length windows, in hours. `week`/`2weeks` are boss weeks, not 7×24 h,
#: so a rescan on Thursday morning does not reach back past the reset.
_HOUR_WINDOWS: dict[str, int] = {"24h": 24, "48h": 48}


def window_since(
    window: str,
    tz: ZoneInfo,
    reset_weekday: int,
    reset_time: time,
    now: datetime | None = None,
) -> datetime:
    """The instant a rescan window starts at.

    ``week`` is the current boss week's reset, ``2weeks`` the one before it --
    which is why they are not simply 168 and 336 hours.
    """
    now = now or utcnow()
    key = (window or DEFAULT_WINDOW).strip().lower()
    hours = _HOUR_WINDOWS.get(key)
    if hours is not None:
        return now - timedelta(hours=hours)
    this_week = current_week_start(tz, reset_weekday, reset_time, now)
    if key == "week":
        return this_week
    if key == "2weeks":
        return previous_week_start(this_week, tz, reset_weekday, reset_time)
    raise ValueError(f"unknown window {window!r}; expected one of {', '.join(WINDOWS)}")


def previous_week_start(
    this_week: datetime, tz: ZoneInfo, reset_weekday: int, reset_time: time
) -> datetime:
    """The reset before ``this_week`` -- what the fallback widens to."""
    return week_start(this_week - timedelta(seconds=1), tz, reset_weekday, reset_time)


def should_widen(window: str, gated_count: int) -> bool:
    """Whether to look back one more boss week.

    Only from the default ``week`` window, and only when the week held *no*
    scheduling chat at all -- a quiet week early on Thursday is the normal case
    right after a reset, and the useful answer is last week's plan.  Never more
    than two weeks: a card about a run that has already happened is noise
    (see :data:`bot.extract.pipeline.STALE_GRACE`).
    """
    return window == "week" and gated_count == 0


def _created_at(row: Any) -> datetime:
    return row["created_at"]


def group_bursts(
    rows: Sequence[Any],
    gap: timedelta = BURST_GAP,
    key: Callable[[Any], datetime] = _created_at,
) -> list[list[Any]]:
    """Split messages into conversations wherever the channel went quiet.

    ``rows`` must be in chronological order.  Returns a list of non-empty
    groups; an empty input gives an empty list.
    """
    groups: list[list[Any]] = []
    current: list[Any] = []
    previous: datetime | None = None
    for row in rows:
        when = key(row)
        if previous is not None and when - previous > gap and current:
            groups.append(current)
            current = []
        current.append(row)
        previous = when
    if current:
        groups.append(current)
    return groups


__all__ = [
    "BURST_GAP",
    "DEFAULT_WINDOW",
    "WINDOWS",
    "group_bursts",
    "previous_week_start",
    "should_widen",
    "window_since",
]
