"""The deterministic keyword gate (DESIGN.md §2.1).

Every watched message is scored here *before* any LLM call.  Banter is dropped,
so a 13 GB model is only woken for messages that could plausibly change the
schedule.  Nothing in this module talks to Discord, Ollama or the database.

Signals
-------
``boss``    a boss alias, with or without a difficulty prefix
``time``    a clock expression -- ``9pm``, ``9:30``, ``930``, ``1030~11+pm``, ``at 11``
``day``     a weekday or a relative day -- ``weds``, ``tmr``, ``tonight``, ``today``
``soon``    ``now`` / ``later`` / ``ltr`` -- real, but far too common to trigger on
``verb``    a scheduling verb -- ``amend``, ``shift``, ``postpone``, ``otot``, ``cancel``
``run``     the weaker ``run``/``do``/``clear`` family
``here``    ``@here`` / ``@everyone``
``mention`` an ``<@id>`` of a roster member
``agree``   ``can`` / ``ok`` / ``kenot`` / ``cannot`` -- an answer, not a proposal

A message is a **strong** hit on ``boss``, ``time``, ``day``, ``verb`` or ``here``
alone.  The weak signals still make it a hit (so it joins the burst and is shown
to the model) but never wake the model by themselves: "Ok" is only meaningful
next to something that was being scheduled, and the pipeline asks for that
context explicitly rather than paying for a model call on every "ok".
"""

from __future__ import annotations

import re
from collections.abc import Collection
from dataclasses import dataclass, field
from typing import Any

#: Signals that are enough, on their own, to run an extraction.
STRONG_SIGNALS = frozenset({"boss", "time", "day", "verb", "here"})

# ---------------------------------------------------------------------------
# bosses
# ---------------------------------------------------------------------------

#: Difficulty spelled out, as it turns up in chat: ``exkalos``, ``hardstar``.
WORD_PREFIXES: dict[str, str] = {
    "easy": "e",
    "normal": "n",
    "norm": "n",
    "hard": "h",
    "chaos": "c",
    "extreme": "x",
    "ex": "x",
}

#: Ordinary words that must never be read as a misspelt boss.  Fuzzy matching is
#: already restricted to the part *after* a difficulty prefix, which removes most
#: of the risk (``start`` never becomes ``Star``); this catches the leftovers.
FUZZY_STOPWORDS = frozenset({"start", "starting", "started", "clear", "chair", "cheap"})

_WORD_RE = re.compile(r"[a-z0-9]+")
_NON_ALNUM_RE = re.compile(r"[^a-z0-9]+")

#: The shortest alias a typo is allowed to be matched against.  ``fa``/``bm``
#: are two characters: one edit away from far too much.
MIN_FUZZY_LENGTH = 4


def _normalise(text: str) -> str:
    return _NON_ALNUM_RE.sub("", text.strip().lower())


def _within_one_edit(a: str, b: str) -> bool:
    """True when ``a`` and ``b`` are at most one insertion/deletion/substitution apart."""
    if a == b:
        return True
    la, lb = len(a), len(b)
    if abs(la - lb) > 1:
        return False
    if la == lb:
        diffs = sum(1 for x, y in zip(a, b, strict=True) if x != y)
        return diffs == 1
    # One is a character shorter: it must be the other with one character removed.
    short, long = (a, b) if la < lb else (b, a)
    i = j = 0
    skipped = False
    while i < len(short) and j < len(long):
        if short[i] == long[j]:
            i += 1
            j += 1
            continue
        if skipped:
            return False
        skipped = True
        j += 1
    return True


@dataclass(frozen=True)
class BossHit:
    """One boss token found in a message.

    ``canonical`` is ``None`` when the token named a boss but not a difficulty
    (``"limbo"``, ``"carling"``) or named a difficulty the boss does not have
    (``hkalos``).  The gate still counts it as a boss signal -- deciding *which*
    HLimbo run is the model's and :mod:`bot.extract.match`'s job -- but nothing
    downstream may invent the missing prefix.
    """

    token: str
    short: str
    difficulty: str | None = None
    canonical: str | None = None
    fuzzy: bool = False


