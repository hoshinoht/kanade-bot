"""The agent loop over a scripted provider: ``ChatPilot.generate`` end to end.

Each step hands the pilot a ready conversation and a queue of raw provider
completions (or a raised transport error) and records every request body the
pilot sent -- with tool schemas reduced to their names; their bytes are the
``tool_schemas`` family -- plus the shaped ``Generation``: reply, rounds, tool
outcomes with their round, model-round diagnostics, summed usage, cards and
the channel focus a posted card leaves. Latencies are wall-clock and omitted.
Strategy prefetch is not exercised (it needs real boss-knowledge documents).
"""

from __future__ import annotations

import asyncio
import copy
from typing import Any

from bot.infrastructure.llm import Capabilities, ModelUnavailable

from .. import fixtures, host
from ..contract import (
    ANY,
    BOOL,
    INT,
    NSTR,
    STR,
    STRS,
    ContractError,
    Family,
    Op,
    arr,
    closed,
    nullable,
    ref,
    run_steps,
    validate_input,
)

RAISES: dict[str, type[Exception]] = {
    "TimeoutError": TimeoutError,
    "ModelUnavailable": ModelUnavailable,
    "RuntimeError": RuntimeError,
}
REPLY = {
    "oneOf": [
        closed({"completion": ANY}),
        closed({"raise": {"enum": list(RAISES)}, "message": STR}),
    ]
}
CAPS = closed(
    {
        "structured_output": BOOL,
        "sampling_controls": BOOL,
        "reasoning_control": BOOL,
        "function_tools": BOOL,
        "reasoning_efforts": nullable(STRS),
    }
)
MESSAGE = {"type": "object", "required": ["role"]}
OUTCOME = closed({**fixtures.OUTCOME_DEF["properties"], "round": INT})
GENERATION = closed(
    {
        "reply": STR,
        "rounds": INT,
        "tool_calls": STRS,
        "outcomes": arr(OUTCOME),
        "error": NSTR,
        "model_rounds": arr(
            closed({"round": INT, "content": NSTR, "thinking": NSTR, "requested_tools": STRS})
        ),
        "prompt_tokens": nullable(INT),
        "completion_tokens": nullable(INT),
        "created": STRS,
        "posted": STRS,
        "focus": STR,
        "requests": arr({"type": "object"}),
        "unused_replies": INT,
    }
)


class ScriptedClient:
    """A gateway stand-in: fixed capabilities and scripted completions, in order."""

    def __init__(self, case_id: str, capabilities: Capabilities):
        self.case_id = case_id
        self.capabilities = capabilities
        self.replies: list[dict[str, Any]] = []
        self.requests: list[dict[str, Any]] = []
        self.ran_out = False

    async def profile(self, alias: str) -> Capabilities:
        return self.capabilities

    async def chat(self, **body: Any) -> Any:
        recorded = copy.deepcopy(body)
        if "tools" in recorded:
            recorded["tools"] = [tool["function"]["name"] for tool in recorded["tools"]]
        self.requests.append(recorded)
        if not self.replies:
            # ``generate`` swallows every exception, so the replay re-raises this.
            self.ran_out = True
            raise ContractError(f"{self.case_id}: the scripted replies ran out")
        reply = self.replies.pop(0)
        if "raise" in reply:
            raise RAISES[reply["raise"]](reply["message"])
        return copy.deepcopy(reply["completion"])


def capabilities(raw: dict[str, Any]) -> Capabilities:
    efforts = raw["reasoning_efforts"]
    return Capabilities(
        structured_output=raw["structured_output"],
        sampling_controls=raw["sampling_controls"],
        reasoning_control=raw["reasoning_control"],
        function_tools=raw["function_tools"],
        reasoning_efforts=tuple(efforts) if efforts is not None else None,
    )


def replay(case: dict[str, Any]) -> dict[str, Any]:
    validate_input(FAMILY, case)
    input_ = case["input"]
    with host.session(case) as s:
        client = ScriptedClient(case["case_id"], capabilities(input_["caps"]))
        pilot = host.Pilot(s.host, client=client, clock=host.Monotonic())

        def generate(step: dict) -> dict[str, Any]:
            client.replies = list(step["replies"])
            client.requests = []
            ctx = host.tool_context(s.host, step, input_)
            result = asyncio.run(pilot.generate(copy.deepcopy(step["conversation"]), ctx))
            if client.ran_out:
                raise ContractError(f"{case['case_id']}: the scripted replies ran out")
            return {
                "reply": result.reply,
                "rounds": result.rounds,
                "tool_calls": list(result.tool_calls),
                "outcomes": [
                    {**host.outcome(outcome), "round": outcome.round} for outcome in result.outcomes
                ],
                "error": result.error,
                "model_rounds": copy.deepcopy(result.model_rounds),
                "prompt_tokens": result.prompt_tokens,
                "completion_tokens": result.completion_tokens,
                "created": list(result.created),
                "posted": list(result.posted),
                "focus": pilot.focus(step["channel_id"]),
                "requests": client.requests,
                "unused_replies": len(client.replies),
            }

        return run_steps(case, {"generate": generate})


