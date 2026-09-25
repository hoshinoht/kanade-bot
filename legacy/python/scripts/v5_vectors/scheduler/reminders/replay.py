"""Replay reminder vectors through v4 ``materialise`` and the in-memory ``Repo``."""

from __future__ import annotations

from datetime import datetime, time
from typing import Any
from zoneinfo import ZoneInfo

from bot.agent import materialise
from bot.infrastructure.db import Repo

from .. import oracle
from . import validation

ERRORS: dict[str, type[Exception]] = {"ValueError": ValueError}


def _at(value: str) -> datetime:
    return datetime.fromisoformat(value)


def _ping(value: str) -> time:
    return time.fromisoformat(value)


def replay(case: dict[str, Any]) -> dict[str, Any]:
    """Replay one input case in a clean store; input is validated before the store opens."""
    validation.validate_input_case(case)
    case_id, input_ = case["case_id"], case["input"]
    tz, clock = ZoneInfo(input_["timezone"]), oracle.Clock(input_["clock"])
    repo, fixed_refs, run_refs, results = Repo(":memory:"), {}, {}, []

    def run(key: str) -> str:
        return oracle.run_id(case_id, run_refs, key)

    def add_fixed(step: dict) -> str:
        fixed_refs[step["fixed_key"]] = repo.add_fixed_run(
            step["owner_id"],
            step["bosses"],
            step["weekday"],
            step["time"],
            step["participants"],
            channel_id=step["channel_id"],
        )
        return fixed_refs[step["fixed_key"]]

    def create_run(step: dict) -> str:
        run_refs[step["run_key"]] = repo.create_run(
            _at(step["week_start"]),
            step["bosses"],
            _at(step["at"]),
            step["participants"],
            step["status"],
            step["source"],
            channel_id=step["channel_id"],
        )
        return run_refs[step["run_key"]]

    def materialise_week(step: dict) -> list[str]:
        start = _at(step["week_start"])
        created = materialise.materialise_week(
            repo, start, tz, _ping(step["ping_time"]), step["countdowns"], now=clock.now
        )
        for key, fixed_id in fixed_refs.items():
            found = repo.run_for_fixed(fixed_id, start)
            if found is not None:
                run_refs[f"{key}@{step['week_start']}"] = found["id"]
        return created

    def set_status(step: dict) -> str:
        repo.set_run_status(run(step["run_key"]), step["status"])
        return step["status"]

    def set_clock(step: dict) -> str:
        clock.now = _at(step["clock"])
        return clock.now.isoformat()

    def reminder_specs(step: dict) -> list[dict[str, str]]:
        return [
            {"kind": spec.kind, "fire_at": spec.fire_at.isoformat()}
            for spec in materialise.reminder_specs(
                _at(step["at"]), step["status"], tz, _ping(step["ping_time"]), step["countdowns"]
            )
        ]

    def ensure_reminders(step: dict) -> list[str]:
        found = repo.get_run(run(step["run_key"]))
        return materialise.ensure_reminders(
            repo,
            found,
            tz,
            _ping(step["ping_time"]),
            step["countdowns"],
            now=clock.now,
            rebuild=step["rebuild"],
        )

    def add_reminder(step: dict) -> str | None:
        sent = step["sent_at"]
        return repo.add_reminder(
            run(step["run_key"]),
            step["kind"],
            _at(step["fire_at"]),
            sent_at=_at(sent) if sent is not None else None,
        )

    def mark_reminder_sent(step: dict) -> None:
        row = oracle.reminder_row(repo, run_refs, case_id, step["run_key"], step["kind"])
        repo.mark_reminder_sent(row["id"], message_id=step["message_id"], at=clock.now)

    def reschedule(step: dict) -> bool:
        row = oracle.reminder_row(repo, run_refs, case_id, step["run_key"], step["kind"])
        return repo.reschedule_unposted_reminder(row["id"], _at(step["fire_at"]), clock.now)

    def reconcile(step: dict) -> int:
        return materialise.reconcile_day_of(repo, tz, _ping(step["ping_time"]), now=clock.now)

    def mark_done(step: dict) -> list[str]:
        return materialise.mark_done(repo, now=clock.now)

    def list_reminders(step: dict) -> list[dict[str, Any]]:
        return [
            {
                "id": row["id"],
                "kind": row["kind"],
                "fire_at": row["fire_at"].isoformat(),
                "sent_at": row["sent_at"].isoformat() if row["sent_at"] else None,
                "message_id": row["message_id"],
            }
            for row in repo.list_reminders(run(step["run_key"]))
        ]

    def is_stale(step: dict) -> bool:
        return materialise.is_stale(step["kind"], _at(step["fire_at"]), _at(step["now"]))

    handlers = {
        "add_fixed": add_fixed,
        "create_run": create_run,
        "materialise": materialise_week,
        "set_status": set_status,
        "set_clock": set_clock,
        "reminder_specs": reminder_specs,
        "ensure_reminders": ensure_reminders,
        "add_reminder": add_reminder,
        "mark_reminder_sent": mark_reminder_sent,
        "reschedule_unposted_reminder": reschedule,
        "reconcile_day_of": reconcile,
        "mark_done": mark_done,
        "list_reminders": list_reminders,
        "is_stale": is_stale,
    }
    ids = oracle.Ids(input_["uuid_sequence"])
    try:
        with oracle.seams(clock, ids, (materialise,)):
            for index, step in enumerate(input_["steps"]):
                handler = handlers[step["op"]]
                results.append(
                    oracle.run_step(case_id, index, step, ids, ERRORS, lambda: handler(step))  # noqa: B023
                )
        return {"steps": results, "final_state": oracle.snapshot(repo, [])}
    finally:
        repo.close()
