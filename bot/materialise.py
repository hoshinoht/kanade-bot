"""Turning fixed runs into concrete weekly runs, and runs into reminder rows.

The ``reminders`` table is the scheduler's source of truth: rows are written
here, and a 30 s ``discord.ext.tasks`` loop in :mod:`bot.client` picks up
anything whose ``fire_at`` has passed and that has not been sent.  Nothing is
held in memory, so a restart loses nothing.
"""

from __future__ import annotations

import logging
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime, time, timedelta
from zoneinfo import ZoneInfo

from .db import Repo
from .rsvp import recompute_after_roster_change
from .timeutil import utcnow
from .weeks import slot_in_week, week_end

log = logging.getLogger(__name__)

DAY_OF = "day_of"
COUNTDOWN_PREFIX = "countdown_"

#: Statuses that get no countdown pings.
NO_COUNTDOWN = ("otot", "cancelled")
#: Statuses that get no reminders at all.
NO_REMINDERS = ("cancelled", "done")

#: How long after its start time a run is still "on". Past that it is `done`:
#: it drops out of `/schedule`, out of the id dropdowns, and out of anything the
#: extractor could propose a change to. Generous, because a boss night that
#: starts at 23:00 is still happening at midnight.
RUN_DONE_AFTER = timedelta(hours=2)

#: Statuses that mean "still to come". `mark_done` retires these; `/schedule`,
#: the portal, `bossctl` and the id dropdowns show only these unless asked
#: otherwise. `cancelled` stays cancelled (it never happened), `done` is over.
LIVE_STATUSES = ("planned", "confirmed", "at_risk", "otot")

#: How late a reminder may fire and still be worth posting. The bot's host can
#: be asleep (a laptop in transit); a morning ping that surfaces at lunch is
#: still useful, a "1h to go" that surfaces after the run is just noise.
STALE_GRACE: dict[str, timedelta] = {DAY_OF: timedelta(hours=12)}
COUNTDOWN_GRACE = timedelta(minutes=30)


def is_stale(kind: str, fire_at: datetime, now: datetime) -> bool:
    """True when ``fire_at`` is so far behind ``now`` that posting would mislead."""
    return now - fire_at > STALE_GRACE.get(kind, COUNTDOWN_GRACE)


def countdown_kind(minutes: int) -> str:
    return f"{COUNTDOWN_PREFIX}{minutes}"


def countdown_minutes(kind: str) -> int | None:
    if not kind.startswith(COUNTDOWN_PREFIX):
        return None
    try:
        return int(kind[len(COUNTDOWN_PREFIX) :])
    except ValueError:
        return None


@dataclass(frozen=True)
class ReminderSpec:
    kind: str
    fire_at: datetime


def reminder_specs(
    run_at: datetime,
    status: str,
    tz: ZoneInfo,
    ping_time: time,
    countdowns: Sequence[int],
) -> list[ReminderSpec]:
    """Which reminders a run should have, as pure data.

    * ``day_of``      -- ``ping_time`` (guild-local) on the run's local date.
    * ``countdown_M`` -- ``M`` minutes before the run.

    ``otot`` runs keep only the day-of ping (so nobody forgets before reset);
    ``cancelled`` and ``done`` runs get nothing.
    """
    if status in NO_REMINDERS:
        return []
    local = run_at.astimezone(tz)
    day_of_at = datetime.combine(local.date(), ping_time).replace(tzinfo=tz)
    if day_of_at >= local:
        # Late-night runs (e.g. 00:30) would otherwise be "reminded" hours after
        # they happened; ping on the previous morning instead.
        day_of_at = (day_of_at.replace(tzinfo=None) - timedelta(days=1)).replace(tzinfo=tz)
    specs = [ReminderSpec(DAY_OF, day_of_at)]
    if status not in NO_COUNTDOWN:
        for minutes in sorted(set(countdowns), reverse=True):
            specs.append(ReminderSpec(countdown_kind(minutes), run_at - timedelta(minutes=minutes)))
    return specs


