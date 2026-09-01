"""Applying a confirmed amendment to the schedule (DESIGN.md §2.3).

Nothing here runs until a human reacts ✅ on the proposal card, and only a
participant of the target run (or an admin, or the guild owner) counts.  The
functions are pure repository work with no Discord objects, so every kind can be
unit tested against an in-memory database; :mod:`bot.client` supplies the
reaction, the channel and the follow-up message.
"""

from __future__ import annotations

import logging
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from datetime import time as clock_time
from datetime import timedelta
from zoneinfo import ZoneInfo

from ..db import Repo
from ..materialise import (
    apply_fixed_to_runs,
    ensure_reminders,
    refresh_run_reminders,
    retire_fixed_run,
)
from ..rsvp import compute_status, recompute_after_roster_change
from ..weeks import current_week_start, next_week_start, week_start

log = logging.getLogger(__name__)

#: A proposal nobody answers is dropped rather than left to be confirmed days
#: later against a week that has already happened.
PROPOSAL_TTL = timedelta(hours=24)


@dataclass
class CommitResult:
    """What committing one amendment did, in terms the caller can post about."""

    amendment_id: str
    kind: str
    applied: bool = False
    run_id: str | None = None
    fixed_run_id: str | None = None
    created_run_ids: list[str] = field(default_factory=list)
    old_datetime: object | None = None
    problem: str | None = None
    #: Cards the caller should annotate: sibling proposals this commit retired.
    superseded: list[dict] = field(default_factory=list)
    #: Runs whose reminders the caller should not re-post about; purely informational.
    notes: list[str] = field(default_factory=list)


def may_commit(
    amendment: dict,
    run: dict | None,
    user_id: int | str,
    *,
    has_role: bool,
    is_admin: bool = False,
    is_owner: bool = False,
) -> bool:
    """Who is allowed to press ✅ on this card.

    A participant of the run being changed, an admin, or the guild owner. Anyone
    else needs the bossing role *and* to be named: an `add`/`fix` card has no run
    to check against, so without the role gate any account in the channel -- a
    lurker, a webhook -- could create a run that pings the whole party every week.

    ``has_role`` is passed in rather than looked up so this stays free of the
    client: :meth:`bot.client.BossBot.has_bossing_role` reads the live member
    object when there is one and falls back to the roster table when there is not.
    """
    if is_admin or is_owner:
        return True
    if not has_role:
        return False
    uid = str(user_id)
    if run is not None:
        return uid in run["participants"]
    named = [str(p) for p in amendment["participants"]]
    return uid in named if named else True


def supersede(
    repo: Repo,
    *,
    run_id: str | None = None,
    channel_id: int | str | None = None,
    bosses: Sequence[str] = (),
    keep_id: str | None = None,
    from_channel: int | str | None = None,
) -> list[dict]:
    """Retire the other live proposals about the same thing; returns them.

    Two things produce a stale card: an urgent flush followed by the ordinary
    debounce flush of the same conversation, and a burst that revisits a run
    somebody already has a card open for. Without this, a ✅ on the older card
    re-applies a change the group has already moved past.

    Keyed on the target run, or -- for `add`/`fix`, which have no run yet -- on
    the channel plus the exact boss set they would create. The second key is
    channel-scoped by construction (:meth:`bot.db.Repo.proposed_for_bosses`);
    the first is not, and ``from_channel`` is what scopes it.

    ``from_channel`` is where the row doing the retiring lives. A card may be
    retired by a row posted in its own channel, or by any row in the run's own
    home channel -- and by nothing else. That closes a retire-across-channels:
    a proposal raised somewhere else about a party's run would otherwise take
    the live cards out from under that party, in a channel that never saw
    either the request or the replacement. Callers that do not name a channel
    keep the old, unscoped behaviour, so a caller only pays for the guard once
    it can say where its row is going.
    """
    if run_id:
        candidates = _same_channel(
            repo, run_id, repo.proposed_for_run(run_id, exclude=keep_id), from_channel
        )
    elif channel_id is not None and bosses:
        candidates = repo.proposed_for_bosses(channel_id, bosses, exclude=keep_id)
    else:
        return []
    for amendment in candidates:
        repo.set_amendment_status(amendment["id"], "superseded")
    if candidates:
        log.info("superseded %d older proposal(s)", len(candidates))
    return candidates


