"""Committing confirmed cards: schedule effects, refusals, authority, supersede and expiry.

Replayed against one clean in-memory ``Repo`` per case with the scheduler
vectors' seams: a pinned aware clock (``set_clock`` moves it) and a
deterministic UUID sequence. ``rematerialise`` stands in for the live client's
``on_fixed_created`` callback by running the real ``BossBot.materialise_weeks``.
"""

from __future__ import annotations

from datetime import datetime, time
from types import SimpleNamespace
from typing import Any

from bot.agent import client, materialise
from bot.agent.client import BossBot
from bot.domain import weeks
from bot.extract import commit as commit_mod
from bot.infrastructure.db import Repo

from ...scheduler import oracle
from .. import fixtures
from ..contract import (
    BOOL,
    INSTANT,
    INT,
    NINSTANT,
    NSTR,
    NUM,
    STR,
    STRS,
    ContractError,
    Family,
    Op,
    arr,
    closed,
    nullable,
    validate_input,
)
from . import commit_cases

AMENDMENT_STATUSES = ["proposed", "confirmed", "rejected", "superseded", "expired"]
RESULT = closed(
    {
        "amendment_id": STR,
        "kind": STR,
        "applied": BOOL,
        "run_id": NSTR,
        "fixed_run_id": NSTR,
        "created_run_ids": STRS,
        "old_datetime": NINSTANT,
        "problem": NSTR,
        "superseded": STRS,
        "notes": STRS,
        "fixed_callbacks": STRS,
    }
)
STATE = closed(
    {
        "fixed_runs": arr({"type": "object"}),
        "runs": arr({"type": "object"}),
        "reminders": arr({"type": "object"}),
        "rsvps": arr({"type": "object"}),
        "amendments": arr(
            closed(
                {
                    "id": STR,
                    "kind": STR,
                    "status": {"enum": AMENDMENT_STATUSES},
                    "run_id": NSTR,
                    "new_datetime": NINSTANT,
                    "payload": {"type": "object"},
                }
            )
        ),
    }
)


def at(value: str | None) -> datetime | None:
    return datetime.fromisoformat(value) if value is not None else None


