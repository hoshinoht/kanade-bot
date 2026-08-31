"""Per-channel buffering, and the whole extraction flow on flush.

Chat is bursty (DESIGN.md §4, "Batching"), so messages are buffered per channel
and one model call covers the burst: after ``EXTRACT_DEBOUNCE_SECONDS`` of
silence, or immediately when a message carries a mention/@here *and* a boss or a
time, which is what an "@here 9:30 later tonight" looks like.

On flush the deterministic stages run in order -- merge the model's per-message
pieces into one candidate per run, resolve ``day_ref``/``time_ref`` against the
last evidence message, match each candidate to a run -- and then:

* an ``rsvp`` is applied straight away through the same code the ✅/❌ reactions
  use (``source="chat"``), because it records an opinion rather than changing a
  schedule;
* everything else becomes a ``proposed`` amendment row and one card per burst,
  which a participant has to ✅ before anything is written.

Anything below ``EXTRACT_MIN_CONFIDENCE`` is logged and never posted.
"""

from __future__ import annotations

import asyncio
import logging
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field, replace
from datetime import date, datetime, timedelta
from datetime import time as clock_time
from typing import TYPE_CHECKING, Any
from zoneinfo import ZoneInfo

from .. import formatting, pings
from ..materialise import RUN_DONE_AFTER
from ..rsvp import apply_reaction
from ..timeutil import utcnow
from ..watch import origin_ids
from ..weeks import week_end, week_start
from . import gate
from . import prompt as prompt_mod
from .commit import supersede
from .llm import Extractor
from .match import NO_BOSS_OVERLAP, match_run, needs_run, reachable, runs_spanned
from .merge import merge
from .resolve import Resolved, resolve
from .schema import Amendment, Extraction
from .window import (
    DEFAULT_WINDOW,
    clamp_window,
    group_for_rescan,
    previous_week_start,
    should_widen,
    split_until,
    window_since,
)

if TYPE_CHECKING:  # pragma: no cover
    from ..client import BossBot

log = logging.getLogger(__name__)

#: How far back a burst may look for context, and how "was this channel talking
#: about scheduling" is decided for a burst of nothing but "ok"/"can".
CONTEXT_WINDOW_HOURS = 48

#: Kinds that can legitimately be about several runs at once, and are split into
#: one candidate per run rather than being forced onto the best single match.
#: "find temp for me for mon and tues" is one sentence and two stand-ins, and
#: DESIGN.md §8 reads "mon and tuesday suddenly got things on can change to
#: wed?" as two moves. The model is asked for one amendment per run (prompt rule
#: 6) and mostly obliges; when it does not, forcing four bosses onto whichever
#: run scored highest silently threw the other run's half away.
#: `split` is absent because it already changes one run and creates another, and
#: `rsvp` because an answer belongs to the run it answers about.
SPLIT_ACROSS_RUNS = frozenset({"sub", "move", "cancel", "otot"})

#: Kinds still worth acting on when the match was a coin toss.  What reaches
#: here after :data:`SPLIT_ACROSS_RUNS` is a tie with no boss evidence at all --
#: "change to wed?" in a channel with two runs -- and guessing there moves
#: somebody's night, so a `move`/`cancel`/`otot`/`split` is dropped instead.
#: These two are the reversible ones: an `rsvp` is an opinion, and
#: :func:`bot.rsvp.apply_reaction` ignores it outright unless the person is on
#: the run it landed on, which is most of the guesswork undone already -- a bare
#: "Can" is the commonest thing anyone says and never names a boss. A stand-in
#: on the wrong one of two equally likely runs is a `/swap` away.
ACT_ON_AMBIGUOUS = frozenset({"sub", "rsvp"})

#: Kinds that become an `add` when the channel has no run with those bosses and
#: the thread did settle on a day and time.
CONVERTS_TO_ADD = frozenset({"move", "split"})

#: How far into the past a proposed time may point before the card is pointless.
#: A rescan reads a whole boss week, so without this it would cheerfully post
#: "move HStar to Monday 21:30" on Wednesday. The grace exists because live chat
#: routinely settles a run just after it was due to start ("start now lah") and
#: that is a real amendment, not a stale one.
STALE_GRACE = timedelta(hours=3)


# ---------------------------------------------------------------------------
# the deterministic half (pure, unit tested)
# ---------------------------------------------------------------------------


@dataclass
class Planned:
    """One merged amendment, resolved and matched, ready to become a row."""

    amendment: Amendment
    resolved: Resolved
    run: dict | None = None
    payload: dict = field(default_factory=dict)
    match_reason: str = ""
    match_code: str = ""
    #: Kinds that lost to this one for the same run (see :func:`one_per_run`).
    #: Named on the card so nothing the thread said disappears silently.
    also_mentioned: list[str] = field(default_factory=list)
    #: Several runs matched this equally well, so the match is a coin toss.
    ambiguous: bool = False
    #: The model's one-line summary of the burst this came from.  Carried per
    #: entry because a rescan consolidates several bursts onto one card, and
    #: stamping the first burst's summary on all of them describes the wrong
    #: conversation.
    summary: str = ""

    @property
    def kind(self) -> str:
        return self.amendment.kind

    @property
    def is_rsvp(self) -> bool:
        return self.amendment.kind == "rsvp"

    @property
    def needs_answer(self) -> bool:
        """The card should be worded as a suggestion, not a decision."""
        return self.amendment.is_question or self.resolved.at is None


@dataclass
class Plan:
    """Everything one burst produced."""

    planned: list[Planned] = field(default_factory=list)
    dropped: list[Planned] = field(default_factory=list)
    summary: str = ""
    #: The model call behind this plan, so `/debug extract` can show its work.
    raw: str = ""
    latency_ms: int = 0
    error: str | None = None
    message_ids: list[str] = field(default_factory=list)
    amendment_ids: list[str] = field(default_factory=list)
    #: The boss week the burst's amendments belong to, set once resolved.
    week_start: datetime | None = None

    @property
    def rsvps(self) -> list[Planned]:
        return [p for p in self.planned if p.is_rsvp]

    @property
    def proposals(self) -> list[Planned]:
        return [p for p in self.planned if not p.is_rsvp]