def ensure_reminders(
    repo: Repo,
    run: dict,
    tz: ZoneInfo,
    ping_time: time,
    countdowns: Sequence[int],
    now: datetime | None = None,
    rebuild: bool = False,
) -> list[str]:
    """Create the reminder rows a run should have; idempotent.

    ``rebuild=True`` drops unsent reminders first -- use it after a run moves,
    is cancelled or goes ``otot``.  Reminders whose ``fire_at`` is already in the
    past are written as already-sent so a restart or a late edit never spams
    stale pings.
    """
    now = now or utcnow()
    specs = reminder_specs(run["datetime"], run["status"], tz, ping_time, countdowns)
    wanted = {spec.kind for spec in specs}

    if rebuild:
        # Every reminder goes, *including already-sent ones*: UNIQUE(run_id, kind)
        # would otherwise block a fresh `day_of` for a run moved later in the same
        # week after its old morning ping already fired, leaving it un-pinged.
        # Consequence: ✅/❌ on the superseded message no longer map to this run.
        # That is intended -- a move resets the run to `planned`, so those answers
        # were about a time that no longer exists and have to be given again.
        repo.delete_reminders(run["id"])
    else:
        # Drop unsent reminders the run should no longer have (cancelled, gone
        # otot, or a countdown offset that was removed from config).
        repo.delete_unsent_reminders(run["id"], keep_kinds=tuple(wanted))

    created: list[str] = []
    for spec in specs:
        sent_at = now if spec.fire_at <= now else None
        if repo.add_reminder(run["id"], spec.kind, spec.fire_at, sent_at=sent_at) is not None:
            created.append(spec.kind)
    return created


def is_past(run_at: datetime, now: datetime) -> bool:
    """True once a run's slot is far enough behind ``now`` to count as over."""
    return run_at + RUN_DONE_AFTER < now


def mark_done(repo: Repo, now: datetime | None = None) -> list[str]:
    """Retire runs whose slot has passed; returns the ids that changed.

    Materialisation fills a whole boss week at once, so by Sunday the week
    already holds Thursday's and Friday's runs. Left alone they sit in
    `/schedule` and in every id dropdown looking like something still to do.
    Their unsent reminders go too -- a `day_of` for a night that has been and
    gone is exactly the stale ping :func:`is_stale` exists to avoid, and this
    removes the row rather than relying on the tick to skip it.
    """
    now = now or utcnow()
    changed: list[str] = []
    for run in repo.list_runs():
        if run["status"] not in LIVE_STATUSES or not is_past(run["datetime"], now):
            continue
        repo.set_run_status(run["id"], "done")
        repo.delete_unsent_reminders(run["id"])
        changed.append(run["id"])
    if changed:
        log.info("marked %d run(s) done", len(changed))
    return changed


def _same_home(left: object, right: object) -> bool:
    """Do a run and a weekly timing share a home channel? Both unset counts."""
    return (str(left) if left is not None else None) == (str(right) if right is not None else None)


def adoptable_run(repo: Repo, fixed: dict, week_start: datetime, now: datetime) -> dict | None:
    """The standalone run in that week which ``fixed`` should take over, if any.

    A member who already has a one-off on the board and then says "make it every
    week" means *that* run, not a second one beside it. Nothing used to hear
    that: the weekly was created, the week was materialised, and the party ended
    up with two nights for the same boss -- one carrying their answers and one
    carrying the reminders.

    Adoptable is deliberately narrow, because a wrong adoption silently
    reschedules a night nobody asked about while a missed one leaves a duplicate
    anybody can cancel. So: the same bosses, the same home channel, the same
    week, still to come, not already spoken for by another timing, and not a
    night that has been called off or is over. Exactly one candidate or none --
    two runs that both fit is precisely the case where guessing is worst.
    """
    wanted = set(fixed["bosses"])
    found = [
        run
        for run in repo.list_runs(week_start=week_start)
        if run["fixed_run_id"] is None
        and run["status"] not in ("done", "cancelled")
        and set(run["bosses"]) == wanted
        and _same_home(run["channel_id"], fixed["channel_id"])
        and not is_past(run["datetime"], now)
    ]
    return found[0] if len(found) == 1 else None