class Session:
    def __init__(self, case: dict[str, Any], repo: Repo, clock: oracle.Clock):
        self.case_id, self.input, self.repo, self.clock = (
            case["case_id"],
            case["input"],
            repo,
            clock,
        )
        self.tz = fixtures.zone(self.input["timezone"])
        self.reset_weekday = self.input["reset_weekday"]
        self.reset_time = time.fromisoformat(self.input["reset_time"])
        self.ping_time = time.fromisoformat(self.input["ping_time"])
        self.countdowns = list(self.input["countdowns"])
        self.host = SimpleNamespace(
            repo=repo,
            tz=self.tz,
            settings=SimpleNamespace(reset_weekday=self.reset_weekday, reset_time=self.reset_time),
            ping_time=self.ping_time,
            countdowns=self.countdowns,
        )
        self.keys: dict[str, str] = {}

    def ref(self, key: str | None) -> str | None:
        if key is None:
            return None
        try:
            return self.keys[key]
        except KeyError:
            raise ContractError(f"{self.case_id}: undefined key {key!r}") from None

    def bind(self, key: str, value: str) -> str:
        if key in self.keys:
            raise ContractError(f"{self.case_id}: key {key!r} is not unique")
        self.keys[key] = value
        return value

    def amendment(self, key: str) -> dict[str, Any]:
        row = self.repo.get_amendment(self.ref(key))
        if row is None:  # pragma: no cover - rows are never deleted
            raise ContractError(f"{self.case_id}: amendment {key!r} vanished")
        return row

    def handlers(self) -> dict[str, Any]:
        repo = self.repo

        def create_run(s: dict) -> str:
            return self.bind(
                s["run_key"],
                repo.create_run(
                    at(s["week_start"]),
                    s["bosses"],
                    at(s["at"]),
                    s["participants"],
                    s["status"],
                    "manual",
                    channel_id=s["channel_id"],
                ),
            )

        def add_fixed(s: dict) -> str:
            return self.bind(
                s["fixed_key"],
                repo.add_fixed_run(
                    s["owner_id"],
                    s["bosses"],
                    s["weekday"],
                    s["time"],
                    s["participants"],
                    channel_id=s["channel_id"],
                ),
            )

        def propose(s: dict) -> str:
            payload = dict(s["payload"])
            if s["fixed_key"] is not None:
                payload["fixed_run_id"] = self.ref(s["fixed_key"])
            return self.bind(
                s["amendment_key"],
                repo.create_amendment(
                    at(s["week_start"]),
                    s["kind"],
                    bosses=s["bosses"],
                    run_id=self.ref(s["run_key"]),
                    new_datetime=at(s["new_datetime"]),
                    participants=s["participants"],
                    confidence=s["confidence"],
                    channel_id=s["channel_id"],
                    is_question=s["is_question"],
                    rsvp=s["rsvp"],
                    day_ref=s["day_ref"],
                    time_ref=s["time_ref"],
                    payload=payload,
                ),
            )

        def set_rsvp(s: dict) -> str:
            repo.set_rsvp(self.ref(s["run_key"]), s["user_id"], s["state"], "reaction")
            return s["state"]

        def set_clock(s: dict) -> str:
            self.clock.now = at(s["clock"])
            return self.clock.now.isoformat()

        def rematerialise(s: dict) -> None:
            BossBot.materialise_weeks(self.host)  # type: ignore[arg-type]

        def find_fixed_run(s: dict) -> str:
            found = repo.run_for_fixed(self.ref(s["fixed_key"]), at(s["week_start"]))
            if found is None:
                raise ContractError(f"{self.case_id}: no run for {s['fixed_key']!r} that week")
            return self.bind(s["run_key"], found["id"])

        def may_commit(s: dict) -> bool:
            run_id = self.ref(s["run_key"])
            return commit_mod.may_commit(
                self.amendment(s["amendment_key"]),
                repo.get_run(run_id) if run_id else None,
                s["user_id"],
                has_role=s["has_role"],
                is_admin=s["is_admin"],
                is_owner=s["is_owner"],
            )

        def commit(s: dict) -> dict[str, Any]:
            callbacks: list[str] = []

            def created(fixed_id: str) -> None:
                callbacks.append(fixed_id)
                BossBot.materialise_weeks(self.host)  # type: ignore[arg-type]

            result = commit_mod.commit(
                repo,
                self.amendment(s["amendment_key"]),
                self.tz,
                self.reset_weekday,
                self.reset_time,
                self.ping_time,
                self.countdowns,
                s["actor_id"],
                channel_id=s["channel_id"],
                on_fixed_created=created if s["rematerialise"] else None,
            )
            old = result.old_datetime
            return {
                "amendment_id": result.amendment_id,
                "kind": result.kind,
                "applied": result.applied,
                "run_id": result.run_id,
                "fixed_run_id": result.fixed_run_id,
                "created_run_ids": list(result.created_run_ids),
                "old_datetime": old.isoformat() if isinstance(old, datetime) else None,
                "problem": result.problem,
                "superseded": [row["id"] for row in result.superseded],
                "notes": list(result.notes),
                "fixed_callbacks": callbacks,
            }

        def supersede(s: dict) -> list[str]:
            retired = commit_mod.supersede(
                repo,
                run_id=self.ref(s["run_key"]),
                channel_id=s["channel_id"],
                bosses=s["bosses"],
                keep_id=self.ref(s["keep_key"]),
                from_channel=s["from_channel"],
            )
            return [row["id"] for row in retired]

        def reject(s: dict) -> str:
            commit_mod.reject(repo, self.amendment(s["amendment_key"]))
            return self.amendment(s["amendment_key"])["status"]

        def expire(s: dict) -> list[str]:
            return [row["id"] for row in commit_mod.expire_stale(repo, self.clock.now)]

        return {
            "create_run": create_run,
            "add_fixed": add_fixed,
            "propose": propose,
            "set_rsvp": set_rsvp,
            "set_clock": set_clock,
            "rematerialise": rematerialise,
            "find_fixed_run": find_fixed_run,
            "may_commit": may_commit,
            "commit": commit,
            "supersede": supersede,
            "reject": reject,
            "expire_stale": expire,
        }


def _state(repo: Repo) -> dict[str, Any]:
    state = oracle.snapshot(repo, [])
    del state["side_effects"]
    state["amendments"] = [
        {
            "id": row["id"],
            "kind": row["kind"],
            "status": row["status"],
            "run_id": row["run_id"],
            "new_datetime": row["new_datetime"].isoformat() if row["new_datetime"] else None,
            "payload": row["payload"],
        }
        for row in repo.list_amendments()
    ]
    return state


