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
#: history is replayed with a much wider gap because a planning thread is not
#: continuous -- a real one ran 11:50 -> 13:15 with fifty-minute pauses, and at
#: fifteen minutes it came apart into six bursts of one to four messages. The
#: model then never saw "then weds lah" next to "Wed i can from 9:30pm", and
#: produced TBD cards from a conversation that had settled.
BURST_GAP = timedelta(hours=3)

#: A burst this big is already more than one prompt should carry, so it is split
#: at its longest internal pause rather than sent whole. A real morning rescan
#: packed 21 messages of one calendar day into a single 7.5k-token prompt, which
#: overran the model's context window and came back as truncated JSON; a
#: six-message burst of the same chat extracted cleanly. The token budget in
#: :func:`bot.extract.prompt.prompt_budget` is what actually guarantees a prompt
#: fits -- this is the cheap cap that keeps most bursts well clear of it.
MAX_BURST_MESSAGES = 12

#: What `/rescan window:` offers, longest first.
WINDOWS: tuple[str, ...] = ("week", "2weeks", "48h", "24h")

DEFAULT_WINDOW = "week"

#: The furthest back the bot may look **on its own initiative**. A person asking
#: for a week has decided that re-reading it is worth the model time and the
#: cards it might post; a scheduled sweep has decided nothing, and quietly
#: re-reading a fortnight every hour is how a bot becomes noise.
AUTOMATED_WINDOW = "48h"

#: Fixed-length windows, in hours. `week`/`2weeks` are boss weeks, not 7×24 h,
#: so a rescan on Thursday morning does not reach back past the reset.
_HOUR_WINDOWS: dict[str, int] = {"24h": 24, "48h": 48}


def clamp_window(window: str, automated: bool) -> str:
    """The window that will actually be read.

    Automated rescans are capped at :data:`AUTOMATED_WINDOW`; a manual one is
    taken at its word. Enforced here rather than by each caller remembering, so
    a future scheduled sweep cannot widen itself by accident.
    """
    key = (window or DEFAULT_WINDOW).strip().lower()
    if key not in WINDOWS:
        raise ValueError(f"unknown window {window!r}; expected one of {', '.join(WINDOWS)}")
    if not automated:
        return key
    return key if key in _HOUR_WINDOWS else AUTOMATED_WINDOW


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


def should_widen(window: str, gated_count: int, automated: bool = False) -> bool:
    """Whether to look back one more boss week.

    Only from the default ``week`` window, only when the week held *no*
    scheduling chat at all -- a quiet week early on Thursday is the normal case
    right after a reset, and the useful answer is last week's plan -- and never
    for an automated run, which is capped at :data:`AUTOMATED_WINDOW` and has
    nobody waiting on an answer. Never more than two weeks either way: a card
    about a run that has already happened is noise (see
    :data:`bot.extract.pipeline.STALE_GRACE`).
    """
    return not automated and window == "week" and gated_count == 0


def _created_at(row: Any) -> datetime:
    return row["created_at"]


def group_for_rescan(
    rows: Sequence[Any],
    tz: ZoneInfo,
    gap: timedelta = BURST_GAP,
    cap: int = MAX_BURST_MESSAGES,
    key: Callable[[Any], datetime] = _created_at,
) -> list[list[Any]]:
    """Cut a window of history into the conversations it actually was.

    A boss night is one conversation even when it has long pauses in it, so the
    unit is **the local calendar day**: if a day's scheduling chat fits in one
    prompt it goes as one burst, pauses and all. Only when a day is too big for
    that is it split -- first on :data:`BURST_GAP` silences, and then, for
    anything still over ``cap``, at its longest internal pause until every piece
    fits. Splitting on the longest pause keeps the halves where the conversation
    genuinely broke rather than at an arbitrary count.
    """
    out: list[list[Any]] = []
    for _day, day_rows in _by_local_day(rows, tz, key):
        if len(day_rows) <= cap:
            out.append(day_rows)
            continue
        for chunk in group_bursts(day_rows, gap, key):
            out.extend(_split_to_fit(chunk, cap, key))
    return out


def _by_local_day(
    rows: Sequence[Any], tz: ZoneInfo, key: Callable[[Any], datetime]
) -> list[tuple[Any, list[Any]]]:
    """``rows`` grouped by their date in the guild timezone, chronologically."""
    days: dict[Any, list[Any]] = {}
    for row in rows:
        days.setdefault(key(row).astimezone(tz).date(), []).append(row)
    return sorted(days.items())


def _longest_pause(rows: list[Any], key: Callable[[Any], datetime]) -> int:
    """Where to cut ``rows`` in two: the index after the longest silence.

    Evenly-spaced chatter has no natural break, so ties go to the most central
    split rather than shaving one message off the front.
    """
    middle = len(rows) // 2
    return max(
        range(1, len(rows)),
        key=lambda index: (key(rows[index]) - key(rows[index - 1]), -abs(index - middle)),
    )


def _split_to_fit(rows: list[Any], cap: int, key: Callable[[Any], datetime]) -> list[list[Any]]:
    """Halve at the longest pause until every piece is at most ``cap`` long."""
    if len(rows) <= cap:
        return [rows]
    at = _longest_pause(rows, key)
    return _split_to_fit(rows[:at], cap, key) + _split_to_fit(rows[at:], cap, key)


def split_until(
    rows: Sequence[Any],
    fits: Callable[[list[Any]], bool],
    key: Callable[[Any], datetime] = _created_at,
) -> list[list[Any]]:
    """Halve at the longest pause until every piece satisfies ``fits``.

    The message-count cap above is a guess at "one prompt's worth"; this is the
    same halving driven by the real answer, which only the caller can work out
    because it depends on how long the messages are and how much context and how
    many runs go in beside them.  A single message that still does not fit comes
    back on its own -- there is nothing left to split.
    """
    rows = list(rows)
    if len(rows) <= 1 or fits(rows):
        return [rows]
    at = _longest_pause(rows, key)
    return split_until(rows[:at], fits, key) + split_until(rows[at:], fits, key)


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
    "AUTOMATED_WINDOW",
    "BURST_GAP",
    "MAX_BURST_MESSAGES",
    "DEFAULT_WINDOW",
    "WINDOWS",
    "clamp_window",
    "group_bursts",
    "group_for_rescan",
    "previous_week_start",
    "should_widen",
    "split_until",
    "window_since",
]
