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
from .timeutil import utcnow
from .weeks import slot_in_week, week_end

log = logging.getLogger(__name__)

DAY_OF = "day_of"
COUNTDOWN_PREFIX = "countdown_"

#: Statuses that get no countdown pings.
NO_COUNTDOWN = ("otot", "cancelled")
#: Statuses that get no reminders at all.
NO_REMINDERS = ("cancelled", "done")


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
    Returns the ids of newly created runs.
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
