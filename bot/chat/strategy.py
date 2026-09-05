"""Deterministic routing for checked-in boss strategy knowledge."""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Literal

from bot.domain.bosses import BossParseError, BossReference, BossTable

STRATEGY_CLARIFICATION_REPLY = (
    "I can only give checked-in strategy notes for a clear boss from our catalog. "
    "Which boss do you mean?"
)
STRATEGY_NARROW_REPLY = "I can cover up to three bosses at once—please narrow it down."

_STRONG_CUE_RE = re.compile(
    r"\b(?:attacks?|beat|defeat|guide|mechanics?|moves|patterns?|phase|gauge|parry|"
    r"dodge|requirements?|strategy|survive|survival|tips?)\b",
    re.IGNORECASE,
)
_WATCH_CUE_RE = re.compile(r"\bwatch\s+(?:out\s+)?for\b", re.IGNORECASE)
_HOW_TO_MECHANICS_RE = re.compile(
    r"\bhow\s+(?:(?:do|can|should)\s+(?:\w+\s+){0,3}|to\s+(?:\w+\s+){0,2})"
    r"(?:clear|fight|handle|approach)\b",
    re.IGNORECASE,
)
_TARGET_SPLIT_RE = re.compile(r"\s*(?:,|;|/|&|\+|\||•|\bvs\.?\b|\band\b)\s*", re.IGNORECASE)
#: Discord markup must not split targets: a role mention `<@&id>` contains `&`.
_MENTION_RE = re.compile(r"<[@#][&!#]?\d+>|<a?:\w+:\d+>")
_ACTION_CONTINUATION_RE = re.compile(
    r"^(?:avoid|parry|dodge|survive|handle|deal\s+with|learn|understand|reveal|ignore)\b",
    re.IGNORECASE,
)
_POLITE_SUFFIX_RE = re.compile(r"^(?:please|pls|thank(?:s|\s+you))$", re.IGNORECASE)


@dataclass(frozen=True)
class StrategyIntent:
    """A pure routing decision for one member message."""

    kind: Literal["not_strategy", "resolved", "unresolved"]
    references: tuple[BossReference, ...] = ()
    reply: str | None = None

    @property
    def is_strategy(self) -> bool:
        return self.kind != "not_strategy"


def _has_strategy_cue(text: str) -> bool:
    return bool(
        _STRONG_CUE_RE.search(text)
        or _WATCH_CUE_RE.search(text)
        or _HOW_TO_MECHANICS_RE.search(text)
    )


def _coordinated_segments(text: str, table: BossTable) -> list[str] | None:
    """Return explicitly coordinated targets, when the wording supplies them.

    An action continuation such as ``beat FA and dodge`` is one target plus an
    action, not an unsupported second boss. A connector before the first named
    boss joins question wording (``mechanics and requirements does FA have``),
    not targets. Other conjunctions after a named boss are deliberately treated
    as a target list: partial retrieval is worse than asking the member to
    clarify.
    """
    cleaned = _MENTION_RE.sub(" ", text)
    pieces = _TARGET_SPLIT_RE.split(cleaned)
    segments = [
        piece.strip(" ?!.'\"`*()[]{}") for piece in pieces if piece.strip(" ?!.'\"`*()[]{}")
    ]
    if len(segments) < 2:
        return None
    if _POLITE_SUFFIX_RE.fullmatch(segments[-1]):
        segments.pop()
    while len(segments) > 1 and _ACTION_CONTINUATION_RE.match(segments[-1]):
        segments.pop()
    if len(segments) < 2:
        return None
    try:
        table.resolve_reference(segments[0])
    except BossParseError:
        return None
    return segments


def _add_reference(found: list[BossReference], reference: BossReference) -> bool:
    """Add a boss once, retaining an explicitly supplied difficulty.

    ``FA, HFA`` is one boss and should retain Hard rather than quietly discard
    the more precise occurrence.  Two incompatible explicit difficulties are
    ambiguous and return ``False``.
    """
    for index, existing in enumerate(found):
        if existing.short != reference.short:
            continue
        if existing.difficulty is None and reference.difficulty is not None:
            found[index] = reference
        return existing.difficulty in (None, reference.difficulty) or reference.difficulty is None
    found.append(reference)
    return True


def _resolve_segments(segments: list[str], table: BossTable) -> list[BossReference] | None:
    found: list[BossReference] = []
    for segment in segments:
        try:
            reference = table.resolve_reference(segment)
        except BossParseError:
            return None
        if not _add_reference(found, reference):
            return None
    return found


def route_strategy_intent(text: str, table: BossTable) -> StrategyIntent:
    """Route clear mechanics questions without sending untrusted names to a tool.

    The catalog remains the sole resolver.  This function only decides whether a
    message calls for guide grounding and, if so, returns canonical references.
    """
    if not _has_strategy_cue(text or ""):
        return StrategyIntent("not_strategy")

    segments = _coordinated_segments(text, table)
    references = _resolve_segments(segments, table) if segments is not None else None
    if segments is None:
        try:
            references = [table.resolve_reference(text)]
        except BossParseError:
            references = None
    if not references:
        return StrategyIntent("unresolved", reply=STRATEGY_CLARIFICATION_REPLY)
    if len(references) > 3:
        return StrategyIntent("unresolved", reply=STRATEGY_NARROW_REPLY)
    return StrategyIntent("resolved", tuple(references))


__all__ = [
    "STRATEGY_CLARIFICATION_REPLY",
    "STRATEGY_NARROW_REPLY",
    "StrategyIntent",
    "route_strategy_intent",
]
