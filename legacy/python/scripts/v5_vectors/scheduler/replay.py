"""Replay serialized scheduler scenarios through the v4 SQLite repository."""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from datetime import datetime, time
from typing import Any
from zoneinfo import ZoneInfo

from bot.agent import materialise, rsvp
from bot.infrastructure import db
from bot.infrastructure.db import Repo

from . import validation


def _instant(value: str) -> datetime:
    return datetime.fromisoformat(value)


@contextmanager
def _seams(clock: datetime, ids: list[str]) -> Iterator[None]:
    """Patch every imported clock/ID alias reached by these scenarios."""
    original_id, original_db_now, original_materialise_now = (
        db.new_id,
        db.utcnow,
        materialise.utcnow,
    )
    sequence = iter(ids)
    db.new_id = lambda: next(sequence)
    db.utcnow = lambda: clock
    materialise.utcnow = lambda: clock
    try:
        yield
    finally:
        db.new_id, db.utcnow, materialise.utcnow = (
            original_id,
            original_db_now,
            original_materialise_now,
        )


def _snapshot(repo: Repo) -> dict[str, Any]:
    runs = repo.list_runs()
    return {
        "fixed_runs": [
            {
                key: row[key]
                for key in (
                    "id",
                    "owner_id",
                    "channel_id",
                    "bosses",
                    "weekday",
                    "time",
                    "participants",
                    "note",
                )
            }
            for row in repo.list_fixed_runs()
        ],
        "runs": [
            {
                **{
                    key: row[key]
                    for key in (
                        "id",
                        "fixed_run_id",
                        "channel_id",
                        "bosses",
                        "participants",
                        "status",
                        "source",
                    )
                },
                "week_start": row["week_start"].isoformat(),
                "datetime": row["datetime"].isoformat(),
            }
            for row in runs
        ],
        "reminders": [
            {
                "id": row["id"],
                "run_id": row["run_id"],
                "kind": row["kind"],
                "fire_at": row["fire_at"].isoformat(),
                "sent_at": row["sent_at"].isoformat() if row["sent_at"] else None,
            }
            for run in runs
            for row in repo.list_reminders(run["id"])
        ],
        "rsvps": [
            {
                "run_id": row["run_id"],
                "user_id": row["user_id"],
                "state": row["state"],
                "source": row["source"],
                "at": db.from_iso(row["at"]).isoformat(),
            }
            for row in repo._conn.execute(  # noqa: SLF001 - no public RSVP history reader exists.
                "SELECT run_id, user_id, state, source, at FROM rsvps ORDER BY run_id, user_id"
            )
        ],
        "side_effects": [],
    }


def _run_id(case_id: str, refs: dict[str, str], key: str) -> str:
    try:
        return refs[key]
    except KeyError as exc:
        raise validation.ContractError(
            f"{case_id}: materialization did not produce run key {key!r}"
        ) from exc


def replay(case: dict[str, Any]) -> dict[str, Any]:
    """Replay one statically valid case; runtime run references remain oracle-derived."""
    validation.validate_input_case(case)
    input_ = case["input"]
    tz, now = ZoneInfo(input_["timezone"]), _instant(input_["clock"])
    week_start = _instant(input_["week_start"])
    ping_time = time.fromisoformat(input_["ping_time"])
    repo, fixed_refs, run_refs, results = Repo(":memory:"), {}, {}, []
    try:
        with _seams(now, input_["uuid_sequence"]):
            for step_index, step in enumerate(input_["steps"]):
                try:
                    op = step["op"]
                    if op == "add_fixed":
                        fixed_refs[step["fixed_key"]] = repo.add_fixed_run(
                            step["owner_id"],
                            step["bosses"],
                            step["weekday"],
                            step["time"],
                            step["participants"],
                            channel_id=step["channel_id"],
                        )
                        value: Any = fixed_refs[step["fixed_key"]]
                    elif op == "create_run":
                        run_refs[step["run_key"]] = repo.create_run(
                            week_start,
                            step["bosses"],
                            _instant(step["at"]),
                            step["participants"],
                            step["status"],
                            step["source"],
                            channel_id=step["channel_id"],
                        )
                        value = run_refs[step["run_key"]]
                    elif op == "materialise":
                        start = _instant(step.get("week_start", input_["week_start"]))
                        value = materialise.materialise_week(
                            repo, start, tz, ping_time, input_["countdowns"], now=now
                        )
                        fixed_keys = {fixed_id: key for key, fixed_id in fixed_refs.items()}
                        for fixed in repo.list_fixed_runs():
                            run = repo.run_for_fixed(fixed["id"], start)
                            if run is not None:
                                run_refs[f"{fixed_keys[fixed['id']]}@{start.isoformat()}"] = run[
                                    "id"
                                ]
                    elif op == "set_status":
                        repo.set_run_status(
                            _run_id(case["case_id"], run_refs, step["run_key"]), step["status"]
                        )
                        value = step["status"]
                    elif op == "set_rsvp":
                        repo.set_rsvp(
                            _run_id(case["case_id"], run_refs, step["run_key"]),
                            step["user_id"],
                            step["state"],
                            source=step["source"],
                        )
                        value = step["state"]
                    elif op == "reaction":
                        run = repo.get_run(_run_id(case["case_id"], run_refs, step["run_key"]))
                        assert run is not None
                        value = rsvp.apply_reaction(
                            repo, run, step["user_id"], step["emoji"], step["added"]
                        ).__dict__
                    elif op == "edit_fixed":
                        fixed_id = fixed_refs[step["fixed_key"]]
                        repo.update_fixed_run(fixed_id, **step["fields"])
                        value = materialise.apply_fixed_to_runs(
                            repo,
                            fixed_id,
                            step["changed"],
                            [_instant(item) for item in step["week_starts"]],
                            tz,
                            ping_time,
                            input_["countdowns"],
                        )
                    elif op == "retire_fixed":
                        value = materialise.retire_fixed_run(
                            repo,
                            fixed_refs[step["fixed_key"]],
                            [_instant(item) for item in step["week_starts"]],
                            tz,
                            ping_time,
                            input_["countdowns"],
                        )
                        del fixed_refs[step["fixed_key"]]
                    else:
                        raise ValueError(f"unknown scheduler operation {op!r}")
                except StopIteration:
                    raise validation.ContractError(
                        f"{case['case_id']}: uuid_sequence exhausted during oracle replay "
                        f"at step {step_index}; any prior writes were ephemeral and discarded"
                    ) from None
                except Exception as exc:
                    expected = step.get("error_type")
                    if expected is None or type(exc).__name__ != expected:
                        raise
                    results.append({"error": {"type": expected, "message": str(exc)}})
                else:
                    if "error_type" in step:
                        raise AssertionError(
                            f"{case['case_id']} expected {step['error_type']} but succeeded"
                        )
                    results.append({"value": value})
        return {"steps": results, "final_state": _snapshot(repo)}
    finally:
        repo.close()
