"""Deciding which run an extracted amendment is about (DESIGN.md §1).

The bot is guild-wide and has no concept of "the" party, so an amendment is
matched to a run by **bosses ∩ participants**, scoped to the channel it was said
in.  "we doing our nstar tonight" in ``#nstar-kanon-nova`` resolves to that
channel's NMaleficStar run, not to somebody else's.

Only the general channel -- one with no runs of its own -- falls back to
guild-wide matching, and there the participant overlap has to carry the decision
on its own.
"""

from __future__ import annotations

from collections.abc import Collection, Sequence
from dataclasses import dataclass, field
from datetime import date
from zoneinfo import ZoneInfo

from bot.domain.ids import canonical as canonical_id
from bot.domain.ids import short_id

#: Runs in these statuses are never the target of a chat amendment.
DEAD_STATUSES = ("cancelled", "done")

#: A `move`/`cancel`/`otot`/`split`/`sub` needs a run; an `add` may create one and
#: a `fix` creates a fixed timing instead, so neither has to match anything.
NEEDS_RUN = ("move", "cancel", "otot", "split", "sub", "rsvp")


#: ``reason_code`` for "the amendment named bosses, and nothing here runs them".
#: The caller acts on this rather than on the prose reason: with a day and time
#: it becomes an `add`, without one it is dropped.
NO_BOSS_OVERLAP = "no-boss-overlap"


def starts_after(run: dict, day: date | None, tz: ZoneInfo) -> bool:
    """True when ``run``'s boss week begins after ``day``.

    Such a run cannot be what an amendment about ``day`` means: the night it is
    talking about had already happened before that run's week began.
    """
    week = run.get("week_start")
    if day is None or week is None:
        return False
    return week.astimezone(tz).date() > day


def reachable(runs: Sequence[dict], day: date | None, tz: ZoneInfo) -> list[dict]:
    """The runs an amendment about ``day`` is allowed to be about.

    The rule is deliberately one-directional. Moving a run *forward* past the
    reset is ordinary -- "shift our monday run to next friday" is a real request,
    and ``_record`` files the row under the week the new time lands in. Reaching
    *backwards* is not: it is how "move HMaleficStar+HFA to Wed" ended up proposing to
    drag next week's freshly-materialised Monday runs back to a Wednesday that
    belongs to the week before them. The thread never mentioned those runs; they
    merely had the right bosses and the right people on them.

    A run with no ``week_start`` is left in -- a caller that assembled the dict
    by hand has told us nothing about which week it belongs to.
    """
    return [run for run in runs if not starts_after(run, day, tz)]


@dataclass(frozen=True)
class MatchResult:
    """The chosen run (if any), why, and what else was in the running."""

    run: dict | None = None
    reason: str = "no candidates"
    candidates: tuple[dict, ...] = field(default=())
    ambiguous: bool = False
    #: A machine-readable form of ``reason`` for the cases callers branch on.
    reason_code: str = ""

    @property
    def matched(self) -> bool:
        return self.run is not None


def _people(amendment, author_id: str | None, mentioned: Collection[str]) -> set[str]:
    who = {str(p) for p in (amendment.participants or [])}
    who.update(str(m) for m in mentioned)
    if author_id:
        who.add(str(author_id))
    return who


def _score(run: dict, bosses: set[str], people: set[str]) -> tuple[int, int]:
    """``(boss overlap, participant overlap)`` -- higher is a better match."""
    return (
        len(bosses & set(run["bosses"])),
        len(people & {str(p) for p in run["participants"]}),
    )


def _hinted(hint: str | None, runs: Sequence[dict], bosses: set[str]) -> dict | None:
    """The run the model pointed at with ``target_run_hint``, if it is real.

    A hint is still only a hint. It is refused when the amendment names bosses
    the hinted run does not have: a 20B model will happily point a `move` about
    NBaldrix at an HMaleficStar run it saw in the prompt, and following that renames
    somebody else's night.
    """
    if not hint:
        return None
    prefix = canonical_id(hint)
    if len(prefix) < 4:
        return None
    for run in runs:
        if not canonical_id(run["id"]).startswith(prefix):
            continue
        if bosses and not (bosses & {str(b) for b in run["bosses"]}):
            return None
        return run
    return None


