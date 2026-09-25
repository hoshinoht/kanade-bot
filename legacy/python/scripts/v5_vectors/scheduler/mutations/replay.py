"""Replay mutation vectors through the real v4 ``bot.api.service`` functions."""

from __future__ import annotations

import asyncio
from datetime import datetime
from typing import Any

from bot.agent import client, materialise
from bot.api import service
from bot.api.errors import BadRequest
from bot.domain import weeks
from bot.infrastructure.db import Repo

from .. import oracle
from .. import validation as scheduler_validation
from . import validation
from .host import Host

ERRORS: dict[str, type[Exception]] = {"BadRequest": BadRequest}


def _at(value: str) -> datetime:
    return datetime.fromisoformat(value)


def _run_view(view: dict[str, Any]) -> dict[str, Any]:
    """The portable scheduler facts of ``service.run_view``; rendering fields are dropped."""
    change = view["roster_change"]
    return {
        "id": view["id"],
        "status": view["status"],
        "datetime": view["datetime"],
        "week_start": view["week_start"],
        "channel_id": view["channel_id"],
        "bosses": view["bosses"],
        "participants": [{"id": p["id"], "rsvp": p["rsvp"]} for p in view["participants"]],
        "roster_change": {
            "out": [p["id"] for p in change["out"]],
            "in": [p["id"] for p in change["in"]],
            "changed": change["changed"],
        },
    }


def _fixed_view(view: dict[str, Any]) -> dict[str, Any]:
    return {
        "id": view["id"],
        "bosses": view["bosses"],
        "weekday": view["weekday"],
        "time": view["time"],
        "participants": [p["id"] for p in view["participants"]],
        "channel_id": view["channel_id"],
        "channel_watched": view["channel_watched"],
        "note": view["note"],
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    """Replay one input case in a clean store; input is validated before the store opens."""
    validation.validate_input_case(case)
    case_id, input_ = case["case_id"], case["input"]
    clock, ids = oracle.Clock(input_["clock"]), oracle.Ids(input_["uuid_sequence"])
    repo, fixed_refs, run_refs, results = Repo(":memory:"), {}, {}, []

    def run(key: str) -> str:
        return oracle.run_id(case_id, run_refs, key)

    def register_materialised() -> None:
        for start in weeks.materialised_week_starts(
            host.tz, host.settings.reset_weekday, host.settings.reset_time, clock.now
        ):
            for key, fixed_id in fixed_refs.items():
                found = repo.run_for_fixed(fixed_id, start)
                if found is not None:
                    run_refs[f"{key}@{start.isoformat()}"] = found["id"]

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

    def set_rsvp(step: dict) -> str:
        repo.set_rsvp(run(step["run_key"]), step["user_id"], step["state"], source=step["source"])
        return step["state"]

    def set_clock(step: dict) -> str:
        clock.now = _at(step["clock"])
        return clock.now.isoformat()

    def materialise_weeks(step: dict) -> None:
        host.materialise_weeks()
        register_materialised()

    def set_status(step: dict) -> dict:
        return _run_view(
            asyncio.run(
                service.set_status(
                    host,
                    run(step["run_key"]),
                    step["status"],
                    announce=step["announce"],
                    mark=step["mark"],
                )
            )
        )

    def amend(step: dict) -> dict:
        return _run_view(asyncio.run(service.amend_run(host, run(step["run_key"]), step["to"])))

    def swap(step: dict) -> dict:
        return _run_view(
            asyncio.run(
                service.swap_participants(
                    host,
                    run(step["run_key"]),
                    remove=step["remove"],
                    add=step["add"],
                    mark=step["mark"],
                )
            )
        )

    def update_fixed(step: dict) -> dict:
        view = asyncio.run(
            service.update_fixed(host, fixed_refs[step["fixed_key"]], **step["changes"])
        )
        register_materialised()
        return _fixed_view(view)

    handlers = {
        "add_fixed": add_fixed,
        "create_run": create_run,
        "set_rsvp": set_rsvp,
        "set_clock": set_clock,
        "materialise_weeks": materialise_weeks,
        "set_status": set_status,
        "amend": amend,
        "swap": swap,
        "update_fixed": update_fixed,
    }
    try:
        with oracle.seams(clock, ids, (materialise, service, client, weeks)):
            host = Host(repo, input_)
            for member in input_["members"]:
                repo.upsert_member(
                    member["user_id"],
                    member["display_name"],
                    member["nickname"],
                    member["has_role"],
                )
                repo.set_ping_level(member["user_id"], member["ping_level"])
            for index, step in enumerate(input_["steps"]):
                handler = handlers[step["op"]]
                results.append(
                    oracle.run_step(case_id, index, step, ids, ERRORS, lambda: handler(step))  # noqa: B023
                )
                if host.unexpected:
                    raise scheduler_validation.ContractError(
                        f"{case_id} step {index}: {'; '.join(host.unexpected)}"
                    )
        return {"steps": results, "final_state": oracle.snapshot(repo, host.effects)}
    finally:
        repo.close()