def _merge_plans(plans: Sequence[Plan]) -> Plan:
    """Fold the pieces of one oversized burst back into a single plan."""
    return Plan(
        planned=[entry for plan in plans for entry in plan.planned],
        dropped=[entry for plan in plans for entry in plan.dropped],
        summary=next((p.summary for p in plans if p.summary), ""),
        raw="\n".join(p.raw for p in plans if p.raw),
        latency_ms=sum(p.latency_ms for p in plans),
        error=next((p.error for p in plans if p.error), None),
        message_ids=sorted({mid for plan in plans for mid in plan.message_ids}),
        week_start=next((p.week_start for p in plans if p.week_start is not None), None),
    )


def _fix_payload(resolved: Resolved) -> dict:
    """A ``fix`` becomes a weekday + HH:MM, which is what ``fixed_runs`` stores."""
    if resolved.day is None or resolved.clock is None:
        return {}
    return {"weekday": resolved.day.weekday(), "time": resolved.clock.strftime("%H:%M")}


def _payload_for(
    amendment: Amendment,
    resolved: Resolved,
    run: dict | None,
    volunteer_ids: Sequence[str] | None = None,
) -> dict:
    if amendment.kind == "fix":
        return _fix_payload(resolved)
    if amendment.kind == "split" and run is not None:
        moved = [b for b in amendment.bosses if b in run["bosses"]]
        return {
            "bosses": moved or list(amendment.bosses),
            "participants": list(amendment.participants),
        }
    if amendment.kind == "sub":
        # Whoever is asking for a temp is the one dropping out. A volunteer is
        # whoever *else* the merged amendment ended up naming -- "find temp for
        # me" then "I can take" is two messages and two different authors.
        leaving = [str(u) for u in amendment.participants]
        volunteers = [str(u) for u in (volunteer_ids or []) if str(u) not in leaving]
        return {"remove": leaving, "add": volunteers}
    return {}


#: Which change wins when a burst says several things about one run's *timing*.
#: An explicit cancel beats everything; a `move` that names a day beats "own
#: time", because naming a night is a decision and "otot" is what people say
#: when there is not one; a `move` with no day is weaker than either. Live: one
#: thread produced both `move HCarling -> Tue` and `otot HCarling`, and the move
#: was what the party meant. `sub` is deliberately absent: a stand-in is a
#: roster change, not a timing one, and "ZedRS's out, X fills, and we do Wed"
#: is one decision that needs both lines on the card.
RUN_PRECEDENCE = ("move", "otot", "move-with-day", "cancel")


def _precedence(entry: Planned) -> int:
    kind = entry.kind
    if kind == "move" and entry.resolved.day is not None:
        kind = "move-with-day"
    return RUN_PRECEDENCE.index(kind) if kind in RUN_PRECEDENCE else -1


def _claim(entry: Planned) -> tuple[bool, int]:
    """How strongly an entry claims its run: settled first, then kind.

    A question is a proposal the thread may well have talked out of; a
    statement is what the group landed on. Live: "can change to wed?" was
    pushed back on ("This Sunday can anot?", "weds later damn packed") and the thread
    settled on doing HCarling on its own time -- but `move-with-day` outranks
    `otot`, so the abandoned question took the run and the decision was demoted
    to "(also mentioned: own time)". Asking beats deciding on no reading of
    that conversation, so settledness is compared before kind.
    """
    return (not entry.amendment.is_question, _precedence(entry))


def one_per_run(entries: Sequence[Planned]) -> list[Planned]:
    """Keep one change per run; note the rest.

    A card that offers two contradictory things for the same run cannot be
    confirmed with one ✅, and the loser is not thrown away -- the winning line
    says "(also mentioned: own time)" so the reader can see what else was said.

    The winner is chosen by :func:`_claim`: a settled change beats a question
    whatever the two kinds are, :data:`RUN_PRECEDENCE` separates two of the same
    settledness, and the later evidence takes an outright tie.

    Kinds outside the precedence list (`add`, `fix`, `split`, `sub`) are left
    alone: `add`/`fix` have no run to collide over, a `split` deliberately
    changes a run *and* creates another, and a `sub` changes who goes, which
    can sit beside any timing change for the same run.
    """
    best: dict[str, Planned] = {}
    out: list[Planned] = []
    for entry in entries:
        rank = _precedence(entry)
        if entry.run is None or rank < 0:
            out.append(entry)
            continue
        run_id = entry.run["id"]
        held = best.get(run_id)
        if held is None:
            best[run_id] = entry
            out.append(entry)
            continue
        # A tie goes to `entry`, which is the later of the two: the entries
        # arrive in evidence order, and when a thread says the same *kind* of
        # thing twice about one run the second time is the group changing its
        # mind ("wed" then "no, thu"), not repeating itself.
        winner, loser = (entry, held) if _claim(entry) >= _claim(held) else (held, entry)
        if winner is entry:
            out[next(i for i, held_entry in enumerate(out) if held_entry is held)] = entry
            best[run_id] = entry
        if loser.kind not in winner.also_mentioned:
            winner.also_mentioned.append(loser.kind)
        log.info(
            "run %s: keeping %s over %s from the same burst",
            run_id[:8],
            winner.kind,
            loser.kind,
        )
    return out


def _at(day: date, clock: clock_time, tz: ZoneInfo, assumed_pm: bool) -> Resolved:
    return Resolved(
        day=day,
        clock=clock,
        at=datetime.combine(day, clock, tzinfo=tz),
        assumed_pm=assumed_pm,
    )


def inherit_from_run(entry: Planned, tz: ZoneInfo) -> Resolved:
    """Fill a `move`'s unsaid half from the run it is moving.

    "can change to wed?" states a day and no time, and the time is not unknown:
    it is the one the run already has. A card reading "Mon 21:30 -> Wed, time
    **TBD**" asks the party to re-decide something nobody proposed changing, and
    a row with no ``new_datetime`` cannot be applied at all -- ✅ on it answers
    "no new time was agreed".

    The other way round, "amend to 9:45pm" about Monday's run is about *Monday*.
    :func:`bot.extract.resolve.resolve` has no run to consult, so it anchors a
    bare clock time to the day the message was sent and rolls it forward once it
    has passed; both are wrong for a thread discussing another night. The run's
    own date replaces them, and anything that still lands in the past is
    :func:`already_passed`'s business rather than a reason to invent a day.

    This is not the guessing DESIGN.md §2b.1 forbids: every value here is read
    off the run being changed, which is evidence, not availability modelling.
    """
    run, resolved = entry.run, entry.resolved
    if entry.kind != "move" or run is None:
        return resolved
    local = run["datetime"].astimezone(tz)
    if resolved.day is not None and resolved.clock is None:
        return _at(resolved.day, local.time(), tz, resolved.assumed_pm)
    # `resolved.day` is never None once a clock parsed, so what marks "no day was
    # said" is the amendment's own empty `day_ref`, not the resolved value.
    if resolved.clock is not None and not entry.amendment.day_ref:
        return _at(local.date(), resolved.clock, tz, resolved.assumed_pm)
    return resolved