def _same_channel(
    repo: Repo, run_id: str, candidates: list[dict], from_channel: int | str | None
) -> list[dict]:
    """The candidates a row in ``from_channel`` is entitled to retire.

    Its own channel's, always. The run's home channel's as well, but only when
    the row is *in* that home channel -- which is the case the whole guard is
    about: a card drafted elsewhere (the chatbot posts its card in the channel
    the question came from, whatever channel the run lives in) must not reach
    into a party's channel and retire what they were about to press.
    """
    if from_channel is None:
        return candidates
    mine = str(from_channel)
    run = repo.get_run(run_id)
    home = str(run["channel_id"]) if run and run["channel_id"] is not None else None
    if home is not None and home == mine:
        return candidates
    return [a for a in candidates if str(a.get("channel_id")) == mine]


def _participants_for(amendment: dict, run: dict | None) -> list[str]:
    people = [str(p) for p in amendment["participants"]]
    if run is not None:
        merged = list(run["participants"])
        for uid in people:
            if uid not in merged:
                merged.append(uid)
        return merged
    return people


def commit(
    repo: Repo,
    amendment: dict,
    tz: ZoneInfo,
    reset_weekday: int,
    reset_time: clock_time,
    ping_time: clock_time,
    countdowns: Sequence[int],
    actor_id: int | str,
    channel_id: int | str | None = None,
    on_fixed_created: Callable[[str], None] | None = None,
) -> CommitResult:
    """Apply one amendment and mark it ``confirmed``.

    ``on_fixed_created`` is called after a ``fix`` creates *or changes* a fixed
    run, so the caller can re-materialise the weeks (that needs the live client's
    config). Its name predates the edit case and is kept because the live client
    and the portal both pass it by keyword.
    """
    result = CommitResult(amendment_id=amendment["id"], kind=amendment["kind"])
    run = repo.get_run(amendment["run_id"]) if amendment["run_id"] else None
    kind = amendment["kind"]
    home = channel_id if channel_id is not None else amendment.get("channel_id")

    handler = _HANDLERS.get(kind)
    if handler is None:
        result.problem = f"don't know how to apply a `{kind}` amendment"
        return result

    problem = handler(
        repo,
        amendment,
        run,
        result,
        Context(
            tz=tz,
            reset_weekday=reset_weekday,
            reset_time=reset_time,
            ping_time=ping_time,
            countdowns=list(countdowns),
            actor_id=str(actor_id),
            channel_id=str(home) if home is not None else None,
            on_fixed_created=on_fixed_created,
        ),
    )
    if problem:
        result.problem = problem
        return result

    repo.set_amendment_status(amendment["id"], "confirmed")
    result.applied = True
    # Whatever else was proposed for this run is now about a state that no
    # longer exists, so those cards must not stay pressable. Keyed on what the
    # amendment *targeted*, not on any run it just created -- nothing can be
    # proposed against a run that did not exist a moment ago.
    if amendment["run_id"]:
        result.superseded = supersede(
            repo,
            run_id=amendment["run_id"],
            keep_id=amendment["id"],
            # This row's own channel: what it may retire is scoped to where it
            # was posted, so confirming a card in one channel cannot clear a
            # party's cards out of another.
            from_channel=home,
        )
    elif amendment["bosses"]:
        result.superseded = supersede(
            repo,
            channel_id=amendment.get("channel_id"),
            bosses=amendment["bosses"],
            keep_id=amendment["id"],
        )
    return result


@dataclass(frozen=True)
class Context:
    """The guild settings a commit needs, gathered once by the caller."""

    tz: ZoneInfo
    reset_weekday: int
    reset_time: clock_time
    ping_time: clock_time
    countdowns: list[int]
    actor_id: str
    channel_id: str | None
    on_fixed_created: Callable[[str], None] | None = None


