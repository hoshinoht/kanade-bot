"""Replay mention vectors through ``bot.agent.pings`` and the card allow-list gate."""

from __future__ import annotations

from types import SimpleNamespace
from typing import Any

from bot.agent import formatting, pings
from bot.agent.client import BossBot
from bot.infrastructure.db import Repo

from .. import oracle
from . import validation

ERRORS: dict[str, type[Exception]] = {"ValueError": ValueError, "KeyError": KeyError}


def _ids(value: Any) -> list[str] | bool:
    if isinstance(value, bool) or not isinstance(value, list):
        return bool(value)
    return [str(item.id) for item in value]


def replay(case: dict[str, Any]) -> dict[str, Any]:
    """Replay one input case in a clean store; no UUID may be consumed."""
    validation.validate_input_case(case)
    case_id, input_ = case["case_id"], case["input"]
    clock, ids = oracle.Clock(input_["clock"]), oracle.Ids([])
    repo = Repo(":memory:")

    def resolve(step: dict) -> list[str]:
        return pings.resolve_mentions(repo, step["candidates"], step["kind"])

    def audience(step: dict) -> dict[str, Any]:
        who = pings.audience(repo, step["people"], step["kind"], candidates=step["candidates"])
        return {"mentioned": list(who.mentioned), "names": [[k, v] for k, v in who.names.items()]}

    def wants(step: dict) -> bool:
        return pings.wants_mention(step["level"], step["kind"])

    def normalise(step: dict) -> str:
        return pings.normalise_level(step["value"])

    def set_level(step: dict) -> str:
        return repo.set_ping_level(step["user_id"], step["level"])

    def not_declined(step: dict) -> list[str]:
        run = {"participants": step["participants"]}
        return formatting.not_declined(run, dict(step["rsvps"]))

    def everyone_on(step: dict) -> list[str]:
        return formatting.everyone_on([{"participants": people} for people in step["runs"]])

    def prepared(step: dict) -> dict[str, Any]:
        card = formatting.Card(content="card", mention_users=step["card_mentions"])
        host = SimpleNamespace(quiet_mode=step["quiet_mode"])
        _, allowed = BossBot._prepared(host, card, step["mention_users"])  # type: ignore[arg-type]
        return {
            "users": _ids(allowed.users),
            "roles": _ids(allowed.roles),
            # discord.py's unset default is a truthy sentinel, not a bool.
            "everyone": bool(allowed.everyone),
            "replied_user": bool(allowed.replied_user),
        }

    def kinds(step: dict) -> dict[str, list[str]]:
        return {
            "essential": sorted(pings.ESSENTIAL_KINDS),
            "informational": sorted(pings.INFORMATIONAL_KINDS),
        }

    handlers = {
        "resolve_mentions": resolve,
        "audience": audience,
        "wants_mention": wants,
        "normalise_level": normalise,
        "set_ping_level": set_level,
        "not_declined": not_declined,
        "everyone_on": everyone_on,
        "prepared_allow_list": prepared,
        "ping_kinds": kinds,
    }
    try:
        with oracle.seams(clock, ids, ()), oracle.quiet_logs():
            for member in input_["members"]:
                repo.upsert_member(
                    member["user_id"],
                    member["display_name"],
                    member["nickname"],
                    member["has_role"],
                )
                repo.set_ping_level(member["user_id"], member["ping_level"])
            results = []
            for index, step in enumerate(input_["steps"]):
                handler = handlers[step["op"]]
                results.append(
                    oracle.run_step(case_id, index, step, ids, ERRORS, lambda: handler(step))  # noqa: B023
                )
        return {"steps": results, "final_state": {"members": _members(repo, input_)}}
    finally:
        repo.close()


def _members(repo: Repo, input_: dict[str, Any]) -> list[dict[str, Any]]:
    known = [member["user_id"] for member in input_["members"]]
    extra = [
        step["user_id"]
        for step in input_["steps"]
        if step["op"] == "set_ping_level" and step["user_id"] not in known
    ]
    return [
        {
            "user_id": uid,
            "ping_level": repo.get_ping_level(uid),
            "known": repo.get_member(uid) is not None,
        }
        for uid in dict.fromkeys(known + extra)
    ]