def _resolve_alias(table: Any, key: str, allow_fuzzy: bool) -> tuple[str, bool] | None:
    """``key`` (normalised, prefix already stripped) -> ``(short_name, was_fuzzy)``."""
    short = table.aliases.get(key)
    if short is not None:
        return short, False
    if not allow_fuzzy or len(key) < MIN_FUZZY_LENGTH or key in FUZZY_STOPWORDS:
        return None
    for alias, candidate in table.aliases.items():
        if len(alias) >= MIN_FUZZY_LENGTH and _within_one_edit(key, alias):
            return candidate, True
    return None


def _hit(table: Any, token: str, short: str, letter: str | None, fuzzy: bool) -> BossHit:
    boss = table.bosses[short]
    canonical = boss.canonical(letter) if letter and letter in boss.difficulties else None
    return BossHit(token=token, short=short, difficulty=letter, canonical=canonical, fuzzy=fuzzy)


def find_bosses(text: str, table: Any) -> list[BossHit]:
    """Every boss token in ``text``, in order, de-duplicated by canonical form.

    A bare alias (``limbo``) matches exactly only.  Fuzzy matching (edit distance
    ≤ 1) is applied to the part *after* a difficulty prefix -- ``hstarr``,
    ``nbaldrx`` -- because a bare fuzzy match turns ordinary words into bosses
    (``start`` -> ``Star``), and the difficulty prefix is the thing that makes a
    token unambiguously a boss name in the first place.
    """
    out: list[BossHit] = []
    seen: set[tuple[str, str | None]] = set()

    def add(hit: BossHit) -> None:
        key = (hit.short, hit.difficulty)
        if key not in seen:
            seen.add(key)
            out.append(hit)

    for raw in _WORD_RE.findall((text or "").lower()):
        key = _normalise(raw)
        if not key:
            continue
        # 1. the whole token is an alias: "limbo", "hfa" is not (h + fa), "star"
        exact = _resolve_alias(table, key, allow_fuzzy=False)
        if exact is not None:
            add(_hit(table, raw, exact[0], None, False))
            continue
        # 2. spelled-out difficulty: "exkalos", "hardstar"
        matched = False
        for word, letter in WORD_PREFIXES.items():
            if key.startswith(word) and len(key) > len(word):
                found = _resolve_alias(table, key[len(word) :], allow_fuzzy=True)
                if found is not None:
                    add(_hit(table, raw, found[0], letter, found[1]))
                    matched = True
                    break
        if matched:
            continue
        # 3. single-letter difficulty prefix: "hstar", "ncarl", "hstarr"
        letter, rest = key[0], key[1:]
        if letter in table.difficulties and rest:
            found = _resolve_alias(table, rest, allow_fuzzy=True)
            if found is not None:
                add(_hit(table, raw, found[0], letter, found[1]))
    return out


def canonical_bosses(hits: Collection[BossHit]) -> list[str]:
    """The canonical names among ``hits``, in order, de-duplicated."""
    out: list[str] = []
    for hit in hits:
        if hit.canonical and hit.canonical not in out:
            out.append(hit.canonical)
    return out


# ---------------------------------------------------------------------------
# times
# ---------------------------------------------------------------------------

#: ``cc9``, ``cc 6``, ``ch3``, ``ch7`` -- MapleStory channel numbers, not times.
#: Masked out before the time scan so "ch7 hstar" is a boss, never a 7 o'clock.
_CHANNEL_REF_RE = re.compile(r"\b(?:cc|ch|c)\s?\d{1,2}\b", re.IGNORECASE)
#: Discord ids, phone numbers, item counts -- long digit runs are not times.
_LONG_NUMBER_RE = re.compile(r"\b\d{5,}\b")
#: Prices: "$200" is not 2 a.m.
_PRICE_RE = re.compile(r"[$\u00a3\u20ac]\s?\d+(?:[.,]\d+)?")
_MENTION_RE = re.compile(r"<@!?(\d+)>")
_URL_RE = re.compile(r"https?://\S+")
_EMOJI_ID_RE = re.compile(r"<a?:\w+:\d+>")

_MERIDIEM = r"(?:a\.?m\.?|p\.?m\.?)"