def replay(case: dict[str, Any]) -> dict[str, Any]:
    validate_input(FAMILY, case)
    input_ = case["input"]
    clock, ids = oracle.Clock(input_["clock"]), oracle.Ids(input_["uuid_sequence"])
    with oracle.seams(clock, ids, (materialise, client, weeks, commit_mod)), oracle.quiet_logs():
        repo = Repo(":memory:")
        try:
            session = Session(case, repo, clock)
            for member in input_["members"]:
                repo.upsert_member(member, member, None, True)
            table = session.handlers()
            results = []
            for index, step in enumerate(input_["steps"]):
                handler = table[step["op"]]
                results.append(
                    oracle.run_step(
                        case["case_id"],
                        index,
                        step,
                        ids,
                        {"ValueError": ValueError},
                        lambda: handler(step),  # noqa: B023
                    )
                )
            return {"steps": results, "final_state": _state(repo)}
        finally:
            repo.close()


KEY = {"type": "string", "pattern": "^[a-z0-9][a-z0-9_-]*$"}
NKEY = nullable(KEY)
OPS = {
    "create_run": Op(
        {
            "run_key": KEY,
            "week_start": INSTANT,
            "at": INSTANT,
            "bosses": STRS,
            "participants": STRS,
            "status": {"enum": fixtures.RUN_STATUSES},
            "channel_id": NSTR,
        },
        STR,
    ),
    "add_fixed": Op(
        {
            "fixed_key": KEY,
            "owner_id": STR,
            "bosses": STRS,
            "weekday": {"type": "integer", "minimum": 0, "maximum": 6},
            "time": STR,
            "participants": STRS,
            "channel_id": NSTR,
        },
        STR,
    ),
    "propose": Op(
        {
            "amendment_key": KEY,
            "kind": {"enum": fixtures.KINDS},
            "week_start": INSTANT,
            "run_key": NKEY,
            "fixed_key": NKEY,
            "bosses": STRS,
            "new_datetime": NINSTANT,
            "participants": STRS,
            "confidence": NUM,
            "channel_id": NSTR,
            "is_question": BOOL,
            "rsvp": NSTR,
            "day_ref": NSTR,
            "time_ref": NSTR,
            "payload": {"type": "object"},
        },
        STR,
    ),
    "set_rsvp": Op(
        {"run_key": KEY, "user_id": STR, "state": {"enum": ["yes", "no", "maybe"]}}, STR
    ),
    "set_clock": Op({"clock": INSTANT}, INSTANT),
    "rematerialise": Op({}, {"type": "null"}),
    "find_fixed_run": Op({"fixed_key": KEY, "week_start": INSTANT, "run_key": KEY}, STR),
    "may_commit": Op(
        {
            "amendment_key": KEY,
            "run_key": NKEY,
            "user_id": STR,
            "has_role": BOOL,
            "is_admin": BOOL,
            "is_owner": BOOL,
        },
        BOOL,
    ),
    "commit": Op(
        {"amendment_key": KEY, "actor_id": STR, "channel_id": NSTR, "rematerialise": BOOL}, RESULT
    ),
    "supersede": Op(
        {
            "run_key": NKEY,
            "channel_id": NSTR,
            "bosses": STRS,
            "keep_key": NKEY,
            "from_channel": NSTR,
        },
        STRS,
    ),
    "reject": Op({"amendment_key": KEY}, STR),
    "expire_stale": Op({}, STRS),
}
FAMILY = Family(
    name="commit",
    version="v5-extract-commit-v1",
    provenance={
        "oracle": "legacy/python bot.extract.commit against an in-memory Repo",
        "functions": [
            "bot.extract.commit.commit",
            "bot.extract.commit.may_commit",
            "bot.extract.commit.supersede",
            "bot.extract.commit.reject",
            "bot.extract.commit.expire_stale",
            "bot.agent.client.BossBot.materialise_weeks",
        ],
        "source_tests": ["tests/test_extract_commit.py"],
        "inventory_surfaces": [
            "extract.commit.move",
            "extract.commit.add",
            "extract.commit.cancel",
            "extract.commit.otot",
            "extract.commit.sub",
            "extract.commit.split",
            "extract.commit.rsvp",
            "extract.commit.fix",
            "extract.commit.unfix",
            "extract.commit.refix",
        ],
    },
    context={
        "clock": INSTANT,
        "timezone": STR,
        "reset_weekday": {"type": "integer", "minimum": 0, "maximum": 6},
        "reset_time": STR,
        "ping_time": STR,
        "countdowns": arr(INT),
        "members": STRS,
        "uuid_sequence": arr({"type": "string", "format": "uuid"}),
    },
    ops=OPS,
    cases=commit_cases.cases,
    replay=replay,
    state=STATE,
)
