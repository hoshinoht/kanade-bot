"""The model-visible tool surface and the loop's fixed constants.

``text`` is the exact compact serialization in v4 key order (what
``ChatPilot._budgeted_messages`` estimates; the file's sorted keys cannot carry
that order), and ``tokens`` is ``estimate_tokens`` over it. ``read`` is the
surface offered on a read-only (rejection follow-up) turn.
"""

from __future__ import annotations

import json
from typing import Any

from bot.chat import agent, gate, tools
from bot.chat.tools import contracts
from bot.extract.prompt import estimate_tokens

from ..contract import INT, STR, STRS, Family, Op, arr, closed, replay_steps

SURFACE = closed(
    {
        "tools": arr({"type": "object"}, 1),
        "text": STR,
        "tokens": INT,
        "names": STRS,
        "write_names": STRS,
    }
)
CONSTANTS = closed(
    {
        "max_tool_rounds": INT,
        "history_exchanges": INT,
        "reply_chain_depth": INT,
        "conversation_budget_tokens": INT,
        "completion_reserve_tokens": INT,
        "anchor_cache": INT,
        "reference_cache": INT,
        "max_runs": INT,
        "max_member_reply": INT,
        "unknown_tool": STR,
        "read_only_turn": STR,
        "failure_reply": STR,
        "strategy_grounding_failure_reply": STR,
        "rate_limited_reply": STR,
        "pool_spent_reply": STR,
        "spoofed_note": STR,
        "reactions": closed({"seen": STR, "rate_limited": STR, "channel_busy": STR}),
        "pool_spent_reason": STR,
    }
)


def surface(read_only: bool) -> dict[str, Any]:
    offered = tools.read_tools() if read_only else tools.TOOLS
    text = json.dumps(offered, ensure_ascii=False, default=str, separators=(",", ":"))
    names = [tool["function"]["name"] for tool in offered]
    return {
        "tools": offered,
        "text": text,
        "tokens": estimate_tokens(text),
        "names": names,
        "write_names": [name for name in names if tools.is_write_tool(name)],
    }


def constants() -> dict[str, Any]:
    return {
        "max_tool_rounds": agent.MAX_TOOL_ROUNDS,
        "history_exchanges": agent.HISTORY_EXCHANGES,
        "reply_chain_depth": agent.REPLY_CHAIN_DEPTH,
        "conversation_budget_tokens": agent.CONVERSATION_BUDGET_TOKENS,
        "completion_reserve_tokens": agent.COMPLETION_RESERVE_TOKENS,
        "anchor_cache": agent.ANCHOR_CACHE,
        "reference_cache": agent.REFERENCE_CACHE,
        "max_runs": contracts.MAX_RUNS,
        "max_member_reply": contracts.MAX_MEMBER_REPLY,
        "unknown_tool": contracts.UNKNOWN_TOOL,
        "read_only_turn": contracts.READ_ONLY_TURN,
        "failure_reply": agent.FAILURE_REPLY,
        "strategy_grounding_failure_reply": agent.STRATEGY_GROUNDING_FAILURE_REPLY,
        "rate_limited_reply": agent.RATE_LIMITED_REPLY,
        "pool_spent_reply": agent.POOL_SPENT_REPLY,
        "spoofed_note": agent.SPOOFED_NOTE,
        "reactions": {
            "seen": gate.SEEN_REACTION,
            "rate_limited": gate.RATE_LIMITED_REACTION,
            "channel_busy": gate.CHANNEL_BUSY_REACTION,
        },
        "pool_spent_reason": gate.POOL_SPENT,
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    return replay_steps(
        FAMILY,
        case,
        {
            "surface": lambda s: surface(s["read_only"]),
            "constants": lambda s: constants(),
        },
    )


def cases() -> list[dict[str, Any]]:
    return [
        {
            "case_id": "surface",
            "input": {
                "steps": [
                    {"op": "surface", "read_only": False},
                    {"op": "surface", "read_only": True},
                    {"op": "constants"},
                ]
            },
        }
    ]


FAMILY = Family(
    name="tool_schemas",
    version="v5-chat-tool-schemas-v1",
    provenance={
        "oracle": "legacy/python bot.chat.tools.schemas, bot.chat.agent",
        "functions": [
            "bot.chat.tools.schemas.TOOLS",
            "bot.chat.tools.schemas.tool_names",
            "bot.chat.tools.dispatching.read_tools",
            "bot.chat.tools.dispatching.is_write_tool",
            "bot.chat.tools.contracts.UNKNOWN_TOOL",
            "bot.chat.tools.contracts.READ_ONLY_TURN",
            "bot.chat.agent.MAX_TOOL_ROUNDS",
            "bot.extract.prompt.estimate_tokens",
        ],
        "source_tests": ["tests/test_chat_tools.py", "tests/test_chat_injection.py"],
        "inventory_surfaces": [],
    },
    context={},
    ops={
        "surface": Op({"read_only": {"type": "boolean"}}, SURFACE),
        "constants": Op({}, CONSTANTS),
    },
    cases=cases,
    replay=replay,
)