_TIME_PATTERNS: tuple[re.Pattern[str], ...] = (
    # 1030~11+pm / 8~1130 / 9-10pm  (a range; the start is what matters)
    re.compile(
        rf"\b\d{{1,2}}(?:[:.]?\d{{2}})?\s*\+?\s*[~\-]\s*\d{{1,4}}\s*\+?\s*{_MERIDIEM}?",
        re.IGNORECASE,
    ),
    # 9:30pm / 9.30 pm / 9 pm / 9+pm / 12am
    re.compile(rf"\b\d{{1,2}}(?:[:.]\d{{2}})?\s*\+?\s*{_MERIDIEM}", re.IGNORECASE),
    # 21:30 / 9:30 / 9.30
    re.compile(r"\b\d{1,2}[:.]\d{2}\b"),
    # 1130pm / 930 pm
    re.compile(rf"\b\d{{3,4}}\s*\+?\s*{_MERIDIEM}", re.IGNORECASE),
    # "at 11", "at 9"
    re.compile(r"\bat\s+\d{1,2}\b", re.IGNORECASE),
    # bare compact 930 / 1030 / 2130 -- validated below
    re.compile(r"\b\d{3,4}\b"),
)


def _plausible_compact(digits: str) -> bool:
    """``930`` -> yes, ``290`` -> no (minute 90), ``2026`` -> no (a year)."""
    hour, minute = int(digits[:-2]), int(digits[-2:])
    if minute > 59 or hour > 23:
        return False
    if len(digits) == 4 and 1900 <= int(digits) <= 2100:
        return False  # a year; nobody writes 8pm as "2000"
    return True


def _mask(text: str) -> str:
    """Blank out things that look numeric but never mean a time."""
    text = _URL_RE.sub(" ", text or "")
    text = _EMOJI_ID_RE.sub(" ", text)
    text = _MENTION_RE.sub(" ", text)
    text = _CHANNEL_REF_RE.sub(" ", text)
    text = _PRICE_RE.sub(" ", text)
    return _LONG_NUMBER_RE.sub(" ", text)


def find_times(text: str) -> list[str]:
    """Clock expressions in ``text``, as the literal substrings that were written."""
    masked = _mask(text)
    spans: list[tuple[int, int, str]] = []
    for pattern in _TIME_PATTERNS:
        for match in pattern.finditer(masked):
            found = match.group(0).strip()
            if pattern is _TIME_PATTERNS[-1] and not _plausible_compact(found):
                continue
            if any(start <= match.start() < end for start, end, _ in spans):
                continue  # already covered by an earlier, more specific pattern
            spans.append((match.start(), match.end(), found))
    spans.sort()
    return [text_ for _, _, text_ in spans]


# ---------------------------------------------------------------------------
# days, verbs, answers
# ---------------------------------------------------------------------------

DAY_WORDS: frozenset[str] = frozenset(
    {
        "mon", "monday", "tue", "tues", "tuesday", "wed", "weds", "wednesday",
        "thu", "thur", "thurs", "thursday", "fri", "friday", "sat", "saturday",
        "sun", "sunday",
        "tmr", "tmrw", "tomorrow", "tomorow", "tonight", "tonite", "tnite",
        "today", "ytd", "yesterday",
    }
)  # fmt: skip

#: Real, but so common in this chat ("cc6 later", "i am free now") that treating
#: them as a trigger would wake the model for most of the corpus.
SOON_WORDS: frozenset[str] = frozenset({"now", "later", "ltr", "l8r", "soon"})

SCHEDULE_VERBS: frozenset[str] = frozenset(
    {
        "amend", "amended", "shift", "shifted", "change", "changed", "chg",
        "postpone", "postponed", "reschedule", "rescheduled", "resched",
        "cancel", "cancelled", "canceled", "skip", "skipping", "otot",
        "fixed", "schedule", "scheduled", "lockin", "delay", "delayed",
        "push", "swap", "reminder", "temp", "sub", "split", "arrange", "organise",
    }
)  # fmt: skip

#: Weaker than :data:`SCHEDULE_VERBS`: "run" and friends appear in every other
#: message here, so they only count next to something else.
ACTIVITY_VERBS: frozenset[str] = frozenset({"run", "runs", "clear", "bossing", "prac"})

AGREE_WORDS: frozenset[str] = frozenset(
    {
        "can", "cancan", "ok", "okay", "oke", "okie", "okei", "okey", "ogei",
        "sure", "ya", "yaya", "yea", "yeah", "ye", "yes", "yup", "yupp", "yep",
        "confirm", "confirmed", "cfm", "no", "nope", "kenot", "cannot", "cant",
        "cmi", "bobian",
    }
)  # fmt: skip

_RSVP_NO = frozenset({"no", "nope", "kenot", "cannot", "cant", "cmi", "bobian"})
_RSVP_YES = AGREE_WORDS - _RSVP_NO
_RSVP_OTHER_ACTIONS = frozenset({"cover", "take"})

