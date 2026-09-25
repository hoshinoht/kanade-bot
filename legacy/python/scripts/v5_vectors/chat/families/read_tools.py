"""Read tools through the guarded dispatcher: exact text the model reads back.

Every step is one ``bot.chat.tools.run`` call with a trusted context (author,
channel, admin and read-only flags, and the schedule-scope defaults
``ChatPilot._answer`` derives from the message). The dispatcher never raises:
refusals, unknown tools and read-only write attempts are outcomes. Strategy
guides come from a stand-in store (``<guide ...>``); guide rendering is not a
chat contract. ``set_clock`` moves the one pinned instant every seam reads.
"""

from __future__ import annotations

from datetime import datetime
from typing import Any

from .. import fixtures, host
from ..contract import ANY, INSTANT, STR, Family, Op, run_steps, validate_input

_EMPTY = object()


def tool_step_fields() -> dict[str, Any]:
    return {"tool": STR, "arguments": ANY, **fixtures.TOOL_CONTEXT}


def replay(case: dict[str, Any]) -> dict[str, Any]:
    validate_input(FAMILY, case)
    with host.session(case) as s:

        def set_clock(step: dict) -> str:
            s.clock.now = datetime.fromisoformat(step["clock"])
            return s.clock.now.isoformat()

        return run_steps(
            case,
            {
                "run": lambda step: host.run_tool(s.host, step, case["input"]),
                "set_clock": set_clock,
            },
        )


def call(tool: str, arguments: Any = _EMPTY, author: str = "11", **context: Any) -> dict[str, Any]:
    return {
        "op": "run",
        "tool": tool,
        "arguments": {} if arguments is _EMPTY else arguments,
        "author_id": author,
        "channel_id": context.pop("channel", fixtures.CHAT_CHANNEL),
        **context,
    }


def schedule(author: str = "11", **context: Any) -> dict[str, Any]:
    arguments = {key: context.pop(key) for key in list(context) if key in _SCHEDULE_ARGS}
    return call("get_schedule", arguments, author, **context)


_SCHEDULE_ARGS = ("week", "week_basis", "scope", "participant", "day")