def match_run(
    amendment,
    channel_runs: Sequence[dict],
    guild_runs: Sequence[dict] = (),
    author_id: str | None = None,
    mentioned: Collection[str] = (),
) -> MatchResult:
    """Pick the run ``amendment`` is about.

    ``channel_runs`` are the runs whose home channel is the one the messages were
    posted in, for the weeks the amendment could touch (this one, and next when
    the day reference points past the reset).  ``guild_runs`` is only consulted
    when the channel has none of its own.

    Ties are broken by participant overlap with the author and anyone they
    mentioned; a tie that survives that is reported as ``ambiguous`` so the
    caller can post a question instead of guessing.
    """
    scoped = [r for r in channel_runs if r["status"] not in DEAD_STATUSES]
    wide = False
    if not scoped:
        scoped = [r for r in guild_runs if r["status"] not in DEAD_STATUSES]
        wide = True
    if not scoped:
        return MatchResult(reason="no runs to match against")

    bosses = {str(b) for b in (amendment.bosses or [])}
    people = _people(amendment, author_id, mentioned)

    hinted = _hinted(getattr(amendment, "target_run_hint", None), scoped, bosses)
    if hinted is not None:
        return MatchResult(hinted, f"model pointed at #{short_id(hinted['id'])}", tuple(scoped))

    scored = sorted(
        ((_score(run, bosses, people), run) for run in scoped),
        key=lambda pair: pair[0],
        reverse=True,
    )
    best_score, best = scored[0]

    if bosses and best_score[0] == 0:
        # Named bosses, none of which any run here has. Participant overlap is
        # NOT enough -- everyone in a party channel is on everything in it, so
        # matching on people alone points a change at whatever run is handy.
        return MatchResult(
            reason="no run here has those bosses",
            candidates=tuple(scoped),
            reason_code=NO_BOSS_OVERLAP,
        )
    if not bosses:
        # No bosses named ("change to wed?", "Can"). One run in the channel is
        # unambiguous; several are not unless the people pick one out.
        if wide and best_score[1] == 0:
            return MatchResult(reason="guild-wide, and nobody named matches", candidates=())
        if len(scoped) == 1:
            return MatchResult(scoped[0], "the only run in this channel", tuple(scoped))
        if best_score[1] == 0:
            return MatchResult(
                reason="no bosses named and no participant overlap", candidates=tuple(scoped)
            )

    rivals = [run for score, run in scored if score == best_score]
    if len(rivals) > 1:
        return MatchResult(
            rivals[0],
            f"{len(rivals)} runs match equally well",
            tuple(scoped),
            ambiguous=True,
        )
    reason = f"bosses {best_score[0]}, participants {best_score[1]}"
    return MatchResult(best, reason + (" (guild-wide)" if wide else ""), tuple(scoped))


def runs_spanned(
    amendment, channel_runs: Sequence[dict], author_id: str | None = None
) -> list[dict]:
    """Every run in the channel the amendment's bosses reach, in schedule order.

    "mon and tuesday i got stuff on, find temp for me this week?" is one sentence
    about two runs. `match_run` has to pick one -- it exists to answer "which
    run" -- so the caller uses this instead when a kind can legitimately apply to
    several at once, and gets one candidate per run rather than one card that
    silently drops half the request.
    """
    live = [r for r in channel_runs if r["status"] not in DEAD_STATUSES]
    wanted = {str(b) for b in (amendment.bosses or [])}
    if not wanted:
        return []
    hits = [r for r in live if wanted & {str(b) for b in r["bosses"]}]
    if author_id is not None:
        # Only runs they are actually on: naming a boss does not put someone on
        # somebody else's run of it.
        mine = [r for r in hits if str(author_id) in [str(p) for p in r["participants"]]]
        if mine:
            return mine
    return hits


def needs_run(kind: str) -> bool:
    """True when this kind of amendment is meaningless without a target run."""
    return kind in NEEDS_RUN


__all__ = [
    "DEAD_STATUSES",
    "NEEDS_RUN",
    "NO_BOSS_OVERLAP",
    "MatchResult",
    "match_run",
    "needs_run",
    "reachable",
    "runs_spanned",
    "starts_after",
]