def adopt_run(
    repo: Repo,
    run: dict,
    fixed: dict,
    week_start: datetime,
    run_at: datetime,
    tz: ZoneInfo,
    ping_time: time,
    countdowns: Sequence[int],
    now: datetime | None = None,
) -> None:
    """Make an existing standalone run that week's run for ``fixed``, in place.

    In place is the point: the run keeps its id, so the card that created it, the
    answers people have already given and the message ids of the pings that have
    already fired all still refer to the night they were about. Only the three
    things the weekly actually decides are pushed onto it -- which slot, which
    party, and which timing it belongs to.

    The party goes through :func:`bot.rsvp.recompute_after_roster_change` for the
    reason ``/swap`` does: somebody added by the weekly never agreed to this run,
    so a ``confirmed`` goes back to being derived, while the answers of members
    still on it survive (:func:`bot.rsvp.compute_status` ignores the rest).
    """
    repo.set_run_fixed(run["id"], fixed["id"])
    repo.set_run_datetime(run["id"], run_at, week_start)
    if list(run["participants"]) != [str(p) for p in fixed["participants"]]:
        repo.set_run_participants(run["id"], fixed["participants"])
        recompute_after_roster_change(repo, run["id"])
    refresh_run_reminders(repo, run["id"], tz, ping_time, countdowns, now=now)


def materialise_week(
    repo: Repo,
    week_start: datetime,
    tz: ZoneInfo,
    ping_time: time,
    countdowns: Sequence[int],
    now: datetime | None = None,
) -> list[int]:
    """Create a run for every fixed run that falls inside ``week_start``'s week.

    Idempotent: a fixed run already materialised for that week is skipped (the
    partial unique index on ``(fixed_run_id, week_start)`` backs this up).
    A slot that has already passed is not created at all -- materialising
    mid-week would otherwise conjure Thursday's run on Sunday, complete with
    reminders that immediately count as sent. Returns the ids of new runs.

    A timing whose week already holds a matching one-off adopts it rather than
    creating a second night beside it (:func:`adoptable_run`). Adopted runs are
    not in the returned ids: nothing was created, and the callers that report
    "materialised N run(s)" would be counting a run the guild already had.
    """
    now = now or utcnow()
    end = week_end(week_start, tz)
    created: list[int] = []
    for fixed in repo.list_fixed_runs():
        if repo.run_for_fixed(fixed["id"], week_start) is not None:
            continue
        hour, minute = (int(p) for p in fixed["time"].split(":"))
        run_at = slot_in_week(week_start, tz, fixed["weekday"], time(hour, minute))
        if not (week_start <= run_at < end):  # pragma: no cover - slot_in_week guarantees this
            log.warning("fixed run %s landed outside its week; skipping", fixed["id"])
            continue
        if is_past(run_at, now):
            log.debug(
                "fixed run %s falls at %s, already past; not materialising it", fixed["id"], run_at
            )
            continue
        adoptable = adoptable_run(repo, fixed, week_start, now)
        if adoptable is not None:
            adopt_run(
                repo, adoptable, fixed, week_start, run_at, tz, ping_time, countdowns, now=now
            )
            log.info(
                "fixed run %s adopted run %s (week starting %s) and moved it to %s",
                fixed["id"],
                adoptable["id"],
                week_start,
                run_at,
            )
            continue
        run_id = repo.create_run(
            week_start=week_start,
            bosses=fixed["bosses"],
            run_at=run_at,
            participants=fixed["participants"],
            status="planned",
            source="fixed",
            fixed_run_id=fixed["id"],
            channel_id=fixed["channel_id"],
        )
        created.append(run_id)
        log.info("materialised run %s from fixed run %s at %s", run_id, fixed["id"], run_at)

    for run in repo.list_runs(week_start=week_start):
        ensure_reminders(repo, run, tz, ping_time, countdowns, now=now)
    return created


def reconcile_day_of(repo: Repo, tz: ZoneInfo, ping_time: time) -> int:
    """Re-place unsent day-of reminders after the ping time changed; returns how many moved."""
    moved = 0
    for reminder in repo.unsent_reminders(kind=DAY_OF):
        run = repo.get_run(reminder["run_id"])
        if run is None:
            continue
        specs = reminder_specs(run["datetime"], run["status"], tz, ping_time, ())
        wanted = {s.kind: s for s in specs}
        spec = wanted.get(DAY_OF)
        if spec is not None and spec.fire_at != reminder["fire_at"]:
            repo.set_reminder_fire_at(reminder["id"], spec.fire_at)
            moved += 1
    return moved


