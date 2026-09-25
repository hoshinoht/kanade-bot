"""Model-response acceptance: ``parse_response`` and the guarded ``Extractor.extract`` call.

A rejected response never raises at the caller: ``Extractor.extract`` retries
once with the schema error, then returns an ``ExtractionCall`` with ``error``
set and no amendments -- the quarantine the pipeline logs instead of planning.
"""

from __future__ import annotations

import asyncio
import copy
from types import SimpleNamespace
from typing import Any

from pydantic import ValidationError

from bot.extract import llm
from bot.infrastructure.llm import ModelUnavailable

from ...scheduler.oracle import quiet_logs
from .. import fixtures
from ..contract import (
    BOOL,
    INT,
    NSTR,
    NUM,
    STR,
    ContractError,
    Family,
    Op,
    arr,
    closed,
    nullable,
    ref,
    replay_steps,
)
from .prompt import CAPS, MESSAGE, caps

RAISES: dict[str, type[Exception]] = {
    "TimeoutError": TimeoutError,
    "ModelUnavailable": ModelUnavailable,
    "RuntimeError": RuntimeError,
}
LOC = arr({"type": ["string", "integer"]})
PARSED = {
    "oneOf": [
        closed({"status": {"const": "accepted"}, "extraction": ref("extraction")}),
        closed(
            {
                "status": {"const": "rejected"},
                "error_class": {"const": "ValueError"},
                "message": STR,
            }
        ),
        closed(
            {
                "status": {"const": "rejected"},
                "error_class": {"const": "ValidationError"},
                "errors": arr(closed({"loc": LOC, "type": STR}), 1),
            }
        ),
    ]
}
REPLY = {
    "oneOf": [
        closed({"content": NSTR, "reasoning": NSTR}),
        closed({"raise": {"enum": list(RAISES)}, "message": STR}),
    ]
}
CALL = closed(
    {
        "ok": BOOL,
        "attempts": INT,
        "error": NSTR,
        "raw": STR,
        "thinking": STR,
        "misconfigured": BOOL,
        "prompt": STR,
        "extraction": nullable(ref("extraction")),
        "requests": arr({"type": "object"}),
    }
)


def parsed(raw: str) -> dict[str, Any]:
    try:
        extraction = llm.parse_response(raw)
    except ValidationError as exc:
        errors = exc.errors(include_url=False, include_input=False, include_context=False)
        return {
            "status": "rejected",
            "error_class": "ValidationError",
            "errors": [{"loc": list(e["loc"]), "type": e["type"]} for e in errors],
        }
    except ValueError as exc:
        return {"status": "rejected", "error_class": "ValueError", "message": str(exc)}
    return {"status": "accepted", "extraction": extraction.model_dump(mode="json")}


class ScriptedClient:
    """A gateway stand-in: fixed capabilities and scripted completions, in order."""

    def __init__(self, case_id: str, capabilities: Any, replies: list[dict[str, Any]]):
        self.case_id = case_id
        self.capabilities = capabilities
        self.replies = list(replies)
        self.requests: list[dict[str, Any]] = []

    async def profile(self, alias: str) -> Any:
        return self.capabilities

    async def chat(self, **body: Any) -> dict[str, Any]:
        self.requests.append(copy.deepcopy(body))
        if not self.replies:
            raise ContractError(f"{self.case_id}: the scripted replies ran out")
        reply = self.replies.pop(0)
        if "raise" in reply:
            raise RAISES[reply["raise"]](reply["message"])
        message = {"role": "assistant", "content": reply["content"]}
        if reply["reasoning"] is not None:
            message["reasoning"] = reply["reasoning"]
        return {"choices": [{"message": message}]}


def replay(case: dict[str, Any]) -> dict[str, Any]:
    def call(step: dict) -> dict[str, Any]:
        client = ScriptedClient(case["case_id"], caps(step["caps"]), step["replies"])
        settings = SimpleNamespace(
            extract_model=step["extract_model"],
            reasoning_effort=step["reasoning_effort"],
            kanata_timeout=step["kanata_timeout"],
        )
        with quiet_logs():
            result = asyncio.run(llm.Extractor(settings, client).extract(step["messages"]))
        if client.replies:
            raise ContractError(f"{case['case_id']}: {len(client.replies)} scripted replies unused")
        return {
            "ok": result.ok,
            "attempts": result.attempts,
            "error": result.error,
            "raw": result.raw,
            "thinking": result.thinking,
            "misconfigured": result.misconfigured,
            "prompt": result.prompt,
            "extraction": result.extraction.model_dump(mode="json") if result.ok else None,
            "requests": client.requests,
        }

    return replay_steps(
        FAMILY, case, {"parse_response": lambda s: parsed(s["raw"]), "extract_call": call}
    )


def _p(raw: str) -> dict[str, Any]:
    return {"op": "parse_response", "raw": raw}


GOOD = (
    '{"amendments":[{"kind":"move","bosses":["HMaleficStar"],"day_ref":"wed","time_ref":null,'
    '"participants":["11"],"rsvp":null,"is_question":true,"confidence":0.8,'
    '"evidence_message_ids":["1"],"target_run_hint":"#a1a1"}],"summary":"move to wed"}'
)
MESSAGES = [
    {"role": "system", "content": "SYSTEM"},
    {"role": "user", "content": "[1] [2026-08-30 13:01 Sun] [Alvin tan <@11>] wed?"},
]


