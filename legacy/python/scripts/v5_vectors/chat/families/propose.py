"""Write tools: what each ``propose_*`` call creates, and every refusal before it does.

A proposal goes through the real ``Pipeline.apply_plan`` (supersede, row,
journalled card post) against the seeded world; the schedule itself never
changes. Each step records the dispatcher outcome and every amendment row
afterwards (timestamps and native message IDs reduced to ``card_posted``), so
supersession of an older card shows up as a status change.
"""

from __future__ import annotations

from typing import Any

from .. import fixtures, host
from ..contract import (
    BOOL,
    INSTANT,
    NINSTANT,
    NSTR,
    NUM,
    STR,
    STRS,
    Family,
    Op,
    arr,
    closed,
    run_steps,
    validate_input,
)
from .read_tools import call, tool_step_fields

AMENDMENT = closed(
    {
        "id": fixtures.UUID,
        "kind": {"enum": ["move", "add", "cancel", "rsvp", "fix"]},
        "status": {"enum": ["proposed", "superseded"]},
        "week_start": INSTANT,
        "run_id": {"anyOf": [fixtures.UUID, {"type": "null"}]},
        "new_datetime": NINSTANT,
        "bosses": STRS,
        "participants": STRS,
        "rsvp": {"enum": ["yes", "no", None]},
        "confidence": NUM,
        "channel_id": NSTR,
        "is_question": BOOL,
        "evidence_msg_ids": STRS,
        "summary": STR,
        "payload": {"type": "object"},
        "card_posted": BOOL,
    }
)
RESULT = closed({"outcome": {"$ref": "#/$defs/tool_outcome"}, "amendments": arr(AMENDMENT)})


def replay(case: dict[str, Any]) -> dict[str, Any]:
    validate_input(FAMILY, case)
    with host.session(case) as s:

        def propose(step: dict) -> dict[str, Any]:
            outcome = host.run_tool(s.host, step, case["input"])
            return {"outcome": outcome, "amendments": host.amendments(s.repo)}

        return run_steps(case, {"run": propose})