# ---------------------------------------------------------------------------
# one function per kind
# ---------------------------------------------------------------------------


def _move(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    if run is None:
        return "that run has gone"
    new_at = amendment["new_datetime"]
    if new_at is None:
        return "no new time was agreed - use `/amend` to set one"
    result.run_id = run["id"]
    result.old_datetime = run["datetime"]
    ws = week_start(new_at, ctx.tz, ctx.reset_weekday, ctx.reset_time)
    repo.set_run_datetime(run["id"], new_at, ws)
    # A move invalidates the answers people gave about the old slot.
    repo.set_run_status(run["id"], "planned")
    refresh_run_reminders(repo, run["id"], ctx.tz, ctx.ping_time, ctx.countdowns)
    return None


def _add(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    new_at = amendment["new_datetime"]
    if new_at is None:
        return "no day and time were agreed - use `/amend` or `/fixed add`"
    if not amendment["bosses"]:
        return "no bosses were named"
    people = _participants_for(amendment, None) or [ctx.actor_id]
    ws = week_start(new_at, ctx.tz, ctx.reset_weekday, ctx.reset_time)
    run_id = repo.create_run(
        week_start=ws,
        bosses=amendment["bosses"],
        run_at=new_at,
        participants=people,
        status="planned",
        source="amend",
        channel_id=ctx.channel_id,
    )
    result.run_id = run_id
    result.created_run_ids.append(run_id)
    repo.set_amendment_run(amendment["id"], run_id)
    ensure_reminders(repo, repo.get_run(run_id), ctx.tz, ctx.ping_time, ctx.countdowns)
    return None


def _cancel(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    if run is None:
        return "that run has gone"
    result.run_id = run["id"]
    repo.set_run_status(run["id"], "cancelled")
    refresh_run_reminders(repo, run["id"], ctx.tz, ctx.ping_time, ctx.countdowns)
    return None


def _otot(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    if run is None:
        return "that run has gone"
    result.run_id = run["id"]
    repo.set_run_status(run["id"], "otot")
    # `otot` keeps the day-of ping so nobody forgets before reset, and drops the
    # countdowns (bot.materialise.reminder_specs decides that).
    refresh_run_reminders(repo, run["id"], ctx.tz, ctx.ping_time, ctx.countdowns)
    return None


def _sub(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    if run is None:
        return "that run has gone"
    result.run_id = run["id"]
    payload = amendment.get("payload") or {}
    people = list(run["participants"])
    for uid in (str(u) for u in payload.get("remove", [])):
        if uid in people:
            people.remove(uid)
    for uid in (str(u) for u in payload.get("add", [])):
        if uid not in people:
            people.append(uid)
    if people == run["participants"]:
        return "nobody to swap in or out - use `/swap` or `/fixed edit`"
    if not people:
        return "that would leave the run with nobody on it - cancel it instead"
    repo.set_run_participants(run["id"], people)
    for uid in run["participants"]:
        if uid not in people:
            repo.clear_rsvp(run["id"], uid)
    # The same rule `/swap` follows: a stand-in never agreed to this run, so a
    # confirmed one goes back to being derived from the tally.
    recompute_after_roster_change(repo, run["id"])
    return None


def _split(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    """Shrink the original run and create a second one for the bosses that left."""
    if run is None:
        return "that run has gone"
    payload = amendment.get("payload") or {}
    moved = [b for b in payload.get("bosses", amendment["bosses"]) if b in run["bosses"]]
    remaining = [b for b in run["bosses"] if b not in moved]
    if not moved:
        return "no bosses from that run were named"
    if not remaining:
        # Everything moved: this is a move, not a split.
        return _move(repo, amendment, run, result, ctx)

    people = [str(p) for p in (payload.get("participants") or amendment["participants"])]
    people = people or list(run["participants"])
    new_at = amendment["new_datetime"] or run["datetime"]
    ws = week_start(new_at, ctx.tz, ctx.reset_weekday, ctx.reset_time)

    repo.set_run_bosses(run["id"], remaining)
    refresh_run_reminders(repo, run["id"], ctx.tz, ctx.ping_time, ctx.countdowns)
    result.run_id = run["id"]

    split_id = repo.create_run(
        week_start=ws,
        bosses=moved,
        run_at=new_at,
        participants=people,
        status="planned",
        source="amend",
        channel_id=ctx.channel_id or run["channel_id"],
    )
    result.created_run_ids.append(split_id)
    ensure_reminders(repo, repo.get_run(split_id), ctx.tz, ctx.ping_time, ctx.countdowns)
    return None


def _rsvp(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    """Record an answer that was proposed rather than applied.

    The extractor never gets here: it applies a chat answer immediately through
    :func:`bot.rsvp.apply_reaction`, because reading "can" off a message is
    recording an opinion its author already stated in public. The chatbot does
    card its answers -- it is acting on a sentence addressed to *it*, so the
    person it is answering for gets to see the card first.

    The tally, and the status derived from it, are updated exactly as
    :func:`bot.api.service.set_rsvp` does, so an answer means the same thing
    however it arrived.
    """
    if run is None:
        return "that run has gone"
    answer = amendment.get("rsvp")
    if answer not in ("yes", "no", "maybe"):
        return "no answer was given"
    people = [str(p) for p in amendment["participants"]]
    if not people:
        return "nobody was named"
    outsiders = [uid for uid in people if uid not in run["participants"]]
    if outsiders:
        # Between the card going up and the ✅, a `/swap` can take somebody off.
        return "that answer is for somebody who is no longer on the run"
    result.run_id = run["id"]
    for uid in people:
        repo.set_rsvp(run["id"], uid, answer, source="chat")
    status = compute_status(run["status"], run["participants"], repo.get_rsvps(run["id"]))
    if status != run["status"]:
        repo.set_run_status(run["id"], status)
    return None


#: A ``fix`` amendment whose payload carries this removes the weekly timing
#: instead of creating one. A payload marker rather than a ninth kind: the two
#: are the same noun, the schema needs no migration for it, and every existing
#: `fix` row (which has no ``op``) keeps meaning exactly what it meant.
FIX_REMOVE = "remove"

#: A ``fix`` whose payload carries this *changes* the weekly timing it names --
#: its night, its party, or both -- instead of creating or retiring one. A third
#: payload marker rather than a ninth kind, for the reasons above and one more:
#: "change the weekly to 23:30" was answered by a second `fix` beside the first,
#: which is precisely the duplicate this exists to stop.
FIX_EDIT = "edit"


def _fix(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    """Create, change or remove a fixed weekly timing, as ``/fixed`` does."""
    payload = amendment.get("payload") or {}
    if payload.get("op") == FIX_REMOVE:
        return _unfix(repo, payload, result, ctx)
    if payload.get("op") == FIX_EDIT:
        return _refix(repo, payload, result, ctx)
    weekday, hhmm = payload.get("weekday"), payload.get("time")
    if weekday is None or not hhmm:
        return "no recurring day and time were agreed - use `/fixed add`"
    if not amendment["bosses"]:
        return "no bosses were named"
    people = _participants_for(amendment, None) or [ctx.actor_id]
    fixed_id = repo.add_fixed_run(
        owner_id=ctx.actor_id,  # the person who confirmed owns it, as /fixed add does
        bosses=amendment["bosses"],
        weekday=int(weekday),
        time_hhmm=str(hhmm),
        participants=people,
        note="created from chat",
        channel_id=ctx.channel_id,
    )
    result.fixed_run_id = fixed_id
    if ctx.on_fixed_created is not None:
        ctx.on_fixed_created(fixed_id)
    return None


def _unfix(repo: Repo, payload: dict, result: CommitResult, ctx: Context):
    """Retire a weekly timing: no more runs from it, and this week's is cancelled.

    Deliberately the same :func:`bot.materialise.retire_fixed_run` that
    `/fixed remove` and the portal call, so a baseline removed from chat leaves
    the database in exactly the state the other two routes leave it in --
    including what it does about runs that have already happened.
    """
    fixed_id = payload.get("fixed_run_id")
    if not fixed_id:  # pragma: no cover - the tool always records one
        return "no weekly timing was named"
    fixed = repo.get_fixed_run(str(fixed_id))
    if fixed is None:
        return "that weekly timing has already gone"
    weeks = [
        current_week_start(ctx.tz, ctx.reset_weekday, ctx.reset_time),
        next_week_start(ctx.tz, ctx.reset_weekday, ctx.reset_time),
    ]
    cancelled = retire_fixed_run(repo, str(fixed_id), weeks, ctx.tz, ctx.ping_time, ctx.countdowns)
    result.fixed_run_id = str(fixed_id)
    result.notes.append(f"cancelled {cancelled} scheduled run(s)")
    return None


def _refix(repo: Repo, payload: dict, result: CommitResult, ctx: Context):
    """Change a weekly timing in place: same row, same id, new night or party.

    In place is the whole point. The runs already materialised from this timing
    keep their ids, their answers and their reminders' history, and next week's
    run comes from the same baseline -- which is what a second `fix` beside the
    first could never do, and what made the live failure this handles cost a
    party two of its three members.

    The push onto those runs is :func:`bot.materialise.apply_fixed_to_runs`, the
    same helper shape `/fixed edit` and the portal's ``PATCH`` follow: only the
    fields the edit touched, and never a night that has already happened.
    Re-materialising afterwards is left to ``on_fixed_created``, exactly as a
    creation leaves it -- an edited timing whose run was never materialised (the
    old slot had passed when the week was filled) gets one for the new slot.
    """
    fixed_id = payload.get("fixed_run_id")
    if not fixed_id:  # pragma: no cover - the tool always records one
        return "no weekly timing was named"
    fixed = repo.get_fixed_run(str(fixed_id))
    if fixed is None:
        return "that weekly timing has gone"
    fields: dict[str, object] = {}
    weekday, hhmm = payload.get("weekday"), payload.get("time")
    if weekday is not None and hhmm:
        fields["weekday"] = int(weekday)
        fields["time"] = str(hhmm)
    people = [str(uid) for uid in payload.get("participants") or []]
    if people:
        fields["participants"] = people
    if not fields:
        return "nothing was left to change on that weekly timing"

    repo.update_fixed_run(str(fixed_id), **fields)
    moved = apply_fixed_to_runs(
        repo,
        str(fixed_id),
        set(fields),
        [
            current_week_start(ctx.tz, ctx.reset_weekday, ctx.reset_time),
            next_week_start(ctx.tz, ctx.reset_weekday, ctx.reset_time),
        ],
        ctx.tz,
        ctx.ping_time,
        ctx.countdowns,
    )
    result.fixed_run_id = str(fixed_id)
    result.notes.append(f"updated {moved} scheduled run(s)")
    if ctx.on_fixed_created is not None:
        ctx.on_fixed_created(str(fixed_id))
    return None


_HANDLERS: dict[str, Callable] = {
    "move": _move,
    "add": _add,
    "cancel": _cancel,
    "otot": _otot,
    "sub": _sub,
    "split": _split,
    "fix": _fix,
    "rsvp": _rsvp,
}


def reject(repo: Repo, amendment: dict) -> None:
    repo.set_amendment_status(amendment["id"], "rejected")


def expire_stale(repo: Repo, now) -> list[dict]:
    """Mark proposals older than :data:`PROPOSAL_TTL` ``expired``; returns them."""
    stale = repo.stale_amendments(now - PROPOSAL_TTL)
    for amendment in stale:
        repo.set_amendment_status(amendment["id"], "expired")
    if stale:
        log.info("expired %d unanswered proposal(s)", len(stale))
    return stale


__all__ = [
    "FIX_EDIT",
    "FIX_REMOVE",
    "PROPOSAL_TTL",
    "CommitResult",
    "commit",
    "expire_stale",
    "may_commit",
    "reject",
    "supersede",
]