def cases() -> list[dict[str, Any]]:
    bot = fixtures.BOT_USER

    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {"case_id": case_id, "input": {**fixtures.base_context(), "steps": steps}}

    return [
        case(
            "schedule-weeks-and-days",
            [
                schedule(),
                schedule(week="next"),
                schedule(week="this_boss"),
                schedule(week="next_boss"),
                schedule(week="this", week_basis="boss"),
                schedule(week="auto", day="fri"),
                schedule(week="auto", day="tomorrow"),
                schedule(week="auto", day="tonight"),
                schedule(week="this", day="tuesday"),
                schedule(week="next", day="next"),
                schedule(week="this_boss", day="sat"),
                schedule(week="later"),
                schedule(week="this", week_basis="lunar"),
                schedule(day="someday"),
                schedule(day="   "),
            ],
        ),
        case(
            "schedule-scope-and-people",
            [
                schedule(scope="channel"),
                schedule(scope="channel", channel=fixtures.GENERAL),
                schedule(scope="everywhere"),
                schedule(participant="me", author="22"),
                schedule(participant="kanon"),
                schedule(participant="Priya", scope="channel"),
                schedule(participant="Mei", week="next_boss", scope="channel"),
                schedule(participant=f"<@{bot}>", author="33"),
                schedule(participant="Zed"),
                schedule(participant="Mei", week="next"),
                schedule(scope="all", force_channel_scope=True),
                schedule(scope="channel", force_all_channels=True),
                schedule(participant="kanon", force_group_schedule=True),
                schedule(upcoming_only=True),
                schedule(upcoming_only=True, scope="channel", channel=fixtures.GENERAL),
                schedule(upcoming_only=True, participant="me", author="22", day="tue"),
            ],
        ),
        case(
            "schedule-after-the-week",
            [
                {"op": "set_clock", "clock": "2026-09-13T23:30:00+08:00"},
                schedule(),
                schedule(upcoming_only=True),
                schedule(upcoming_only=True, participant="me", scope="channel"),
            ],
        ),
        case(
            "run-lookup",
            [
                call("get_run", {"query": "hstar"}),
                call("get_run", {"query": "a1a1a1a1"}),
                call("get_run", {"query": "hstar wednesday"}),
                call("get_run", {"query": "star fri"}),
                call("get_run", {"query": "friday"}),
                call("get_run", {"query": "tonight"}),
                call("get_run", {"query": "kalos"}),
                call("get_run", {"query": "limbo"}),
                call("get_run", {"query": "hstar hbaldrix"}),
                call("get_run", {"query": "wednesday friday"}),
                call("get_run", {"query": "gibberish"}),
                call("get_run", {"query": ""}),
                call("get_run", {}),
            ],
        ),
        case(
            "catalogue-weekly-pending",
            [
                call("list_bosses"),
                call("list_fixed"),
                call("get_pending"),
            ],
        ),
        case(
            "boss-strategy",
            [
                call("get_boss_strategy", {"boss": "hstar"}),
                call("get_boss_strategy", {"boss": "MaleficStar", "difficulty": "hard"}),
                call("get_boss_strategy", {"boss": "kalos"}),
                call("get_boss_strategy", {"boss": "kalos", "difficulty": "x"}),
                call("get_boss_strategy", {"boss": "hstar", "difficulty": "normal"}),
                call("get_boss_strategy", {"boss": "kalos", "difficulty": "hard"}),
                call("get_boss_strategy", {"boss": "kalos", "difficulty": "weird"}),
                call("get_boss_strategy", {"boss": "kalos", "difficulty": " "}),
                call("get_boss_strategy", {"boss": "carling"}),
                call("get_boss_strategy", {"boss": "lotus"}),
                call("get_boss_strategy", {"boss": ""}),
                call("get_boss_strategy", {"boss": 7}),
            ],
        ),
        case(
            "dispatch-boundary",
            [
                call("delete_run", {"run_query": "hstar"}),
                call("", {}),
                call("propose_add", {"boss": "hlimbo", "when": "sat 21:00"}, read_only=True),
                call("propose_cancel", {"run_query": "hstar"}, read_only=True),
                call("get_schedule", {}, read_only=True),
                call("get_run", '{"query": "hbaldrix"}'),
                call("get_run", "{not json"),
                call("get_run", '["hstar"]'),
                call("get_run", ["hstar"]),
                call("get_run", None),
                call("list_fixed", {"ignored": "extra arguments are ignored"}),
            ],
        ),
    ]


FAMILY = Family(
    name="read_tools",
    version="v5-chat-read-tools-v1",
    provenance={
        "oracle": "legacy/python bot.chat.tools (read handlers and dispatch)",
        "functions": [
            "bot.chat.tools.dispatching.run",
            "bot.chat.tools.get_schedule.handle",
            "bot.chat.tools.get_run.handle",
            "bot.chat.tools.list_bosses.handle",
            "bot.chat.tools.list_fixed.handle",
            "bot.chat.tools.get_pending.handle",
            "bot.chat.tools.get_boss_strategy.handle",
            "bot.chat.tools.resolution.resolve_run",
            "bot.chat.tools.rendering.run_line",
            "bot.chat.tools.rendering.run_detail",
        ],
        "source_tests": [
            "tests/test_chat_tools.py",
            "tests/test_chat_scope.py",
            "tests/test_chat_strategy.py",
            "tests/test_chat_injection.py",
            "tests/test_chat_followup.py",
        ],
        "inventory_surfaces": [],
    },
    context=fixtures.HOST_CONTEXT,
    defs={**fixtures.HOST_DEFS, "tool_outcome": fixtures.OUTCOME_DEF},
    ops={
        "run": Op(
            tool_step_fields(),
            {"$ref": "#/$defs/tool_outcome"},
            optional=fixtures.TOOL_CONTEXT_OPTIONAL,
        ),
        "set_clock": Op({"clock": INSTANT}, INSTANT),
    },
    cases=cases,
    replay=replay,
)