def refresh_run_reminders(
    repo: Repo,
    run_id: int,
    tz: ZoneInfo,
    ping_time: time,
    countdowns: Sequence[int],
    now: datetime | None = None,
) -> None:
    """Rebuild the unsent reminders of one run after it changed."""
    run = repo.get_run(run_id)
    if run is None:
        return
    ensure_reminders(repo, run, tz, ping_time, countdowns, now=now, rebuild=True)


def apply_fixed_to_runs(
    repo: Repo,
    fixed_id: str,
    changed: Sequence[str],
    week_starts: Sequence[datetime],
    tz: ZoneInfo,
    ping_time: time,
    countdowns: Sequence[int],
) -> int:
    """Push an edited weekly timing onto the runs it has already produced.

    The pure half of `/fixed edit`, in the shape :func:`retire_fixed_run` is the
    pure half of `/fixed remove`: the chatbot's ratified ``fix``/``edit`` card
    commits through it, so a timing changed from chat leaves the database in the
    state the slash command and the portal leave it in.

    Only the fields the edit actually *touched* are pushed, which is the rule the
    other two routes follow (``bot.commands._apply_fixed_to_runs``,
    ``bot.api.service._apply_fixed_to_runs``): re-snapping every field would undo
    this week's `/amend` -- editing a note would drag a run that was moved
    Mon -> Wed back to Monday. A run that is already ``done`` or ``cancelled`` is
    left alone, because rewriting a night that has happened would falsify the
    record of it.

    Returns how many runs were touched.
    """
    fixed = repo.get_fixed_run(fixed_id)
    fields = set(changed)
    if fixed is None or not fields:
        return 0
    reschedule = bool(fields & {"weekday", "time"})
    touched = 0
    for week_start in week_starts:
        run = repo.run_for_fixed(fixed_id, week_start)
        if run is None or run["status"] in ("done", "cancelled"):
            continue
        if "bosses" in fields:
            repo.set_run_bosses(run["id"], fixed["bosses"])
        if "participants" in fields:
            repo.set_run_participants(run["id"], fixed["participants"])
        if "channel_id" in fields:
            repo.set_run_channel(run["id"], fixed["channel_id"])
        if reschedule:
            hour, minute = (int(part) for part in fixed["time"].split(":"))
            run_at = slot_in_week(week_start, tz, fixed["weekday"], time(hour, minute))
            repo.set_run_datetime(run["id"], run_at, week_start)
            # Only the slot moved, so only the reminders need re-placing.
            refresh_run_reminders(repo, run["id"], tz, ping_time, countdowns)
        touched += 1
    return touched


def retire_fixed_run(
    repo: Repo,
    fixed_id: str,
    week_starts: Sequence[datetime],
    tz: ZoneInfo,
    ping_time: time,
    countdowns: Sequence[int],
) -> int:
    """Delete a weekly timing and cancel the runs it has already produced.

    Returns how many live runs were cancelled. The pure half of `/fixed remove`:
    :func:`bot.api.service.delete_fixed` wraps this with the channel notice, and
    the chatbot's ratified ``fix``/``remove`` card commits through it too, so a
    baseline removed from Discord, from the portal and from chat all leave the
    database in exactly the same state.

    A run that is already ``done`` or ``cancelled`` is left alone: retiring a
    timing is about the weeks to come, and rewriting a night that has already
    happened would falsify the record of it.
    """
    cancelled = 0
    for week_start in week_starts:
        run = repo.run_for_fixed(fixed_id, week_start)
        if run is not None and run["status"] not in ("done", "cancelled"):
            repo.set_run_status(run["id"], "cancelled")
            refresh_run_reminders(repo, run["id"], tz, ping_time, countdowns)
            cancelled += 1
    repo.delete_fixed_run(fixed_id)
    return cancelled