def cases() -> list[dict[str, Any]]:
    f = fixtures
    bot = f.BOT_USER

    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {"case_id": case_id, "input": {**f.base_context(), "steps": steps}}

    return [
        case(
            "move",
            [
                call("propose_move", {"run_query": "hstar", "to_when": "wed 21:30"}),
                call("propose_move", {"run_query": "hstar", "to_when": ""}),
                call("propose_move", {"run_query": "hstar", "to_when": "whenever"}),
                call("propose_move", {"run_query": "hstar", "to_when": "wed 09:00"}),
                call("propose_move", {"run_query": "hstar", "to_when": "thu 22:00"}, "33"),
                call(
                    "propose_move",
                    {"run_query": "hbaldrix", "to_when": "sat 21:00"},
                    channel=f.ADOPTED,
                ),
                call(
                    "propose_move",
                    {"run_query": "hstar", "to_when": "thu 22:00"},
                    channel=f.PARTY_STAR,
                ),
                # A second card for the same run supersedes the first.
                call(
                    "propose_move",
                    {"run_query": "a1a1a1a1", "to_when": "tomorrow 9:45pm"},
                    "22",
                    channel=f.PARTY_STAR,
                ),
            ],
        ),
        case(
            "add-one-off-and-weekly",
            [
                call("propose_add", {"boss": "hlimbo", "when": "sat 21:00"}),
                call(
                    "propose_add",
                    {
                        "boss": "nlimbo",
                        "when": "sun 20:00",
                        "participants": f"<@{bot}> me and kanon",
                        "weekly": "false",
                    },
                ),
                call(
                    "propose_add",
                    {
                        "boss": "xkalos",
                        "when": "thu 23:00",
                        "participants": ["kanon", "Priya"],
                        "weekly": True,
                    },
                    "22",
                    channel=f.ADOPTED,
                ),
                call("propose_add", {"boss": "limbo", "when": "sat 21:00"}),
                call("propose_add", {"boss": "hlimbo"}),
                call("propose_add", {"boss": "hlimbo", "when": "yesterday 21:00"}),
                call("propose_add", {"boss": "hlimbo", "when": "sat 21:00", "participants": "Zed"}),
                call(
                    "propose_add",
                    {"boss": "hlimbo", "when": "sat 21:00", "participants": "NotABosser"},
                ),
            ],
        ),
        case(
            "cancel",
            [
                call("propose_cancel", {"run_query": "hstar"}, "22"),
                call("propose_cancel", {"run_query": "limbo"}, "11"),
                call("propose_cancel", {"run_query": "hcarling"}, "11", channel=f.ADOPTED),
                call("propose_cancel", {"run_query": ""}),
                call("propose_cancel", {"run_query": "hcarling"}, "44", channel=f.ADOPTED),
            ],
        ),
        case(
            "rsvp-for-the-speaker-only",
            [
                call("propose_rsvp", {"run_query": "hbaldrix", "answer": "No"}),
                call(
                    "propose_rsvp",
                    {"run_query": "hstar", "answer": "yes", "user": "22", "participant": "22"},
                    channel=f.PARTY_STAR,
                ),
                call("propose_rsvp", {"run_query": "hstar", "answer": "maybe"}),
                call("propose_rsvp", {"run_query": "hbaldrix", "answer": "yes"}, "44"),
                call("propose_rsvp", {"run_query": "hstar", "answer": "yes"}, "33"),
            ],
        ),
        case(
            "remove-weekly",
            [
                call("propose_remove_fixed", {"query": "hstar"}),
                call("propose_remove_fixed", {"query": "tuesday"}, "22", channel=f.PARTY_KALOS),
                call("propose_remove_fixed", {"query": "kalos"}),
                call("propose_remove_fixed", {"query": "limbo"}),
                call("propose_remove_fixed", {"query": "sunday"}),
                call("propose_remove_fixed", {"query": ""}),
                call("propose_remove_fixed", {"query": "f3f3f3f3"}, "44"),
            ],
        ),
        case(
            "change-weekly",
            [
                call("propose_change_fixed", {"query": "hstar", "day": "thu"}),
                call("propose_change_fixed", {"query": "hstar", "time": "22:15"}, "22"),
                call(
                    "propose_change_fixed",
                    {"query": "hbaldrix", "participants": "me and Priya"},
                    "44",
                ),
                call(
                    "propose_change_fixed",
                    {"query": "hbaldrix", "day": "fri", "time": "21:00", "participants": "Alvin"},
                ),
                call("propose_change_fixed", {"query": "hstar", "day": "someday"}),
                call("propose_change_fixed", {"query": "hstar", "time": "25:99"}),
                call("propose_change_fixed", {"query": "kalos", "day": "wed"}),
            ],
        ),
    ]


FAMILY = Family(
    name="propose",
    version="v5-chat-propose-v1",
    provenance={
        "oracle": "legacy/python bot.chat.tools (write handlers) + bot.extract.pipeline",
        "functions": [
            "bot.chat.tools.dispatching.run",
            "bot.chat.tools.propose_move.handle",
            "bot.chat.tools.propose_add.handle",
            "bot.chat.tools.propose_cancel.handle",
            "bot.chat.tools.propose_rsvp.handle",
            "bot.chat.tools.propose_remove_fixed.handle",
            "bot.chat.tools.propose_change_fixed.handle",
            "bot.chat.tools.proposals.propose",
            "bot.chat.tools.resolution.resolve_fixed",
            "bot.extract.pipeline.Pipeline.apply_plan",
            "bot.api.service.parse_when",
        ],
        "source_tests": [
            "tests/test_chat_tools.py",
            "tests/test_chat_add.py",
            "tests/test_chat_weekly.py",
            "tests/test_chat_change_fixed.py",
            "tests/test_chat_remove_fixed.py",
            "tests/test_chat_authority.py",
            "tests/test_chat_injection.py",
        ],
        "inventory_surfaces": [],
    },
    context=fixtures.HOST_CONTEXT,
    defs={**fixtures.HOST_DEFS, "tool_outcome": fixtures.OUTCOME_DEF},
    ops={"run": Op(tool_step_fields(), RESULT, optional=fixtures.TOOL_CONTEXT_OPTIONAL)},
    cases=cases,
    replay=replay,
)