# --- scripted completions -------------------------------------------------------------


def completion(message: dict[str, Any], finish: str = "stop", usage: Any = None) -> dict:
    out: dict[str, Any] = {"choices": [{"message": message, "finish_reason": finish}]}
    if usage is not None:
        out["usage"] = usage
    return {"completion": out}


def says(text: Any, *, usage: Any = None, reasoning: str | None = None) -> dict[str, Any]:
    message: dict[str, Any] = {"role": "assistant", "content": text}
    if reasoning is not None:
        message["reasoning"] = reasoning
    return completion(message, usage=usage)


def tool(name: str, arguments: Any, call_id: Any = "") -> dict[str, Any]:
    out: dict[str, Any] = {
        "type": "function",
        "function": {"name": name, "arguments": arguments},
    }
    if call_id != "":
        out["id"] = call_id
    return out


def wants(*calls: dict[str, Any], content: Any = None, usage: Any = None) -> dict[str, Any]:
    return completion(
        {"role": "assistant", "content": content, "tool_calls": list(calls)},
        finish="tool_calls",
        usage=usage,
    )


def call(name: str, call_id: str, **arguments: Any) -> dict[str, Any]:
    import json

    return tool(name, json.dumps(arguments), call_id)


CONVERSATION = [
    {"role": "system", "content": "SYSTEM (assembled elsewhere; see the context family)"},
    {"role": "user", "content": "Alvin tan: what's on this week?"},
]


def generate(
    replies: list[dict[str, Any]],
    *,
    author: str = "11",
    channel: str = fixtures.CHAT_CHANNEL,
    conversation: list[dict[str, Any]] | None = None,
    **context: Any,
) -> dict[str, Any]:
    return {
        "op": "generate",
        "author_id": author,
        "channel_id": channel,
        **context,
        "conversation": copy.deepcopy(conversation or CONVERSATION),
        "replies": replies,
    }


FULL_CAPS = {
    "structured_output": False,
    "sampling_controls": True,
    "reasoning_control": True,
    "function_tools": True,
    "reasoning_efforts": None,
}


