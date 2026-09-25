"""Prompt message bytes, the structured-output schema bytes, and the request body."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any

from bot.extract import llm
from bot.extract import prompt as prompt_mod
from bot.extract.schema import json_schema
from bot.infrastructure.llm import Capabilities

from .. import fixtures
from ..contract import (
    BOOL,
    INT,
    NSTR,
    STR,
    STRS,
    Family,
    Op,
    arr,
    closed,
    nullable,
    ref,
    replay_steps,
)
from ..fixtures import RUN_A, RUN_B, RUN_C, msg, run

MESSAGE = closed({"role": STR, "content": STR})
MEMBER = closed({"user_id": STR, "display_name": STR, "nickname": NSTR})
FIXED = closed(
    {
        "id": STR,
        "bosses": STRS,
        "weekday": {"type": "integer", "minimum": 0, "maximum": 6},
        "time": STR,
        "participants": STRS,
        "channel_id": NSTR,
    }
)
CONTEXT = closed(
    {
        "channel_name": STR,
        "burst": arr(ref("msg")),
        "context": arr(ref("msg")),
        "runs": arr(ref("run")),
        "fixed_runs": arr(ref("fixed")),
        "roster": arr(ref("member")),
        "guild_runs": arr(ref("run")),
    }
)
CAPS = closed(
    {
        "structured_output": BOOL,
        "sampling_controls": BOOL,
        "reasoning_control": BOOL,
        "reasoning_efforts": nullable(STRS),
    }
)


def _msg(raw: dict[str, Any]) -> prompt_mod.Msg:
    return prompt_mod.Msg(
        id=raw["id"],
        author_id=raw["author_id"],
        author_name=raw["author_name"],
        created_at=datetime.fromisoformat(raw["created_at"]),
        content=raw["content"],
    )


def caps(raw: dict[str, Any]) -> Capabilities:
    efforts = raw["reasoning_efforts"]
    return Capabilities(
        structured_output=raw["structured_output"],
        sampling_controls=raw["sampling_controls"],
        reasoning_control=raw["reasoning_control"],
        reasoning_efforts=tuple(efforts) if efforts is not None else None,
    )


def schema_text() -> str:
    """The schema exactly as the JSON-only instruction serialises it."""
    return json.dumps(json_schema(), ensure_ascii=False, separators=(",", ":"))


def replay(case: dict[str, Any]) -> dict[str, Any]:
    input_ = case["input"]
    tz = fixtures.zone(input_["timezone"])
    table = fixtures.table(input_["catalog"])

    def context(raw: dict[str, Any]) -> prompt_mod.PromptContext:
        return prompt_mod.PromptContext(
            tz=tz,
            table=table,
            burst=[_msg(m) for m in raw["burst"]],
            context=[_msg(m) for m in raw["context"]],
            runs=[fixtures.run_in(r) for r in raw["runs"]],
            fixed_runs=raw["fixed_runs"],
            roster=raw["roster"],
            channel_name=raw["channel_name"],
            guild_runs=[fixtures.run_in(r) for r in raw["guild_runs"]],
        )

    def body(step: dict) -> dict[str, Any]:
        return llm.extraction_body(
            step["model"], step["messages"], step["reasoning_effort"], caps(step["caps"])
        )

    handlers = {
        "system_prompt": lambda s: prompt_mod.SYSTEM_PROMPT,
        "build_messages": lambda s: prompt_mod.build_messages(context(s["context"])),
        "relevant_roster": lambda s: [
            str(m["user_id"]) for m in prompt_mod.relevant_roster(context(s["context"]))
        ],
        "named_bosses": lambda s: sorted(prompt_mod.named_bosses(context(s["context"]))),
        "json_schema": lambda s: {"schema": json_schema(), "text": schema_text()},
        "json_instruction": lambda s: llm.json_instruction(),
        "extraction_body": body,
        "estimate_tokens": lambda s: prompt_mod.estimate_tokens(s["text"]),
        "estimate_messages": lambda s: prompt_mod.estimate_messages(s["messages"]),
        "prompt_budget": lambda s: prompt_mod.prompt_budget(s["num_ctx"]),
        "json_instruction_tokens": lambda s: llm.json_instruction_tokens(),
    }
    return replay_steps(FAMILY, case, handlers)


SUN = "2026-08-30T13:"
FIXED_ROW = {
    "id": "f6f6f6f6-0000-4000-8000-000000000006",
    "bosses": ["HLimbo"],
    "weekday": 1,
    "time": "22:30",
    "participants": ["11", "44"],
    "channel_id": "900",
}
PARTY = [
    run(RUN_A, ["HMaleficStar", "HFA"], "2026-08-31T21:30:00+08:00", ["11", "22"]),
    run(RUN_B, ["HCarling", "XKalos"], "2026-09-01T22:00:00+08:00", ["11", "22", "99"], "otot"),
]


def _ctx(**fields: Any) -> dict[str, Any]:
    out: dict[str, Any] = {
        "channel_name": "",
        "burst": [],
        "context": [],
        "runs": [],
        "fixed_runs": [],
        "roster": fixtures.ROSTER,
        "guild_runs": [],
    }
    out.update(fields)
    return out


PARTY_CTX = _ctx(
    channel_name="hstar-party",
    context=[msg("398", "33", "2026-08-30T12:40:00+08:00", "anyone free mon?")],
    burst=[
        msg("401", "11", SUN + "01:00+08:00", "mon cannot leh, can change to wed?"),
        msg("402", "22", SUN + "05:00+08:00", "okay for wed\n\n  <@44> u coming?  "),
        msg("403", "11", SUN + "07:00+08:00", "and we add our nstar tmr 930?"),
    ],
    runs=PARTY,
    fixed_runs=[FIXED_ROW],
)
GENERAL_CTX = _ctx(
    burst=[msg("501", "44", "2026-09-02T20:15:00+08:00", "can we do ours at 9 instead")],
    guild_runs=[
        run(RUN_C, ["NMaleficStar"], "2026-09-02T21:00:00+08:00", ["33", "44"], channel_id="901")
    ],
)
BANTER_CTX = _ctx(
    channel_name="lounge",
    burst=[msg("601", "99", "2026-09-02T20:15:00+08:00", "lol that ring price")],
)
EMPTY_ROSTER_CTX = _ctx(roster=[], burst=[msg("701", "99", "2026-09-02T20:15:00+08:00", "hi")])


def cases() -> list[dict[str, Any]]:
    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {
            "case_id": case_id,
            "input": {"timezone": fixtures.TIMEZONE, "catalog": fixtures.CATALOG, "steps": steps},
        }

    full = {"structured_output": True, "sampling_controls": True, "reasoning_control": True}
    small = [{"role": "system", "content": "sys"}, {"role": "user", "content": "[1] hi"}]
    return [
        case(
            "channel-prompt-bytes",
            [
                {"op": "system_prompt"},
                {"op": "build_messages", "context": PARTY_CTX},
                {"op": "relevant_roster", "context": PARTY_CTX},
                {"op": "named_bosses", "context": PARTY_CTX},
            ],
        ),
        case(
            "guild-fallback-and-full-table",
            [
                {"op": "build_messages", "context": GENERAL_CTX},
                {"op": "named_bosses", "context": GENERAL_CTX},
                {"op": "build_messages", "context": BANTER_CTX},
                {"op": "relevant_roster", "context": BANTER_CTX},
                {"op": "named_bosses", "context": BANTER_CTX},
                {"op": "build_messages", "context": EMPTY_ROSTER_CTX},
            ],
        ),
        case(
            "structured-output-schema-and-request-body",
            [
                {"op": "json_schema"},
                {"op": "json_instruction"},
                {
                    "op": "extraction_body",
                    "model": "extractor",
                    "messages": small,
                    "reasoning_effort": "low",
                    "caps": {**full, "reasoning_efforts": None},
                },
                {
                    "op": "extraction_body",
                    "model": "extractor",
                    "messages": small,
                    "reasoning_effort": "low",
                    "caps": {**full, "reasoning_efforts": ["medium", "high"]},
                },
                {
                    "op": "extraction_body",
                    "model": "extractor",
                    "messages": small,
                    "reasoning_effort": "low",
                    "caps": {
                        "structured_output": False,
                        "sampling_controls": False,
                        "reasoning_control": False,
                        "reasoning_efforts": None,
                    },
                },
                {
                    "op": "extraction_body",
                    "model": "extractor",
                    "messages": [{"role": "user", "content": "[1] hi"}],
                    "reasoning_effort": "",
                    "caps": {**full, "structured_output": False, "reasoning_efforts": None},
                },
            ],
        ),
        case(
            "token-budget-estimates",
            [
                {"op": "estimate_tokens", "text": ""},
                {"op": "estimate_tokens", "text": "hello world"},
                {"op": "estimate_tokens", "text": "[123456789012345678] <@1234> 99999 ok"},
                {"op": "estimate_messages", "messages": small},
                {"op": "prompt_budget", "num_ctx": 8192},
                {"op": "prompt_budget", "num_ctx": 3000},
                {"op": "json_instruction_tokens"},
            ],
        ),
    ]


FAMILY = Family(
    name="prompt",
    version="v5-extract-prompt-v1",
    provenance={
        "oracle": "legacy/python bot.extract.prompt, bot.extract.schema, bot.extract.llm",
        "functions": [
            "bot.extract.prompt.SYSTEM_PROMPT",
            "bot.extract.prompt.build_messages",
            "bot.extract.prompt.relevant_roster",
            "bot.extract.prompt.named_bosses",
            "bot.extract.prompt.estimate_tokens",
            "bot.extract.prompt.estimate_messages",
            "bot.extract.prompt.prompt_budget",
            "bot.extract.schema.json_schema",
            "bot.extract.llm.json_instruction",
            "bot.extract.llm.json_instruction_tokens",
            "bot.extract.llm.extraction_body",
        ],
        "source_tests": [
            "tests/test_extract_pipeline.py",
            "tests/test_extract_schema.py",
            "tests/test_extract_llm.py",
        ],
        "inventory_surfaces": [
            "extract.prompt-and-budget",
            "extract.schema",
            "extract.kanata-gateway-adapter",
        ],
    },
    context={"timezone": STR, "catalog": ref("catalog")},
    defs={
        "catalog": fixtures.CATALOG_DEF,
        "run": fixtures.RUN_DEF,
        "msg": fixtures.MSG_DEF,
        "member": MEMBER,
        "fixed": FIXED,
        "context": CONTEXT,
        "message": MESSAGE,
        "caps": CAPS,
    },
    ops={
        "system_prompt": Op({}, STR),
        "build_messages": Op({"context": ref("context")}, arr(ref("message"))),
        "relevant_roster": Op({"context": ref("context")}, STRS),
        "named_bosses": Op({"context": ref("context")}, STRS),
        "json_schema": Op({}, closed({"schema": {"type": "object"}, "text": STR})),
        "json_instruction": Op({}, STR),
        "extraction_body": Op(
            {
                "model": STR,
                "messages": arr(ref("message")),
                "reasoning_effort": STR,
                "caps": ref("caps"),
            },
            {"type": "object", "required": ["model", "messages"]},
        ),
        "estimate_tokens": Op({"text": STR}, INT),
        "estimate_messages": Op({"messages": arr(ref("message"))}, INT),
        "prompt_budget": Op({"num_ctx": INT}, INT),
        "json_instruction_tokens": Op({}, INT),
    },
    cases=cases,
    replay=replay,
)
