"""Replay loop shared by the families that drive real ``BossBot`` methods."""

from __future__ import annotations

import asyncio
from collections.abc import Callable
from datetime import datetime
from typing import Any

from bot.agent import client, materialise
from bot.domain import weeks
from bot.infrastructure.db import Repo

from . import delivery, oracle, validation
from .discord_host import Host

Handler = Callable[[dict[str, Any]], Any]
ERRORS: dict[str, type[Exception]] = {"ValueError": ValueError}


def at(value: str) -> datetime:
    return datetime.fromisoformat(value)


class Session:
    """One clean store, host, key maps and recorded intents for one case."""

    def __init__(self, case: dict[str, Any], repo: Repo, clock: oracle.Clock):
        self.case_id, self.input = case["case_id"], case["input"]
        self.repo, self.clock = repo, clock
        self.host = Host(repo, self.input)
        self.fixed: dict[str, str] = {}
        self.runs: dict[str, str] = {}
        self.intents: list[dict[str, Any]] = []
        delivery.record_plans(repo, self.intents)

    def run(self, key: str) -> str:
        return oracle.run_id(self.case_id, self.runs, key)

    def register_materialised(self) -> None:
        for start in weeks.materialised_week_starts(
            self.host.tz,
            self.host.settings.reset_weekday,
            self.host.settings.reset_time,
            self.clock.now,
        ):
            for key, fixed_id in self.fixed.items():
                found = self.repo.run_for_fixed(fixed_id, start)
                if found is not None:
                    self.runs[f"{key}@{start.isoformat()}"] = found["id"]

    def common(self) -> dict[str, Handler]:
        repo = self.repo

        def create_run(step: dict) -> str:
            self.runs[step["run_key"]] = repo.create_run(
                at(step["week_start"]),
                step["bosses"],
                at(step["at"]),
                step["participants"],
                step["status"],
                step["source"],
                channel_id=step["channel_id"],
            )
            return self.runs[step["run_key"]]

        def set_rsvp(step: dict) -> str:
            repo.set_rsvp(self.run(step["run_key"]), step["user_id"], step["state"], step["source"])
            return step["state"]

        def set_status(step: dict) -> str:
            repo.set_run_status(self.run(step["run_key"]), step["status"])
            return step["status"]

        def set_clock(step: dict) -> str:
            self.clock.now = at(step["clock"])
            return self.clock.now.isoformat()

        def set_transport(step: dict) -> bool:
            self.host.fail_sends = step["fail_sends"]
            return step["fail_sends"]

        return {
            "create_run": create_run,
            "set_rsvp": set_rsvp,
            "set_status": set_status,
            "set_clock": set_clock,
            "set_transport": set_transport,
        }


def replay(
    case: dict[str, Any],
    gate: Callable[[object], None],
    handlers: Callable[[Session], dict[str, Handler]],
    extra_state: Callable[[Session], dict[str, Any]],
) -> dict[str, Any]:
    """Validate input first, then replay every step in one clean store."""
    gate(case)
    input_ = case["input"]
    clock, ids = oracle.Clock(input_["clock"]), oracle.Ids(input_["uuid_sequence"])
    with (
        oracle.seams(clock, ids, (materialise, client, weeks)),
        delivery.journal_seams(clock),
        oracle.quiet_logs(),
    ):
        repo = Repo(":memory:")
        try:
            session = Session(case, repo, clock)
            for member in input_["members"]:
                repo.upsert_member(
                    member["user_id"],
                    member["display_name"],
                    member["nickname"],
                    member["has_role"],
                )
                repo.set_ping_level(member["user_id"], member["ping_level"])
            table = {**session.common(), **handlers(session)}
            results = []
            for index, step in enumerate(input_["steps"]):
                handler = table[step["op"]]
                results.append(
                    oracle.run_step(
                        case["case_id"],
                        index,
                        step,
                        ids,
                        ERRORS,
                        lambda: handler(step),  # noqa: B023
                    )
                )
                if session.host.unexpected:
                    raise validation.ContractError(
                        f"{case['case_id']} step {index}: {'; '.join(session.host.unexpected)}"
                    )
            state = oracle.snapshot(repo, session.intents)
            state.update(extra_state(session))
            return {"steps": results, "final_state": state}
        finally:
            repo.close()


def run_async(coroutine: Any) -> Any:
    return asyncio.run(coroutine)