def is_no_op(entry: Planned, tz: ZoneInfo | None = None) -> bool:
    """True when applying this would change nothing at all.

    A rescan re-reads chat that has already been acted on, so it routinely
    proposes the schedule that already exists -- "Wed 21:30 -> Wed 21:30" is a
    card asking someone to confirm a decision they made days ago.

    A day-only move needs ``tz`` to answer: "move it to Wednesday" about a run
    that is already on Wednesday changes nothing, and whether it is already on
    Wednesday is a question about the guild's local calendar.
    """
    run = entry.run
    if run is None:
        return False
    if entry.kind == "add":
        # This `add` matched a run that already exists, so it is proposing a
        # night the channel is already having.
        return True
    if entry.kind == "move":
        at = entry.resolved.at
        if at is not None:
            return at == run["datetime"]
        day = entry.resolved.day
        return day is not None and tz is not None and day == run["datetime"].astimezone(tz).date()
    if entry.kind == "otot":
        return run["status"] == "otot"
    if entry.kind == "cancel":
        return run["status"] == "cancelled"
    return False


def consolidate(entries: Sequence[Planned]) -> list[Planned]:
    """One entry per thing changed, latest evidence winning.

    A rescan reads a week as several conversations, and the same run is often
    revisited in more than one of them -- Sunday proposes Wednesday, Monday
    settles the time. Posting a card per burst produced a stack of cards that
    superseded each other in the channel; this keeps the last word on each
    target and posts one.

    Keyed on the run being changed, or -- for `add`/`fix`, which have no run yet
    -- on the boss set they would create, which is the same key
    :func:`bot.extract.commit.supersede` uses.
    """
    best: dict[tuple, Planned] = {}
    for entry in entries:
        target = entry.run["id"] if entry.run else tuple(sorted(entry.amendment.bosses))
        best[(entry.kind, target)] = entry  # later bursts overwrite earlier ones
    return one_per_run(list(best.values()))


def already_passed(entry: Planned, now: datetime, tz: ZoneInfo | None = None) -> bool:
    """True when acting on this amendment would change something already over.

    Three ways that happens, and a rescan over old chat hits all of them:

    * the **day** it names is before today -- "we doing our nstar tonight?" read
      back two days later is about a night that has been and gone, and it has no
      clock time for the check below to catch;
    * the time it proposes is behind ``now`` by more than :data:`STALE_GRACE`;
    * the run it targets is finished, cancelled, or its slot has passed.

    Judged against *now* rather than the burst's anchor: the anchor is when the
    conversation happened, which during a rescan is exactly the point.
    """
    at = entry.resolved.at
    if at is not None and at < now - STALE_GRACE:
        return True
    day = entry.resolved.day
    if day is not None and tz is not None and day < now.astimezone(tz).date():
        return True
    run = entry.run
    if run is None:
        return False
    if run["status"] in ("done", "cancelled"):
        return True
    return run["datetime"] + RUN_DONE_AFTER < now


def volunteers_for(amendment: Amendment, author_ids: dict[str, str]) -> list[str]:
    """Who offered to stand in, from the authors of a `sub`'s evidence.

    The person asking for a temp writes the first message; anyone who answers
    it is offering ("I can come", "I take"). The model is not asked to work out
    who volunteered -- it cites the messages, and the author of a later one who
    is not the person leaving is the volunteer.
    """
    leaving = {str(u) for u in amendment.participants}
    out: list[str] = []
    for message_id in amendment.evidence_message_ids:
        author = author_ids.get(str(message_id))
        if author and author not in leaving and author not in out:
            out.append(author)
    return out