def cases() -> list[dict[str, Any]]:
    def case(
        case_id: str, steps: list[dict[str, Any]], caps: dict | None = None, **settings: Any
    ) -> dict[str, Any]:
        base = fixtures.base_context()
        base["settings"].update(settings)
        return {"case_id": case_id, "input": {**base, "caps": caps or FULL_CAPS, "steps": steps}}

    move = call("propose_move", "c1", run_query="hstar", to_when="thu 22:00")
    return [
        case(
            "answers-in-words",
            [
                generate(
                    [says("  Nothing much!  ", usage={"prompt_tokens": 7, "completion_tokens": 3})]
                ),
                generate([says(None)]),
                generate([says("Here it is.\nA proposal card is ready for you.")]),
                generate([says(["not", "a", "string"], reasoning="thinking out loud")]),
            ],
        ),
        case(
            "read-then-grounded-answer",
            [
                generate(
                    [
                        wants(
                            call("get_schedule", "s1", week="this_boss"),
                            usage={"prompt_tokens": 100, "completion_tokens": 5},
                        ),
                        says(
                            "Ara~ this week:\n\nMaleficStar - 21:30 - run ID 'a1a1a1a1' (1/2)\n\n"
                            'Ask get_schedule(week="next") for more!',
                            usage={"prompt_tokens": "150", "completion_tokens": None},
                        ),
                    ],
                    force_all_channels=True,
                    force_group_schedule=True,
                )
            ],
        ),
        case(
            "round-cap-withholds-tools-on-the-last-round",
            [
                generate(
                    [
                        wants(call("list_bosses", "r1")),
                        wants(call("list_fixed", "r2")),
                        wants(call("get_pending", "r3")),
                        says("Four rounds in, answered in words."),
                    ]
                ),
                generate(
                    [
                        wants(call("list_bosses", "q1")),
                        wants(call("list_bosses", "q2")),
                        wants(call("list_bosses", "q3")),
                        wants(call("list_bosses", "q4"), content="still calling"),
                    ]
                ),
            ],
        ),
        case(
            "posted-write-reserves-the-confirmation-round",
            [
                generate(
                    [
                        wants(move, content="On it."),
                        says("Card's up for Thu 10 Sep 22:00 -- needs a ✅!"),
                    ],
                    channel=fixtures.PARTY_STAR,
                ),
            ],
        ),
        case(
            "refused-write-claims-are-overwritten",
            [
                generate(
                    [
                        wants(call("propose_cancel", "w1", run_query="hstar")),
                        says("Done, card's up ✅"),
                    ],
                    author="33",
                ),
                generate(
                    [
                        wants(call("propose_add", "w2", boss="limbo", when="sat 21:00")),
                        says("Normal or Hard Limbo?"),
                    ]
                ),
                generate(
                    [
                        wants(call("propose_add", "w3", boss="limbo", when="sat 21:00")),
                        says(""),
                    ]
                ),
            ],
        ),
        case(
            "call-ids-duplicates-and-malformed-calls",
            [
                generate(
                    [
                        wants(
                            call("list_fixed", "dup"),
                            call("get_pending", "dup"),
                            tool("get_run", '{"query": "hbaldrix"}'),
                            tool("get_run", "{not json", None),
                            {"type": "function"},
                            "not a call",
                        ),
                        wants(call("list_fixed", "dup"), tool("delete_run", {"run_query": "x"}, 7)),
                        says("Checked."),
                    ]
                ),
            ],
        ),
        case(
            "read-only-turn",
            [
                generate(
                    [
                        wants(call("propose_add", "ro1", boss="hlimbo", when="sat 21:00")),
                        says("Which night should it be?"),
                    ],
                    read_only=True,
                )
            ],
        ),
        case(
            "transport-failures",
            [
                generate([{"raise": "TimeoutError", "message": ""}]),
                generate([{"raise": "ModelUnavailable", "message": "gateway returned 503"}]),
                generate(
                    [wants(call("list_fixed", "t1")), {"raise": "RuntimeError", "message": "boom"}]
                ),
                generate([{"completion": "not a completion"}]),
            ],
        ),
        case(
            "model-without-function-tools",
            [generate([says("never reached")])],
            caps={**FULL_CAPS, "function_tools": False, "sampling_controls": False},
        ),
        case(
            "reasoning-and-sampling-controls",
            [generate([says("ok")])],
            caps={
                **FULL_CAPS,
                "sampling_controls": False,
                "reasoning_efforts": ["low", "high"],
            },
            chat_pilot_think="off",
        ),
        case(
            "missing-model-alias",
            [generate([says("never reached")])],
            chat_pilot_model="",
        ),
        case(
            "context-budget",
            [
                generate(
                    [says("fits after trimming")],
                    conversation=[
                        CONVERSATION[0],
                        {"role": "user", "content": "Alvin tan: " + "older question " * 1500},
                        {"role": "assistant", "content": "older answer " * 1500},
                        {"role": "user", "content": "Alvin tan: and now?"},
                    ],
                ),
                generate(
                    [says("never reached")],
                    conversation=[
                        CONVERSATION[0],
                        {"role": "user", "content": "Alvin tan: " + "huge " * 8000},
                    ],
                ),
            ],
            model_context_tokens=8192,
        ),
    ]


FAMILY = Family(
    name="loop",
    version="v5-chat-loop-v1",
    provenance={
        "oracle": "legacy/python bot.chat.agent.ChatPilot.generate",
        "functions": [
            "bot.chat.agent.ChatPilot.generate",
            "bot.chat.agent.ChatPilot._loop",
            "bot.chat.agent.ChatPilot._chat",
            "bot.chat.agent.ChatPilot._budgeted_messages",
            "bot.chat.agent.ChatPilot._finalize_write_reply",
            "bot.chat.agent.ChatPilot._finalize_read_claim",
            "bot.chat.agent.ChatPilot.voice_reminder",
            "bot.chat.agent.MAX_TOOL_ROUNDS",
            "bot.infrastructure.llm.wire.parse_reply",
            "bot.infrastructure.llm.wire.chat_body",
        ],
        "source_tests": [
            "tests/test_chat_agent.py",
            "tests/test_chat_tools.py",
            "tests/test_chat_gateway.py",
            "tests/test_chat_logging.py",
            "tests/test_chat_followup.py",
        ],
        "inventory_surfaces": [],
    },
    context={**fixtures.HOST_CONTEXT, "caps": ref("caps")},
    defs={**fixtures.HOST_DEFS, "caps": CAPS, "reply": REPLY, "message": MESSAGE},
    ops={
        "generate": Op(
            {
                **fixtures.TOOL_CONTEXT,
                "conversation": arr(ref("message"), 1),
                "replies": arr(ref("reply")),
            },
            GENERATION,
            optional=fixtures.TOOL_CONTEXT_OPTIONAL,
        )
    },
    cases=cases,
    replay=replay,
)