def _call(replies: list[dict[str, Any]], **overrides: Any) -> dict[str, Any]:
    step: dict[str, Any] = {
        "op": "extract_call",
        "extract_model": "extractor",
        "reasoning_effort": "low",
        "kanata_timeout": 30.0,
        "caps": {
            "structured_output": True,
            "sampling_controls": True,
            "reasoning_control": False,
            "reasoning_efforts": None,
        },
        "messages": MESSAGES,
        "replies": replies,
    }
    step.update(overrides)
    return step


def _reply(content: str | None, reasoning: str | None = None) -> dict[str, Any]:
    return {"content": content, "reasoning": reasoning}


def cases() -> list[dict[str, Any]]:
    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {"case_id": case_id, "input": {"steps": steps}}

    return [
        case(
            "accepted-and-coerced",
            [
                _p(GOOD),
                _p("```json\n" + GOOD + "\n```"),
                _p("```\n" + GOOD + "```"),
                _p('{"amendments": [], "summary": "no schedule change"}'),
                _p("{}"),
                _p('{"amendments": null, "summary": null}'),
                _p(
                    '{"amendments": {"kind": " MOVE ", "bosses": "HMaleficStar; HFA, ,null",'
                    ' "participants": ["<@11>", "<@!22>", "11"], "day_ref": "null",'
                    ' "time_ref": "  9pm ", "rsvp": "N/A", "is_question": "yes",'
                    ' "confidence": 82, "evidence_message_ids": 7, "target_run_hint": "TBD",'
                    ' "extra": 1}, "summary": "-", "note": "ignored"}'
                ),
                _p(
                    '{"amendments": [{"kind": "rsvp", "rsvp": "YES", "confidence": "high"},'
                    ' {"kind": "otot", "confidence": -3, "is_question": "0"}]}'
                ),
            ],
        ),
        case(
            "rejected-responses",
            [
                _p(""),
                _p("   "),
                _p("Sure! Here is the JSON:\n" + GOOD),
                _p('{"amendments": [{"kind": "move"'),
                _p("[]"),
                _p('"just a string"'),
                _p("```json\n{}\n``` trailing"),
                _p('{"amendments": [{"kind": "reschedule"}]}'),
                _p('{"amendments": [{"kind": "rsvp", "rsvp": "perhaps"}]}'),
                _p('{"amendments": [{"kind": "move", "is_question": "maybe", "bosses": {}}]}'),
                _p('{"amendments": "move"}'),
                _p('{"amendments": [{}]}'),
            ],
        ),
        case(
            "guarded-call-accepts-first-answer",
            [
                _call([_reply(GOOD, "thinking about it")]),
                _call(
                    [_reply(GOOD)],
                    caps={
                        "structured_output": False,
                        "sampling_controls": False,
                        "reasoning_control": True,
                        "reasoning_efforts": None,
                    },
                ),
            ],
        ),
        case(
            "guarded-call-retries-once-then-quarantines",
            [
                _call([_reply("not json"), _reply(GOOD)]),
                _call([_reply(""), _reply('{"amendments": [{"kind": "reschedule"}]}')]),
                _call([_reply(None), _reply("[]")]),
            ],
        ),
        case(
            "guarded-call-transport-failures",
            [
                _call([{"raise": "TimeoutError", "message": ""}]),
                _call([{"raise": "ModelUnavailable", "message": "gateway returned 503"}]),
                _call([{"raise": "RuntimeError", "message": "boom"}]),
                _call([_reply("nope"), {"raise": "TimeoutError", "message": ""}]),
                _call([], extract_model=""),
            ],
        ),
    ]


FAMILY = Family(
    name="parse",
    version="v5-extract-parse-v1",
    provenance={
        "oracle": "legacy/python bot.extract.llm, bot.extract.schema",
        "functions": [
            "bot.extract.llm.parse_response",
            "bot.extract.llm.Extractor.extract",
            "bot.extract.llm.RETRY_INSTRUCTION",
            "bot.extract.schema.Extraction",
            "bot.extract.schema.Amendment",
        ],
        "source_tests": ["tests/test_extract_llm.py", "tests/test_extract_schema.py"],
        "inventory_surfaces": ["extract.schema", "extract.kanata-gateway-adapter"],
    },
    context={},
    defs={
        "amendment": fixtures.AMENDMENT_DEF,
        "extraction": fixtures.EXTRACTION_DEF,
        "message": MESSAGE,
        "caps": CAPS,
        "reply": REPLY,
    },
    ops={
        "parse_response": Op({"raw": STR}, PARSED),
        "extract_call": Op(
            {
                "extract_model": STR,
                "reasoning_effort": STR,
                "kanata_timeout": NUM,
                "caps": ref("caps"),
                "messages": arr(ref("message")),
                "replies": arr(ref("reply")),
            },
            CALL,
        ),
    },
    cases=cases,
    replay=replay,
)
