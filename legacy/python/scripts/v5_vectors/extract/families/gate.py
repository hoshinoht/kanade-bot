"""Keyword gate: ``find_*`` scanners, ``evaluate``, ``should_extract`` and ``urgent``."""

from __future__ import annotations

from typing import Any

from bot.extract import gate
from bot.extract.pipeline import urgent

from .. import fixtures
from ..contract import BOOL, NSTR, STR, STRS, Family, Op, arr, closed, ref, replay_steps

ROSTER_IDS = [m["user_id"] for m in fixtures.ROSTER]

HIT = closed({"token": STR, "short": STR, "difficulty": NSTR, "canonical": NSTR, "fuzzy": BOOL})
RESULT = closed(
    {
        "signals": STRS,
        "bosses": arr(ref("boss_hit")),
        "times": STRS,
        "days": STRS,
        "mentions": STRS,
        "hit": BOOL,
        "strong": BOOL,
        "reasons": STR,
    }
)
TEXT = {"text": STR}


def _hit(hit: gate.BossHit) -> dict[str, Any]:
    return {
        "token": hit.token,
        "short": hit.short,
        "difficulty": hit.difficulty,
        "canonical": hit.canonical,
        "fuzzy": hit.fuzzy,
    }


def _result(result: gate.GateResult) -> dict[str, Any]:
    return {
        "signals": sorted(result.signals),
        "bosses": [_hit(hit) for hit in result.bosses],
        "times": list(result.times),
        "days": list(result.days),
        "mentions": list(result.mentions),
        "hit": result.hit,
        "strong": result.strong,
        "reasons": result.reasons,
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    input_ = case["input"]
    table = fixtures.table(input_["catalog"])
    roster = input_["roster_ids"]

    def evaluate(text: str) -> gate.GateResult:
        return gate.evaluate(text, table, roster)

    handlers = {
        "find_bosses": lambda s: [_hit(h) for h in gate.find_bosses(s["text"], table)],
        "canonical_bosses": lambda s: gate.canonical_bosses(gate.find_bosses(s["text"], table)),
        "find_times": lambda s: gate.find_times(s["text"]),
        "find_days": lambda s: gate.find_days(s["text"]),
        "find_mentions": lambda s: gate.find_mentions(s["text"], s["roster_ids"]),
        "explicit_rsvp": lambda s: gate.explicit_rsvp(s["text"]),
        "evaluate": lambda s: _result(evaluate(s["text"])),
        "should_extract": lambda s: gate.should_extract(
            [evaluate(text) for text in s["texts"]], s["context_is_scheduling"]
        ),
        "urgent": lambda s: urgent(evaluate(s["text"])),
    }
    return replay_steps(FAMILY, case, handlers)


def _case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
    return {
        "case_id": case_id,
        "input": {"catalog": fixtures.CATALOG, "roster_ids": ROSTER_IDS, "steps": steps},
    }


def _each(op: str, texts: list[str]) -> list[dict[str, Any]]:
    return [{"op": op, "text": text} for text in texts]


def cases() -> list[dict[str, Any]]:
    return [
        _case(
            "boss-tokens-prefixes-words-and-fuzzy",
            _each(
                "find_bosses",
                [
                    "we doing our nstar and ncarl tonight?",
                    "hstarr later",
                    "nbaldrx 930",
                    "exkalos and hardstar",
                    "limbo or carling",
                    "hkalos anyone",
                    "start the run, clear chair",
                    "HMaleficStar + HFA",
                    "xkalos xkalos XKALOS",
                    "ch7 hstar",
                    "fa star bald",
                    "normbaldguy extremefa",
                    "",
                ],
            )
            + _each("canonical_bosses", ["nstar hstar limbo hkalos nstar", "chaoskalos easykalos"]),
        ),
        _case(
            "clock-expressions-and-masks",
            _each(
                "find_times",
                [
                    "9pm",
                    "9:30pm then 9.30 pm",
                    "930 ok",
                    "1030~11+pm",
                    "8~1130",
                    "9-10pm",
                    "at 11",
                    "lvl 290 now",
                    "since 2026",
                    "ring $200 each",
                    "cc9 later, ch7, c 3",
                    "call 91234567",
                    "<@123456789012345678> 930",
                    "see https://example.invalid/930 pls",
                    "12am or 9+pm",
                    "21:30 sharp",
                    "1130pm",
                    "10pm-11pm",
                    "<:pepe:123456789012345678> 945",
                ],
            ),
        ),
        _case(
            "day-words-and-mentions",
            _each(
                "find_days",
                ["weds or thurs", "tmr tonight", "Mon and tuesday", "today ytd today", "cc6 later"],
            )
            + [
                {
                    "op": "find_mentions",
                    "text": "<@11> <@!22> <@99> <@11>",
                    "roster_ids": ROSTER_IDS,
                },
                {"op": "find_mentions", "text": "<@11> <@99>", "roster_ids": []},
            ],
        ),
        _case(
            "explicit-rsvp-answers",
            _each(
                "explicit_rsvp",
                [
                    "Can",
                    "ok",
                    "kenot sry",
                    "can?",
                    "i can take",
                    "can anot",
                    "okay for wed",
                    "yes no",
                    "ok ok ok ok ok ok ok ok ok",
                    "I ok",
                    "not free",
                    "can shift",
                    "",
                ],
            ),
        ),
        _case(
            "evaluate-signals",
            _each(
                "evaluate",
                [
                    "lol that ring price is crazy",
                    "ok",
                    "@here 9:30 later tonight",
                    "we lock in tue night as default?",
                    "run later",
                    "<@11> hstar tmr",
                    "<@99> hi",
                    "cc9 pls",
                ],
            ),
        ),
        _case(
            "burst-worth-a-call-and-urgency",
            [
                {"op": "should_extract", "texts": ["ok", "Can"], "context_is_scheduling": False},
                {"op": "should_extract", "texts": ["ok", "Can"], "context_is_scheduling": True},
                {"op": "should_extract", "texts": ["lol", "hi"], "context_is_scheduling": True},
                {"op": "should_extract", "texts": ["ok", "hstar?"], "context_is_scheduling": False},
                {"op": "should_extract", "texts": ["later"], "context_is_scheduling": False},
            ]
            + _each(
                "urgent",
                [
                    "@here 9:30 later tonight",
                    "<@22> hstar",
                    "@everyone hi",
                    "hstar 930",
                    "<@22> ok",
                ],
            ),
        ),
    ]


FAMILY = Family(
    name="gate",
    version="v5-extract-gate-v1",
    provenance={
        "oracle": "legacy/python bot.extract.gate, bot.extract.pipeline.urgent",
        "functions": [
            "bot.extract.gate.find_bosses",
            "bot.extract.gate.canonical_bosses",
            "bot.extract.gate.find_times",
            "bot.extract.gate.find_days",
            "bot.extract.gate.find_mentions",
            "bot.extract.gate.explicit_rsvp",
            "bot.extract.gate.evaluate",
            "bot.extract.gate.should_extract",
            "bot.extract.pipeline.urgent",
        ],
        "source_tests": ["tests/test_extract_gate.py", "tests/test_extract_pipeline.py"],
        "inventory_surfaces": ["extract.gate", "extract.pipeline-and-backfill"],
    },
    context={"catalog": ref("catalog"), "roster_ids": STRS},
    defs={"catalog": fixtures.CATALOG_DEF, "boss_hit": HIT},
    ops={
        "find_bosses": Op(TEXT, arr(ref("boss_hit"))),
        "canonical_bosses": Op(TEXT, STRS),
        "find_times": Op(TEXT, STRS),
        "find_days": Op(TEXT, STRS),
        "find_mentions": Op({**TEXT, "roster_ids": STRS}, STRS),
        "explicit_rsvp": Op(TEXT, {"enum": ["yes", "no", None]}),
        "evaluate": Op(TEXT, RESULT),
        "should_extract": Op({"texts": STRS, "context_is_scheduling": BOOL}, BOOL),
        "urgent": Op(TEXT, BOOL),
    },
    cases=cases,
    replay=replay,
)
