"""Turning the model's literal ``day_ref``/``time_ref`` into a real datetime.

The model is never asked what date "weds" is (DESIGN.md §2.1): it echoes the
expression it saw and this module resolves it, deterministically, against the
timestamp of the latest message that was used as evidence.

Why not ``dateparser``
----------------------
``dateparser`` is used for slash-command input (``/amend to: wed 21:30``), where
people type carefully.  It does not survive this chat.  Measured against
``RELATIVE_BASE`` with ``PREFER_DATES_FROM=future`` and the guild timezone:

``tonight`` ``tmr`` ``weds`` ``thurs`` ``930`` ``1130pm`` ``11pm onward``
``later`` ``this sunday`` ``next mon``   -> ``None``
``1030~11+pm``  -> ``1030-08-30`` (the year 1030)
``at 11``       -> 30 November
``9pm``         -> *tomorrow* 21:00

Guessing wrong here silently moves a run, so the parsing is spelled out below and
anything unrecognised returns ``None``, leaving the field ``TBD`` on the card.

The pm rule
-----------
A bare hour of 1-11 with no ``am``/``pm`` means **pm**: this guild's runs are all
evening runs and "930", "10", "at 11" are always 21:30, 22:00, 23:00.  A bare
``12`` means midnight, which is how "12" reads at the end of a boss night.  Hours
of 13-23 are already 24-hour and are left alone.  This is deliberately *not*
:func:`bot.domain.weeks.parse_hhmm`, which reads compact digits as 24-hour (``930`` ->
09:30) because it parses times typed into a slash command, not chat.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from datetime import date, datetime, time, timedelta
from zoneinfo import ZoneInfo

from bot.domain.weeks import WEEKDAYS

#: Bare hours at or below this are read as pm; see the module docstring.
PM_CUTOFF = 11

_WEEKDAY_ALIASES: dict[str, int] = {
    **WEEKDAYS,
    "monday": 0,
    "tuesday": 1,
    "tues": 1,
    "wednesday": 2,
    "weds": 2,
    "wedns": 2,
    "thursday": 3,
    "thur": 3,
    "thurs": 3,
    "friday": 4,
    "saturday": 5,
    "sunday": 6,
}

#: ``day_ref`` values that mean "the day the message was sent".
TODAY_WORDS = frozenset({"today", "tonight", "tonite", "tnite", "this evening", "this night"})
#: ...and these mean the same day, but so vaguely that a past clock time should
#: roll into tomorrow rather than resolve to something that already happened.
SOON_WORDS = frozenset({"now", "later", "ltr", "l8r", "soon", "in a bit", "just now"})
TOMORROW_WORDS = frozenset(
    {"tmr", "tmrw", "tmmr", "tmr night", "tomorrow", "tomorow", "tomm", "2mr"}
)
YESTERDAY_WORDS = frozenset({"ytd", "yesterday"})

_ISO_DATE_RE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})$")
_MERIDIEM_RE = re.compile(r"\b(a\.?m\.?|p\.?m\.?)\b|(?<=\d)\s*(am|pm)\b", re.IGNORECASE)
#: Words that only ever qualify a day/time and never change its value.
_DAY_NOISE_RE = re.compile(
    r"\b(?:this|coming|on|the|at|around|about|ard|by|from|night|nite|evening|"
    r"morning|afternoon|onwards?|ish|latest|earliest|sharp|pls|please)\b",
    re.IGNORECASE,
)
_TIME_NOISE_RE = re.compile(
    r"\b(?:at|around|about|ard|by|from|onwards?|ish|latest|plus|sharp|"
    r"night|nite|evening|morning|afternoon|pm ish)\b",
    re.IGNORECASE,
)
_NEXT_RE = re.compile(r"\bnext\b", re.IGNORECASE)


@dataclass(frozen=True)
class Resolved:
    """What could be pinned down, and what could not.

    ``at`` is filled in only when both a day and a clock time are known; a
    day-only reference ("wed") leaves it ``None`` so the card can say
    "Wed — time TBD" instead of inventing 00:00.
    """

    day: date | None = None
    clock: time | None = None
    at: datetime | None = None
    #: A bare hour was read as pm (see :data:`PM_CUTOFF`).
    assumed_pm: bool = False

    @property
    def known(self) -> bool:
        return self.day is not None or self.clock is not None


# ---------------------------------------------------------------------------
# clock times
# ---------------------------------------------------------------------------

_RANGE_RE = re.compile(r"^\s*(\d{1,2}(?:[:.]?\d{2})?)\s*\+?\s*[~\-]\s*\d{1,4}\s*\+?\s*(.*)$")
_RANGE_SPLIT_RE = re.compile(r"[~\-–—]|\bto\b|\btill\b|\buntil\b", re.IGNORECASE)
_HHMM_RE = re.compile(r"^(\d{1,2})[:.](\d{2})$")
_COMPACT_RE = re.compile(r"^(\d{3,4})$")
_HOUR_RE = re.compile(r"^(\d{1,2})$")


def _apply_meridiem(hour: int, meridiem: str | None) -> tuple[int, bool]:
    """``(hour24, assumed)``.  ``assumed`` marks the bare-hour pm default."""
    if meridiem:
        letter = meridiem.lower().replace(".", "")[0]
        if letter == "p" and hour != 12:
            return hour + 12, False
        if letter == "a" and hour == 12:
            return 0, False
        return hour, False
    if 1 <= hour <= PM_CUTOFF:
        return hour + 12, True
    if hour == 12:
        # "12" at the end of a boss night is midnight, not lunchtime.
        return 0, True
    return hour, False


def parse_clock(time_ref: str | None) -> tuple[time, bool] | None:
    """``"9:30pm"`` -> ``(21:30, False)``, ``"930"`` -> ``(21:30, True)``.

    Returns ``None`` for anything that is not a clock time -- including "night",
    "later" and "after boss", which are day-ish, not times.
    """
    if not time_ref:
        return None
    text = str(time_ref).strip().lower()
    if not text:
        return None

    match = _MERIDIEM_RE.search(text)
    meridiem = (match.group(1) or match.group(2)) if match else None

    # A range ("1030~11+pm", "8~1130", "9-10pm") is the *start* time; any
    # meridiem written at the end of the range applies to it.
    range_match = _RANGE_RE.match(text)
    core = range_match.group(1) if range_match else _MERIDIEM_RE.sub(" ", text)
    core = _TIME_NOISE_RE.sub(" ", core)
    # Any other range spelling ("9pm-11pm", "11 to 1145pm"): the start is the time.
    core = _RANGE_SPLIT_RE.split(core, maxsplit=1)[0]
    core = re.sub(r"[^\d:.]", "", core).strip(".:")
    if not core:
        return None

    for pattern, reader in (
        (_HHMM_RE, lambda m: (int(m.group(1)), int(m.group(2)))),
        (_COMPACT_RE, lambda m: (int(m.group(1)[:-2]), int(m.group(1)[-2:]))),
        (_HOUR_RE, lambda m: (int(m.group(1)), 0)),
    ):
        found = pattern.match(core)
        if not found:
            continue
        hour, minute = reader(found)
        if minute > 59 or hour > 23:
            return None
        hour, assumed = _apply_meridiem(hour, meridiem)
        if hour > 23:
            return None
        return time(hour, minute), assumed
    return None


# ---------------------------------------------------------------------------
# days
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class _DayRef:
    day: date
    #: The reference named a specific day, so a clock time already past must not
    #: roll into tomorrow ("today 9pm" said at 10pm is still today).
    explicit: bool


def _parse_day(day_ref: str | None, anchor: datetime) -> _DayRef | None:
    if not day_ref:
        return None
    text = str(day_ref).strip().lower()
    if not text:
        return None

    iso = _ISO_DATE_RE.match(text)
    if iso:
        try:
            return _DayRef(date(int(iso.group(1)), int(iso.group(2)), int(iso.group(3))), True)
        except ValueError:
            return None

    wants_next = bool(_NEXT_RE.search(text))
    cleaned = _NEXT_RE.sub(" ", text)
    cleaned = _DAY_NOISE_RE.sub(" ", cleaned)
    words = [w for w in re.split(r"[^a-z0-9]+", cleaned) if w]

    phrase = " ".join(words)
    if phrase in TODAY_WORDS or (words and words[0] in TODAY_WORDS):
        return _DayRef(anchor.date(), True)
    if phrase in SOON_WORDS or (words and words[0] in SOON_WORDS):
        return _DayRef(anchor.date(), False)
    if phrase in TOMORROW_WORDS or (words and words[0] in TOMORROW_WORDS):
        return _DayRef(anchor.date() + timedelta(days=1), True)
    if phrase in YESTERDAY_WORDS or (words and words[0] in YESTERDAY_WORDS):
        return _DayRef(anchor.date() - timedelta(days=1), True)

    for word in words:
        target = _WEEKDAY_ALIASES.get(word)
        if target is None:
            continue
        ahead = (target - anchor.weekday()) % 7
        if wants_next and ahead == 0:
            ahead = 7
        # Same weekday, no explicit time yet: "wed" said on a Wednesday means
        # today; the caller rolls it a week on if the clock time has passed.
        return _DayRef(anchor.date() + timedelta(days=ahead), ahead != 0)
    return None


# ---------------------------------------------------------------------------
# the public entry point
# ---------------------------------------------------------------------------


def resolve(
    day_ref: str | None,
    time_ref: str | None,
    anchor: datetime,
    tz: ZoneInfo,
) -> Resolved:
    """Resolve a day/time reference against the latest evidence message.

    ``anchor`` is that message's timestamp (any timezone; it is converted to
    ``tz`` first).  Neither field is required: a day with no time resolves to a
    date, a time with no day to that time on the anchor's day, and neither to an
    empty :class:`Resolved`.  Anything unparseable comes back empty -- the card
    then says ``TBD`` rather than guessing.
    """
    if anchor.tzinfo is None:
        raise ValueError("resolve() needs an aware anchor datetime")
    local = anchor.astimezone(tz)

    clock = parse_clock(time_ref)
    day = _parse_day(day_ref, local)
    if clock is None and day is None:
        return Resolved()
    if clock is None:
        return Resolved(day=day.day if day else None)

    clock_time, assumed_pm = clock
    if day is None:
        # A time with no day is "today", and rolls into tomorrow once it is past.
        candidate = datetime.combine(local.date(), clock_time, tzinfo=tz)
        if candidate < local:
            candidate += timedelta(days=1)
        return Resolved(day=candidate.date(), clock=clock_time, at=candidate, assumed_pm=assumed_pm)

    candidate = datetime.combine(day.day, clock_time, tzinfo=tz)
    if candidate < local and not day.explicit:
        # "later at 11" past 23:00, or "wed 9:30pm" said late on a Wednesday.
        candidate += timedelta(days=7 if _parse_weekday_only(day_ref) else 1)
    return Resolved(day=candidate.date(), clock=clock_time, at=candidate, assumed_pm=assumed_pm)


def _parse_weekday_only(day_ref: str | None) -> bool:
    """True when ``day_ref`` names a weekday, so a past time rolls a week not a day."""
    if not day_ref:
        return False
    cleaned = _DAY_NOISE_RE.sub(" ", _NEXT_RE.sub(" ", str(day_ref).lower()))
    return any(w in _WEEKDAY_ALIASES for w in re.split(r"[^a-z0-9]+", cleaned) if w)


#: Every spelling of a weekday this guild actually uses, mapped to its index.
#: Public because it is not only the extractor's: :mod:`bot.chat.tools` reads
#: "hstar weds" the same way, and two tables would drift.
WEEKDAY_ALIASES = _WEEKDAY_ALIASES

__all__ = ["PM_CUTOFF", "WEEKDAY_ALIASES", "Resolved", "parse_clock", "resolve"]