def plan_burst(
    extraction: Extraction,
    *,
    anchor: datetime,
    tz: ZoneInfo,
    channel_runs: list[dict],
    guild_runs: list[dict] = (),
    burst_order: list[str] = (),
    author_ids: dict[str, str] | None = None,
    min_confidence: float = 0.0,
    now: datetime | None = None,
) -> Plan:
    """Merge, resolve and match one extraction.  No database, no Discord.

    ``author_ids`` maps message id -> author id, so an amendment that names
    nobody still matches against whoever wrote the evidence.  ``now`` is what
    "already passed" is measured against; it defaults to the real now, so a
    rescan over last week's chat proposes nothing.
    """
    author_ids = author_ids or {}
    now = now or utcnow()
    merged = merge(extraction.amendments, burst_order, [r["bosses"] for r in channel_runs])
    # Bosses the burst is already proposing a new run for, so a `move` about the
    # same ones is that proposal settling rather than a second run.
    proposed_bosses = {str(b) for a in merged if a.kind in ("add", "fix") for b in (a.bosses or [])}
    plan = Plan(summary=extraction.summary)
    for amendment in merged:
        resolved = resolve(amendment.day_ref, amendment.time_ref, anchor, tz)
        author = next(
            (author_ids[m] for m in amendment.evidence_message_ids if m in author_ids), None
        )
        mentioned = list(amendment.participants)

        entries: list[Planned] = []
        # Runs whose boss week starts after the night this amendment is about
        # are not candidates for it, on any of the three paths below -- the
        # model's own hint, the best single match, or the split across runs.
        # Filtered before matching rather than after, so the *right* run can
        # still win when both weeks have one with those bosses.
        here = reachable(channel_runs, resolved.day, tz)
        guild_here = reachable(guild_runs, resolved.day, tz)

        spanned = (
            runs_spanned(amendment, here, author_id=author)
            if amendment.kind in SPLIT_ACROSS_RUNS
            else []
        )
        if len(spanned) > 1:
            # One candidate per run, each naming only that run's bosses, so the
            # card says "stand-in for HStar+HFA" and "for HCarling+XKalos"
            # instead of one line listing four bosses across two nights.
            for run in spanned:
                per_run = amendment.model_copy(deep=True)
                per_run.bosses = [b for b in run["bosses"]]
                entries.append(
                    Planned(
                        amendment=per_run,
                        resolved=resolved,
                        run=run,
                        payload=_payload_for(
                            per_run, resolved, run, volunteers_for(per_run, author_ids)
                        ),
                        match_reason=f"one of {len(spanned)} runs it spans",
                    )
                )
        else:
            result = match_run(amendment, here, guild_here, author_id=author, mentioned=mentioned)
            if result.run is None and channel_runs and not here:
                result = replace(result, reason="every run here belongs to a later boss week")
            if (
                result.run is None
                and result.reason_code == NO_BOSS_OVERLAP
                and amendment.kind in CONVERTS_TO_ADD
                and resolved.at is not None
                # A named day, not a bare time: turning "amend to 9:45" into a
                # brand new run would invent a night nobody chose.
                and amendment.day_ref
                and not (proposed_bosses & {str(b) for b in (amendment.bosses or [])})
            ):
                # Nothing here runs those bosses, but a day and a time were
                # agreed: the thread is proposing a new run, whatever grammar it
                # used. Better one `add` card than a `move` pointed at a
                # stranger's night.
                log.info(
                    "no run for %s in this channel; reading the %s as a new run",
                    "+".join(amendment.bosses),
                    amendment.kind,
                )
                amendment = amendment.model_copy(deep=True)
                amendment.kind = "add"
            entries.append(
                Planned(
                    amendment=amendment,
                    resolved=resolved,
                    run=result.run,
                    payload=_payload_for(
                        amendment, resolved, result.run, volunteers_for(amendment, author_ids)
                    ),
                    match_reason=result.reason,
                    match_code=result.reason_code,
                    ambiguous=result.ambiguous,
                )
            )

        for entry in entries:
            entry.summary = extraction.summary
            if entry.amendment.confidence < min_confidence:
                plan.dropped.append(entry)
                continue
            if needs_run(entry.kind) and entry.run is None:
                # A move/cancel/otot/split/sub/rsvp with nothing to apply it to.
                entry.match_reason = f"no run matched ({entry.match_reason})"
                plan.dropped.append(entry)
                continue
            if entry.ambiguous and entry.kind not in ACT_ON_AMBIGUOUS:
                # Two runs fit equally well and nothing in the message picks one.
                # `match_run` returns the first rival so the caller *can* guess;
                # guessing here renames somebody else's night, so it does not.
                entry.match_reason = f"ambiguous ({entry.match_reason})"
                plan.dropped.append(entry)
                continue
            if entry.kind == "add" and not entry.resolved.known and not entry.amendment.is_question:
                # A new run stated flatly, with neither a day nor a time, is not
                # schedulable and nobody asked about it -- it is the shape a
                # truncated extraction leaves behind. A *question* with no day or
                # time is DESIGN.md §8 row 4 ("wanna try trio ncarling also?")
                # and is exactly what a card asking "when?" is for.
                entry.match_reason = "a stated add with no day or time"
                plan.dropped.append(entry)
                continue
            # Before the staleness and no-op checks, both of which judge the
            # instant a move points at -- which a half-stated move only has once
            # the run has filled in the half nobody changed.
            entry.resolved = inherit_from_run(entry, tz)
            if already_passed(entry, now, tz):
                entry.match_reason = "already passed"
                plan.dropped.append(entry)
                continue
            if is_no_op(entry, tz):
                entry.match_reason = "already scheduled"
                plan.dropped.append(entry)
                continue
            if entry.kind == "add":
                # An `add` creates a run; it never edits the one its bosses
                # happened to match. Carrying a `run_id` made it supersede that
                # run's other proposals and commit against the wrong night.
                # Nulled *after* `is_no_op`, which needs the match to spot an
                # `add` for a run the channel already has.
                entry.run = None
            plan.planned.append(entry)
    # One change per run, so a card can never offer two contradictory things
    # for the same night.
    plan.planned = one_per_run(plan.planned)
    return plan


def relevant_weeks(
    anchor: datetime, tz: ZoneInfo, reset_weekday: int, reset_time
) -> tuple[datetime, datetime]:
    """``(this week, next week)`` -- an amendment may point past the reset."""
    this = week_start(anchor, tz, reset_weekday, reset_time)
    return this, week_end(this, tz)


# ---------------------------------------------------------------------------
# the buffering half
# ---------------------------------------------------------------------------


@dataclass
class RescanReport:
    """What one `/rescan` did, in the terms the ephemeral reply reports."""

    channel_id: str
    window: str
    since: datetime
    #: True when the requested week held no scheduling chat and the search was
    #: widened to the boss week before it (once, never further).
    widened: bool = False
    backfilled: int = 0
    stored: int = 0
    gated: int = 0
    bursts: int = 0
    extracted: int = 0
    proposals: int = 0
    dropped: int = 0
    stale: int = 0
    #: Rows written but left without a card, because Discord could not be
    #: reached. They are re-posted by the next :meth:`Pipeline.apply_plan`.
    unposted: int = 0
    elapsed_ms: int = 0
    cancelled: bool = False
    errors: list[str] = field(default_factory=list)
    plans: list[Plan] = field(default_factory=list)

    @property
    def asked(self) -> bool:
        """Was the model called at all?"""
        return self.extracted > 0

    @property
    def planned(self) -> list[Planned]:
        return [entry for plan in self.plans for entry in plan.planned]


@dataclass
class _Prepared:
    """One burst's prompt and the schedule it was built against."""

    messages: list[dict[str, str]]
    burst: list[prompt_mod.Msg]
    context: list[prompt_mod.Msg]
    anchor: datetime
    this_week: datetime
    channel_runs: list[dict]
    guild_runs: list[dict]


@dataclass
class Burst:
    """Messages waiting for the debounce to run out in one channel."""

    channel_id: str
    message_ids: list[str] = field(default_factory=list)
    results: list[gate.GateResult] = field(default_factory=list)
    task: asyncio.Task | None = None

    def add(self, message_id: str, result: gate.GateResult) -> None:
        if message_id not in self.message_ids:
            self.message_ids.append(message_id)
            self.results.append(result)