_HERE_RE = re.compile(r"@(?:here|everyone)\b", re.IGNORECASE)
#: "lock in" is two words; normalise it so ``lockin`` in SCHEDULE_VERBS catches it.
_LOCK_IN_RE = re.compile(r"\block\s+in\b", re.IGNORECASE)


def _tokens(text: str) -> list[str]:
    return _WORD_RE.findall(_LOCK_IN_RE.sub("lockin", (text or "").lower()))


def find_days(text: str) -> list[str]:
    """Weekday and relative-day words, in order, de-duplicated."""
    out: list[str] = []
    for token in _tokens(_mask(text)):
        if token in DAY_WORDS and token not in out:
            out.append(token)
    return out


def find_mentions(text: str, roster_ids: Collection[str] = ()) -> list[str]:
    """``<@id>`` mentions of roster members (all mentions when no roster is given)."""
    known = {str(uid) for uid in roster_ids}
    out: list[str] = []
    for match in _MENTION_RE.finditer(text or ""):
        uid = match.group(1)
        if (not known or uid in known) and uid not in out:
            out.append(uid)
    return out


def explicit_rsvp(text: str) -> str | None:
    """Return a short, unambiguous attendance answer."""
    tokens = _tokens(_mask(text))
    if (
        not tokens
        or len(tokens) > 8
        or "?" in (text or "")
        or "anot" in tokens
        or set(tokens) & (SCHEDULE_VERBS | _RSVP_OTHER_ACTIONS)
    ):
        return None
    if set(tokens) & _RSVP_NO:
        return "no"
    if set(tokens) & _RSVP_YES:
        return "yes"
    return None


# ---------------------------------------------------------------------------
# the gate itself
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class GateResult:
    """What the gate saw in one message."""

    signals: frozenset[str] = frozenset()
    bosses: tuple[BossHit, ...] = ()
    times: tuple[str, ...] = ()
    days: tuple[str, ...] = ()
    mentions: tuple[str, ...] = field(default=())

    @property
    def hit(self) -> bool:
        """Worth keeping in the burst and showing to the model."""
        return bool(self.signals)

    @property
    def strong(self) -> bool:
        """Worth waking the model for on its own."""
        return bool(self.signals & STRONG_SIGNALS)

    @property
    def reasons(self) -> str:
        return ",".join(sorted(self.signals)) or "-"


def evaluate(text: str, table: Any, roster_ids: Collection[str] = ()) -> GateResult:
    """Score one message.  Pure: no I/O, no model, no database."""
    text = text or ""
    bosses = tuple(find_bosses(text, table))
    times = tuple(find_times(text))
    days = tuple(find_days(text))
    mentions = tuple(find_mentions(text, roster_ids))
    tokens = set(_tokens(_mask(text)))

    signals: set[str] = set()
    if bosses:
        signals.add("boss")
    if times:
        signals.add("time")
    if days:
        signals.add("day")
    if tokens & SOON_WORDS:
        signals.add("soon")
    if tokens & SCHEDULE_VERBS:
        signals.add("verb")
    if tokens & ACTIVITY_VERBS:
        signals.add("run")
    if tokens & AGREE_WORDS:
        signals.add("agree")
    if _HERE_RE.search(text):
        signals.add("here")
    if mentions:
        signals.add("mention")

    return GateResult(
        signals=frozenset(signals), bosses=bosses, times=times, days=days, mentions=mentions
    )


def should_extract(burst: Collection[GateResult], context_is_scheduling: bool = False) -> bool:
    """Is this burst worth one model call?

    Yes when any message in it is a strong hit.  A burst of nothing but answers
    ("Can", "Ok", "kenot") is worth a call *only* if the channel was talking
    about scheduling recently -- otherwise "ok" alone would wake a 13 GB model.
    """
    results = list(burst)
    if any(r.strong for r in results):
        return True
    return context_is_scheduling and any(r.hit for r in results)


__all__ = [
    "AGREE_WORDS",
    "DAY_WORDS",
    "SCHEDULE_VERBS",
    "STRONG_SIGNALS",
    "BossHit",
    "GateResult",
    "canonical_bosses",
    "evaluate",
    "explicit_rsvp",
    "find_bosses",
    "find_days",
    "find_mentions",
    "find_times",
    "should_extract",
]
