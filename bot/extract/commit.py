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
from ..materialise import ensure_reminders, refresh_run_reminders
from ..weeks import week_start

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
) -> list[dict]:
    """Retire the other live proposals about the same thing; returns them.

    Two things produce a stale card: an urgent flush followed by the ordinary
    debounce flush of the same conversation, and a burst that revisits a run
    somebody already has a card open for. Without this, a ✅ on the older card
    re-applies a change the group has already moved past.

    Keyed on the target run, or -- for `add`/`fix`, which have no run yet -- on
    the channel plus the exact boss set they would create.
    """
    if run_id:
        candidates = repo.proposed_for_run(run_id, exclude=keep_id)
    elif channel_id is not None and bosses:
        candidates = repo.proposed_for_bosses(channel_id, bosses, exclude=keep_id)
    else:
        return []
    for amendment in candidates:
        repo.set_amendment_status(amendment["id"], "superseded")
    if candidates:
        log.info("superseded %d older proposal(s)", len(candidates))
    return candidates


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

    ``on_fixed_created`` is called after a ``fix`` creates a fixed run, so the
    caller can re-materialise the weeks (that needs the live client's config).
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
        result.superseded = supersede(repo, run_id=amendment["run_id"], keep_id=amendment["id"])
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


def _fix(repo: Repo, amendment: dict, run: dict | None, result: CommitResult, ctx: Context):
    """Create the fixed weekly timing exactly as ``/fixed add`` would."""
    payload = amendment.get("payload") or {}
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


_HANDLERS: dict[str, Callable] = {
    "move": _move,
    "add": _add,
    "cancel": _cancel,
    "otot": _otot,
    "sub": _sub,
    "split": _split,
    "fix": _fix,
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
    "PROPOSAL_TTL",
    "CommitResult",
    "commit",
    "expire_stale",
    "may_commit",
    "reject",
    "supersede",
]
