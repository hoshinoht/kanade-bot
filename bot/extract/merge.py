"""Collapsing one burst's amendments into one candidate per affected run.

DESIGN.md §2b.2: "the extractor emits the pieces per message; Python merges them
into one candidate amendment per affected run: latest explicit value wins per
field".  A three-message burst --

    [401] we doing our nstar and ncarl tonight?
    [402] 9pm i reach kk early
    [403] Aiyo amend to 9:45pm

-- is one new run, not three.  The model reliably emits a piece per message, so
the merge happens here where it is deterministic and testable rather than being
asked of a 20B model.

Merging is deliberately conservative: only amendments of the same ``kind`` about
the same target are folded together, ``rsvp`` is keyed by the person answering
(two people agreeing are two answers), and the *latest* message wins each field
so "amend to 9:45pm" beats the "9pm" before it.
"""

from __future__ import annotations

from collections.abc import Collection, Sequence

from .schema import Amendment

#: Fields taken from the latest amendment that actually states them.
_LATEST_WINS = ("day_ref", "time_ref", "rsvp", "target_run_hint")


def _order(amendment: Amendment, positions: dict[str, int]) -> int:
    """How late in the burst this amendment's evidence is."""
    return max((positions.get(str(m), -1) for m in amendment.evidence_message_ids), default=-1)


def _key(amendment: Amendment) -> tuple:
    """What makes two amendments the same candidate."""
    if amendment.kind == "rsvp":
        # One answer per person; a second "ok" from the same person is the same answer.
        return ("rsvp", tuple(sorted(amendment.participants)))
    return (amendment.kind, tuple(sorted(amendment.bosses)), amendment.target_run_hint or "")


def merge(
    amendments: Sequence[Amendment],
    message_order: Sequence[str] = (),
    existing_bosses: Collection[Sequence[str]] = (),
) -> list[Amendment]:
    """Fold a burst's amendments into one per affected run, latest value winning.

    ``message_order`` is the burst's message ids oldest-first; it is what makes
    "latest" mean anything.  Without it the input order is used.

    ``existing_bosses`` is the boss list of every run already scheduled in the
    channel.  It is what lets a `move` about a run that does not exist yet be
    recognised as part of the `add` proposing it -- see
    :func:`_fold_time_changes_into_adds`.
    """
    positions = {str(mid): index for index, mid in enumerate(message_order)}
    groups: dict[tuple, list[tuple[int, int, Amendment]]] = {}
    for index, amendment in enumerate(amendments):
        groups.setdefault(_key(amendment), []).append(
            (_order(amendment, positions), index, amendment)
        )

    out: list[Amendment] = []
    for members in groups.values():
        members.sort(key=lambda triple: (triple[0], triple[1]))
        newest = members[-1][2]
        merged = newest.model_copy(deep=True)

        for field in _LATEST_WINS:
            value = next(
                (
                    getattr(entry[2], field)
                    for entry in reversed(members)
                    if getattr(entry[2], field) is not None
                ),
                None,
            )
            setattr(merged, field, value)

        # Bosses, participants and evidence are unions: each message contributes.
        merged.bosses = _union(members, "bosses")
        merged.participants = _union(members, "participants")
        merged.evidence_message_ids = _union(members, "evidence_message_ids")
        # A burst that ends on an answer is no longer an open question; one that
        # ends on a question still is.
        merged.is_question = newest.is_question
        merged.confidence = max(entry[2].confidence for entry in members)
        out.append(merged)

    out = _fold_time_changes_into_adds(out, positions, existing_bosses)
    out = _carry_time_across_moves(out)
    # Keep the caller's ordering stable: earliest evidence first.
    out.sort(key=lambda a: _order(a, positions))
    return out


def _matches_an_existing_run(
    amendment: Amendment, existing_bosses: Collection[Sequence[str]]
) -> bool:
    wanted = {str(b) for b in amendment.bosses}
    if not wanted:
        # No bosses named: it could be about anything already scheduled.
        return bool(existing_bosses)
    return any(wanted & {str(b) for b in bosses} for bosses in existing_bosses)


def _fold_time_changes_into_adds(
    candidates: list[Amendment],
    positions: dict[str, int],
    existing_bosses: Collection[Sequence[str]],
) -> list[Amendment]:
    """A `move` about a run that does not exist yet is part of the `add` for it.

    Real bursts read "we doing our nstar and ncarl tonight?" -> "9pm i reach kk early"
    -> "Aiyo amend to 9:45pm", and the model calls the third message a `move`
    because it is one, grammatically. But there is no NStar run to move: it is
    the same proposal, settling on a time. Folding it here means one card at
    21:45 rather than a new run at 21:00 plus a move nothing can apply to.

    Only later time changes are folded, and only onto an `add` they share a boss
    with -- a `move` that does match a scheduled run is left alone.
    """
    adds = [a for a in candidates if a.kind == "add"]
    if not adds:
        return candidates

    keep: list[Amendment] = []
    for amendment in candidates:
        if amendment.kind != "move" or _matches_an_existing_run(amendment, existing_bosses):
            keep.append(amendment)
            continue
        target = next(
            (
                add
                for add in adds
                if {str(b) for b in add.bosses} & {str(b) for b in amendment.bosses}
                and _order(amendment, positions) >= _order(add, positions)
            ),
            None,
        )
        if target is None:
            keep.append(amendment)
            continue
        for field in ("day_ref", "time_ref"):
            value = getattr(amendment, field)
            if value is not None:
                setattr(target, field, value)
        for mid in amendment.evidence_message_ids:
            if mid not in target.evidence_message_ids:
                target.evidence_message_ids.append(mid)
        for uid in amendment.participants:
            if uid not in target.participants:
                target.participants.append(uid)
        target.confidence = max(target.confidence, amendment.confidence)
        # The proposal is settled by the later, more specific message.
        target.is_question = amendment.is_question
    return keep


def _day_key(amendment: Amendment) -> str | None:
    ref = (amendment.day_ref or "").strip().lower()
    return ref or None


def _carry_time_across_moves(candidates: list[Amendment]) -> list[Amendment]:
    """One time stated for a day applies to every run moved to that day.

    "mon and tuesday suddenly got things on can change to wed?" then "Wed i
    free from 9:30pm" is two moves and one time: the time was said once because
    it is the same evening for both. Without this the second run reads
    "Wed - time TBD" and the card asks a question the thread already answered.

    Only when the day is stated the same way for both and **exactly one** time
    was given for it -- two different times mean the thread has not settled, and
    guessing which applies to which run would be inventing a decision. The
    borrowed evidence is cited on the run that borrowed it.
    """
    moves = [a for a in candidates if a.kind == "move"]
    by_day: dict[str, list[Amendment]] = {}
    for amendment in moves:
        day = _day_key(amendment)
        if day is not None:
            by_day.setdefault(day, []).append(amendment)

    for group in by_day.values():
        stated = [a for a in group if a.time_ref]
        missing = [a for a in group if not a.time_ref]
        if len(stated) != 1 or not missing:
            continue
        source = stated[0]
        for amendment in missing:
            amendment.time_ref = source.time_ref
            amendment.is_question = amendment.is_question and source.is_question
            for mid in source.evidence_message_ids:
                if mid not in amendment.evidence_message_ids:
                    amendment.evidence_message_ids.append(mid)
    return candidates


def _union(members: list[tuple[int, int, Amendment]], field: str) -> list[str]:
    seen: list[str] = []
    for _, _, amendment in members:
        for value in getattr(amendment, field):
            if value not in seen:
                seen.append(value)
    return seen


__all__ = ["merge"]