def urgent(result: gate.GateResult) -> bool:
    """A mention or @here plus a boss or a time: extract without waiting."""
    has_target = "here" in result.signals or "mention" in result.signals
    return has_target and bool(result.signals & {"boss", "time"})


class Pipeline:
    """Owns the buffers and drives one extraction per burst."""

    def __init__(self, bot: BossBot, extractor: Extractor | None = None):
        self.bot = bot
        self.extractor = extractor or Extractor(bot.settings)
        self._bursts: dict[str, Burst] = {}

    # -- intake ------------------------------------------------------------
    @property
    def enabled(self) -> bool:
        # `extract_enabled` is the DB-backed runtime flag (seeded from the env
        # var of the same name), so the portal can switch the model off without
        # a redeploy. `paused` is `/bot pause`.
        return bool(getattr(self.bot, "extract_enabled", True)) and not self.bot.paused

    def gate_message(self, content: str) -> gate.GateResult:
        roster = [m["user_id"] for m in self.bot.repo.list_members()]
        return gate.evaluate(content, self.bot.bosses, roster)

    async def offer(self, message: Any) -> gate.GateResult | None:
        """Called by ``on_message`` for a stored, watched, non-bot message."""
        if not self.enabled:
            return None
        if not self.bot.repo.has_role(message.author.id):
            # DESIGN.md §1: messages from non-members are ignored by the extractor.
            return None
        result = self.gate_message(message.content or "")
        if not result.hit:
            return None
        channel_id, _thread = origin_ids(message.channel)
        burst = self._bursts.setdefault(str(channel_id), Burst(channel_id=str(channel_id)))
        burst.add(str(message.id), result)
        if urgent(result):
            await self._cancel_timer(burst)
            await self.flush(str(channel_id))
        else:
            self._restart_timer(burst)
        return result

    def _restart_timer(self, burst: Burst) -> None:
        if burst.task is not None:
            burst.task.cancel()
        burst.task = asyncio.create_task(self._debounce(burst.channel_id))

    async def _cancel_timer(self, burst: Burst) -> None:
        if burst.task is not None:
            burst.task.cancel()
            burst.task = None

    async def _debounce(self, channel_id: str) -> None:
        try:
            await asyncio.sleep(self.bot.settings.extract_debounce_seconds)
            await self.flush(channel_id)
        except asyncio.CancelledError:  # pragma: no cover - a newer message arrived
            raise
        except Exception:  # pragma: no cover - a burst must never kill the bot
            log.exception("burst flush failed for channel %s", channel_id)

    async def shutdown(self) -> None:
        for burst in self._bursts.values():
            await self._cancel_timer(burst)
        await self.extractor.close()

    # -- the flow ----------------------------------------------------------
    async def flush(self, channel_id: str) -> Plan | None:
        """Extract from the buffered burst in one channel."""
        burst = self._bursts.pop(str(channel_id), None)
        if burst is None or not burst.message_ids:
            return None
        await self._cancel_timer(burst)
        if not self.enabled:
            return None

        repo = self.bot.repo
        rows = [repo.get_message(mid) for mid in burst.message_ids]
        rows = [row for row in rows if row is not None]
        if not rows:
            return None

        context_is_scheduling = self._recent_scheduling(channel_id, exclude=burst.message_ids)
        if not gate.should_extract(burst.results, context_is_scheduling):
            log.debug(
                "channel %s: burst of %d message(s) was answers only, not extracting",
                channel_id,
                len(rows),
            )
            repo.mark_messages_processed(burst.message_ids)
            return None
        return await self.extract(channel_id, rows)

    async def rescan(
        self, channel_id: int | str, hours: int = 24, post: bool = True
    ) -> Plan | None:
        """One model call over a channel's stored history for the last ``hours``.

        The narrow form, kept for callers that want a single burst and a single
        :class:`Plan`.  `/rescan` uses :meth:`rescan_window`, which backfills
        from Discord first and splits a whole week into conversations.
        """
        since = utcnow() - timedelta(hours=hours)
        gated = self._gated_since(channel_id, since)[1]
        if not gated:
            return None
        return await self.extract(str(channel_id), gated, post=post)

    def _gated_since(
        self, channel_id: int | str, since: datetime, until: datetime | None = None
    ) -> tuple[list[dict], list[dict]]:
        """``(stored rows, the ones worth a model call)`` for a window.

        Same two filters the live listener applies: only roster members count
        (DESIGN.md §1), and only messages the keyword gate hits.
        """
        rows = self.bot.repo.recent_messages(channel_id, since, until)
        rows = [r for r in rows if self.bot.repo.has_role(r["author_id"])]
        return rows, [row for row in rows if self.gate_message(row["content"]).hit]

    async def _backfill(self, channel_id: int | str, since: datetime) -> int:
        """Pull history from Discord, if there is a live client to pull it with."""
        backfill = getattr(self.bot, "backfill_channel", None)
        if backfill is None:  # pragma: no cover - only a stand-in bot lacks it
            return 0
        try:
            return await backfill(channel_id, since)
        except Exception:  # noqa: BLE001 - a rescan must still read what is stored
            log.exception("backfill failed for channel %s", channel_id)
            return 0

    async def rescan_window(
        self,
        channel_id: int | str,
        window: str = DEFAULT_WINDOW,
        post: bool = True,
        automated: bool = False,
        should_stop: Callable[[], bool] | None = None,
    ) -> RescanReport:
        """Backfill from Discord, then re-read a whole window, burst by burst.

        The order matters. Reading only the ``messages`` table finds nothing at
        all after a database reset, which is exactly when a rescan is wanted, so
        history is pulled from Discord first. A boss week of chat is then split
        back into the conversations it came from (:func:`group_bursts`) and each
        one gets its own model call, oldest first -- one prompt per week would
        be both enormous and impossible for the model to attribute.

        Calls are sequential: :data:`bot.extract.llm.MODEL_LOCK` serialises them
        anyway, and issuing them in order keeps "latest stated value wins" true
        across the week. One card is posted at the end, not one per burst -- see
        :func:`consolidate`.
        """
        started = utcnow()
        bot = self.bot
        window = clamp_window(window, automated)
        since = window_since(
            window, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, started
        )
        report = RescanReport(channel_id=str(channel_id), window=window, since=since)
        report.backfilled = await self._backfill(channel_id, since)
        rows, gated = self._gated_since(channel_id, since)

        if should_widen(window, len(gated), automated):
            # A quiet week is normal just after a Thursday reset; the useful
            # answer is last week's plan. Once only -- anything older would be
            # dropped as `already passed` regardless.
            report.widened = True
            report.since = since = previous_week_start(
                since, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time
            )
            report.backfilled += await self._backfill(channel_id, since)
            rows, gated = self._gated_since(channel_id, since)

        report.stored = len(rows)
        report.gated = len(gated)
        groups = group_for_rescan(gated, bot.tz)
        report.bursts = len(groups)

        # Each conversation gets its own model call, but the *cards* wait: a
        # week is often one decision revisited across several evenings, and a
        # card per burst left a stack of superseded cards in the channel.
        collected: list[Planned] = []
        consumed: list[str] = []
        week = None
        for group in groups:
            if should_stop is not None and should_stop():
                # Cancellation is cooperative and lands between bursts: a model
                # call in flight is left to finish rather than abandoned.
                report.cancelled = True
                break
            plan = await self.extract(str(channel_id), group, post=False)
            if plan is None:
                continue
            report.extracted += 1
            report.plans.append(plan)
            report.dropped += len(plan.dropped)
            report.stale += sum(1 for e in plan.dropped if e.match_reason == "already passed")
            if plan.error:
                report.errors.append(plan.error)
                continue
            collected.extend(plan.planned)
            consumed.extend(plan.message_ids)
            week = plan.week_start or week

        if post and collected and week is not None:
            entries = consolidate(collected)
            amendment_ids = await self.apply_plan(
                str(channel_id),
                [e for e in entries if e.is_rsvp],
                [e for e in entries if not e.is_rsvp],
                week,
                # Only the fallback: each entry carries its own burst's summary.
                next((p.summary for p in report.plans if p.summary), ""),
                report=report,
            )
            report.proposals = len(amendment_ids)
            self.bot.repo.mark_messages_processed(sorted(set(consumed)))

        report.elapsed_ms = int((utcnow() - started).total_seconds() * 1000)
        log.info(
            "rescan of channel %s (%s%s): %d backfilled, %d gated, %d burst(s), %d proposal(s)",
            channel_id,
            window,
            ", widened to the previous week" if report.widened else "",
            report.backfilled,
            report.gated,
            report.bursts,
            report.proposals,
        )
        return report

    def _recent_scheduling(self, channel_id: str, exclude: list[str]) -> bool:
        """Was this channel talking about a schedule recently?  Gates bare answers."""
        since = utcnow() - timedelta(hours=6)
        skip = set(exclude)
        for row in self.bot.repo.recent_messages(channel_id, since):
            if str(row["id"]) in skip:
                continue
            if self.gate_message(row["content"]).strong:
                return True
        return False

    def _context_rows(self, channel_id: str, burst: list[dict]) -> list[dict]:
        """The messages immediately *before* a burst, oldest first.

        Anchored to the burst, not to the wall clock: a rescan replays old
        conversations, and taking "the last 25 messages in the last 48 hours"
        fed the model chat from *after* the burst it was being asked about --
        including, on a real thread, the answers to a question it was supposed
        to be reading fresh.
        """
        if not burst:
            return []
        first = burst[0]["created_at"]
        limit = self.bot.settings.extract_context_messages
        if not limit:
            return []
        burst_ids = {str(row["id"]) for row in burst}
        rows = [
            row
            for row in self.bot.repo.recent_messages(
                channel_id, first - timedelta(hours=CONTEXT_WINDOW_HOURS), until=first
            )
            if str(row["id"]) not in burst_ids
        ]
        return rows[-limit:]

    def _name_for(self, user_id: str) -> str:
        member = self.bot.repo.get_member(user_id)
        if member:
            return member["nickname"] or member["display_name"] or str(user_id)
        return f"user{str(user_id)[-4:]}"

    def _msgs(self, rows: list[dict]) -> list[prompt_mod.Msg]:
        return [prompt_mod.Msg.from_row(row, self._name_for(row["author_id"])) for row in rows]

    def _prepare(self, channel_id: str, rows: list[dict]) -> _Prepared:
        """Everything one burst's model call needs, gathered from the database.

        Split out so the budget check in :meth:`fit_to_budget` measures the
        prompt that would actually be sent rather than an approximation of it.
        """
        bot, repo = self.bot, self.bot.repo
        burst = self._msgs(rows)
        context = self._msgs(self._context_rows(channel_id, rows))
        anchor = burst[-1].created_at

        this_week, next_week = relevant_weeks(
            anchor, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time
        )
        channel_runs = [
            run
            for week in (this_week, next_week)
            for run in repo.list_runs(week_start=week, channel_id=channel_id)
        ]
        guild_runs = [
            run for week in (this_week, next_week) for run in repo.list_runs(week_start=week)
        ]
        fixed_runs = [f for f in repo.list_fixed_runs() if f["channel_id"] == str(channel_id)]

        context_obj = prompt_mod.PromptContext(
            tz=bot.tz,
            table=bot.bosses,
            burst=burst,
            context=context,
            runs=channel_runs,
            fixed_runs=fixed_runs,
            roster=repo.list_members(),
            channel_name=self._channel_name(channel_id),
            guild_runs=guild_runs,
        )
        return _Prepared(
            messages=prompt_mod.build_messages(context_obj),
            burst=burst,
            context=context,
            anchor=anchor,
            this_week=this_week,
            channel_runs=channel_runs,
            guild_runs=guild_runs,
        )

    def fit_to_budget(self, channel_id: str, rows: list[dict]) -> list[list[dict]]:
        """Cut ``rows`` into pieces whose prompts each fit the context window.

        :data:`bot.extract.window.MAX_BURST_MESSAGES` caps a burst by message
        count, which is only a guess: twelve long messages in a channel with
        four runs and a full roster still overrun. This measures the real
        prompt, so nothing is ever sent that leaves the model no room to answer.
        """
        budget = prompt_mod.prompt_budget(self.bot.settings.ollama_num_ctx)

        def fits(chunk: list[dict]) -> bool:
            return prompt_mod.estimate_messages(self._prepare(channel_id, chunk).messages) <= budget

        return split_until(rows, fits)

    async def extract(self, channel_id: str, rows: list[dict], post: bool = True) -> Plan | None:
        """Prompt -> model -> resolve/match -> rows + one card.  Never raises."""
        try:
            chunks = self.fit_to_budget(channel_id, rows)
            if len(chunks) == 1:
                return await self._extract(channel_id, chunks[0], post=post)
            log.info(
                "channel %s: a burst of %d message(s) is too big for one prompt; reading it as %d",
                channel_id,
                len(rows),
                len(chunks),
            )
            return await self._extract_chunks(channel_id, chunks, post=post)
        except Exception:  # noqa: BLE001 - the extractor must never take the bot down
            log.exception("extraction failed for channel %s", channel_id)
            return None

    async def _extract_chunks(
        self, channel_id: str, chunks: list[list[dict]], post: bool
    ) -> Plan | None:
        """Read an oversized burst in pieces, and still post one card for it.

        The pieces are separate model calls but one conversation, so they are
        consolidated exactly as a rescan consolidates a week -- otherwise
        splitting a burst would turn one card into three.
        """
        plans = [await self._extract(channel_id, chunk, post=False) for chunk in chunks]
        plans = [plan for plan in plans if plan is not None]
        if not plans:
            return None
        merged = _merge_plans(plans)
        if post and merged.planned and merged.week_start is not None:
            merged.planned = consolidate(merged.planned)
            merged.amendment_ids = await self.apply_plan(
                channel_id,
                merged.rsvps,
                merged.proposals,
                merged.week_start,
                merged.summary,
            )
            self.bot.repo.mark_messages_processed(merged.message_ids)
        return merged

    async def _extract(self, channel_id: str, rows: list[dict], post: bool) -> Plan | None:
        bot, repo = self.bot, self.bot.repo
        burst_ids = {str(row["id"]) for row in rows}
        prepared = self._prepare(channel_id, rows)
        burst, context = prepared.burst, prepared.context
        anchor, this_week = prepared.anchor, prepared.this_week
        channel_runs, guild_runs = prepared.channel_runs, prepared.guild_runs

        call = await self.extractor.extract(prepared.messages)
        extraction_id = repo.log_extraction(
            model=bot.settings.ollama_model,
            prompt=call.prompt,
            raw_response=call.raw or (call.error or ""),
            latency_ms=call.latency_ms,
            message_ids=sorted(burst_ids),
        )
        if not call.ok:
            log.warning("channel %s: extraction failed (%s)", channel_id, call.error)
            return Plan(
                raw=call.raw,
                latency_ms=call.latency_ms,
                error=call.error,
                message_ids=sorted(burst_ids),
            )

        plan = plan_burst(
            call.extraction,
            anchor=anchor,
            tz=bot.tz,
            channel_runs=channel_runs,
            guild_runs=guild_runs,
            burst_order=[m.id for m in context + burst],
            author_ids={m.id: m.author_id for m in context + burst},
            min_confidence=bot.settings.extract_min_confidence,
            now=utcnow(),
        )
        plan.raw = call.raw
        plan.latency_ms = call.latency_ms
        plan.message_ids = sorted(burst_ids)
        for entry in plan.dropped:
            log.info(
                "channel %s: dropped %s (confidence %.2f, %s)",
                channel_id,
                entry.kind,
                entry.amendment.confidence,
                entry.match_reason,
            )

        plan.week_start = this_week
        amendment_ids: list[str] = []
        if post:
            amendment_ids = await self.apply_plan(
                channel_id, plan.rsvps, plan.proposals, this_week, plan.summary
            )
            # Only a real pass consumes the messages; a dry run leaves them for it.
            repo.mark_messages_processed(sorted(burst_ids))
        plan.amendment_ids = amendment_ids
        repo.set_extraction_amendments(extraction_id, amendment_ids)
        log.info(
            "channel %s: %d message(s) -> %d proposal(s), %d rsvp(s) in %d ms",
            channel_id,
            len(rows),
            len(plan.proposals),
            len(plan.rsvps),
            call.latency_ms,
        )
        return plan

    def _channel_name(self, channel_id: str) -> str:
        channel = self.bot.get_channel(int(channel_id)) if str(channel_id).isdigit() else None
        return getattr(channel, "name", "") or ""

    # -- effects -----------------------------------------------------------
    async def apply_plan(
        self,
        channel_id: str,
        rsvps: list[Planned],
        proposals: list[Planned],
        week: datetime,
        summary: str,
        report: RescanReport | None = None,
    ) -> list[str]:
        """Turn planned entries into answers, rows and one card. Returns the row ids.

        Shared by the live path (one burst) and a rescan (a whole window,
        consolidated), so a card looks the same whichever produced it.
        """
        await self._repost_stranded(channel_id, report)
        for entry in rsvps:
            await self._apply_rsvp(entry, channel_id)
        if not proposals:
            return []
        amendment_ids, retired = self._record(proposals, channel_id, week, summary)
        await self._mark_superseded(retired)
        if not await self._post_card(channel_id, amendment_ids):
            self._note_unposted(channel_id, amendment_ids, report)
        return amendment_ids

    async def _repost_stranded(self, channel_id: str, report: RescanReport | None) -> None:
        """Give a card to rows a previous pass wrote but never managed to post.

        A DNS failure inside :meth:`_post_card` used to leave rows ``proposed``
        with no ``proposal_message_id``: invisible in the channel, unanswerable,
        and still blocking their run's next proposal as an older sibling. They
        are the same rows a card would have carried, so they get one now.
        """
        stranded = [
            a
            for a in self.bot.repo.list_amendments(status="proposed", channel_id=channel_id)
            if not a["proposal_message_id"]
        ]
        if not stranded:
            return
        ids = [a["id"] for a in stranded]
        log.info("channel %s: re-posting %d proposal(s) left without a card", channel_id, len(ids))
        if not await self._post_card(channel_id, ids):
            self._note_unposted(channel_id, ids, report)

    def _note_unposted(
        self, channel_id: str, amendment_ids: list[str], report: RescanReport | None
    ) -> None:
        log.error(
            "channel %s: could not post the card for %d proposal(s); they stay unposted",
            channel_id,
            len(amendment_ids),
        )
        if report is not None:
            report.unposted += len(amendment_ids)
            report.errors.append(f"{len(amendment_ids)} proposal(s) could not be posted")

    async def _apply_rsvp(self, entry: Planned, channel_id: str) -> None:
        """Apply a chat answer through the same path the ✅/❌ reactions use."""
        run = entry.run
        if run is None or not entry.amendment.rsvp:
            return
        emoji = {"yes": "✅", "no": "❌"}.get(entry.amendment.rsvp)
        if emoji is None:  # "maybe" is recorded, not acted on
            return
        for user_id in entry.amendment.participants:
            result = apply_reaction(self.bot.repo, run, user_id, emoji, added=True)
            if not result.applied:
                continue
            self.bot.repo.set_rsvp(run["id"], user_id, entry.amendment.rsvp, source="chat")
            fresh = self.bot.repo.get_run(run["id"]) or run
            name = self._name_for(str(user_id))
            if result.state == "no":
                await self.bot.notify_decline(fresh, user_id, name, channel_id=channel_id)
            else:
                await self.bot.retract_decline(fresh, user_id)

    def _record(
        self, entries: list[Planned], channel_id: str, week: datetime, summary: str
    ) -> tuple[list[str], list[dict]]:
        """Insert the proposal rows, retiring any older card about the same run.

        Returns ``(new amendment ids, superseded rows)``. An urgent flush
        followed by the ordinary debounce flush of the same conversation would
        otherwise leave two live cards for one run, and a ✅ on the older one
        would re-apply a change the group has already moved past.

        Every retirement happens *before* the first row is written, so nothing
        this pass creates can be retired by a later entry in the same pass. A
        rescan puts several changes for one run on one card -- a `move` and the
        `sub` that goes with it -- and superseding as it went marked the `move`
        stale the moment the `sub` was recorded: the card was posted with a line
        whose ✅ silently did nothing, because committing requires the row to
        still be ``proposed``.
        """
        retired: list[dict] = []
        for entry in entries:
            if entry.run is not None:
                retired.extend(supersede(self.bot.repo, run_id=entry.run["id"]))
            elif entry.amendment.bosses:
                retired.extend(
                    supersede(self.bot.repo, channel_id=channel_id, bosses=entry.amendment.bosses)
                )

        ids: list[str] = []
        for entry in entries:
            when = entry.resolved.at
            # An amendment that points past the reset belongs to next boss week.
            target_week = (
                week_start(
                    when, self.bot.tz, self.bot.settings.reset_weekday, self.bot.settings.reset_time
                )
                if when is not None
                else week
            )
            ids.append(
                self.bot.repo.create_amendment(
                    week_start=target_week,
                    kind=entry.kind,
                    bosses=entry.amendment.bosses,
                    run_id=entry.run["id"] if entry.run else None,
                    new_datetime=when,
                    participants=entry.amendment.participants,
                    confidence=entry.amendment.confidence,
                    evidence_msg_ids=entry.amendment.evidence_message_ids,
                    channel_id=channel_id,
                    is_question=entry.amendment.is_question,
                    rsvp=entry.amendment.rsvp,
                    day_ref=entry.amendment.day_ref,
                    time_ref=entry.amendment.time_ref,
                    summary=entry.summary or summary,
                    payload={**entry.payload, "also_mentioned": list(entry.also_mentioned)},
                )
            )
        # Rows inserted in this pass are never their own predecessors.
        fresh = set(ids)
        return ids, [a for a in retired if a["id"] not in fresh]

    async def _mark_superseded(self, retired: list[dict]) -> None:
        annotate = getattr(self.bot, "annotate_message", None)
        if annotate is None:  # pragma: no cover - only a stand-in bot lacks it
            return
        seen: set[str] = set()
        for amendment in retired:
            message_id = amendment.get("proposal_message_id")
            if not message_id or message_id in seen:
                continue
            seen.add(message_id)
            await annotate(amendment.get("channel_id"), message_id, formatting.SUPERSEDED_NOTICE)

    async def _post_card(self, channel_id: str, amendment_ids: list[str]) -> bool:
        """Post one card for these rows.  False when it could not be posted."""
        repo = self.bot.repo
        amendments = [repo.get_amendment(aid) for aid in amendment_ids]
        amendments = [a for a in amendments if a is not None]
        if not amendments:
            return True
        runs = {}
        for amendment in amendments:
            if amendment["run_id"]:
                run = repo.get_run(amendment["run_id"])
                if run is not None:
                    runs[amendment["run_id"]] = run

        channel = await self.bot.post_channel(channel_id)
        if channel is None:
            log.error("no channel available for the proposal card; leaving it unposted")
            return False
        for amendment in amendments:
            # `payload` is the row's kind-specific bag; the card reads the note
            # from the top level, so lift it there.
            amendment["also_mentioned"] = (amendment.get("payload") or {}).get("also_mentioned", [])
        unanswered = self._unanswered(amendments, runs)
        listed = formatting.everyone_on(
            [{"participants": a["participants"] or []} for a in amendments]
            + [{"participants": r["participants"]} for r in runs.values()]
            + [{"participants": unanswered}]
        )
        # A card is a question, and only the people who can answer it are worth
        # a notification: `may_commit` requires the bossing role, so anybody
        # without it would be pinged for a ✅ the bot would then ignore.
        eligible = [uid for uid in listed if self.bot.repo.has_role(uid)]
        who = pings.audience(self.bot.repo, listed, "proposal", candidates=eligible)
        card = formatting.proposal_card(
            amendments,
            runs,
            self.bot.tz,
            unanswered=unanswered,
            confidence=min(a["confidence"] or 0.0 for a in amendments),
            who=who,
        )
        message = await self.bot._post(channel, card)
        if message is None:
            return False
        for amendment in amendments:
            repo.set_amendment_proposal_message(amendment["id"], message.id)
        return True

    @staticmethod
    def _unanswered(amendments: list[dict], runs: dict[str, dict]) -> list[str]:
        """Participants of an affected run who have not said anything yet.

        DESIGN.md §2b.1: a missing field is never filled in on someone's behalf,
        it is left TBD and the people who still need to answer are named.
        """
        if not any(a["is_question"] or a["new_datetime"] is None for a in amendments):
            return []
        spoke = {str(p) for a in amendments for p in a["participants"]}
        waiting: list[str] = []
        for run in runs.values():
            for uid in run["participants"]:
                if uid not in spoke and uid not in waiting:
                    waiting.append(uid)
        return waiting


__all__ = ["Burst", "Pipeline", "Plan", "Planned", "plan_burst", "urgent"]
