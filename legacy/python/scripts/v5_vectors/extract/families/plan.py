"""Burst planning from a scripted model response: ``plan_burst`` and ``consolidate``.

Each step carries the raw model text; it goes through the real
``parse_response`` and then ``plan_burst`` with every argument the live
pipeline passes (anchor, now, runs, burst order, authors, confidence floor,
boss table, burst messages), so nothing is defaulted from the wall clock.
"""

from __future__ import annotations

import json
from datetime import datetime
from types import SimpleNamespace
from typing import Any

from bot.extract import llm
from bot.extract.pipeline import Plan, Planned, consolidate, plan_burst

from .. import fixtures
from ..contract import (
    BOOL,
    INSTANT,
    NUM,
    STR,
    STRS,
    ContractError,
    Family,
    Op,
    arr,
    closed,
    ref,
    replay_steps,
)
from ..fixtures import RUN_A, RUN_B, RUN_D, WEEK2, amendment, run

BURST_MESSAGE = closed({"id": STR, "author_id": STR, "content": STR})
BURST = {
    "raw": STR,
    "anchor": INSTANT,
    "now": INSTANT,
    "min_confidence": NUM,
    "channel_runs": STRS,
    "guild_runs": STRS,
    "burst_order": STRS,
    "author_ids": {"type": "object", "additionalProperties": STR},
    "burst_messages": arr(ref("burst_message")),
    "use_boss_table": BOOL,
}
PLAN = closed({"planned": arr(ref("planned")), "dropped": arr(ref("planned")), "summary": STR})


def dump_planned(entry: Planned) -> dict[str, Any]:
    return {
        "kind": entry.kind,
        "amendment": fixtures.dump_amendment(entry.amendment),
        "resolved": fixtures.dump_resolved(entry.resolved),
        "run_id": entry.run["id"] if entry.run else None,
        "payload": entry.payload,
        "match_reason": entry.match_reason,
        "match_code": entry.match_code,
        "also_mentioned": list(entry.also_mentioned),
        "ambiguous": entry.ambiguous,
        "summary": entry.summary,
        "needs_answer": entry.needs_answer,
    }


def dump_plan(plan: Plan) -> dict[str, Any]:
    return {
        "planned": [dump_planned(e) for e in plan.planned],
        "dropped": [dump_planned(e) for e in plan.dropped],
        "summary": plan.summary,
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    input_ = case["input"]
    tz = fixtures.zone(input_["timezone"])
    table = fixtures.table(input_["catalog"])
    pool = {raw["id"]: fixtures.run_in(raw) for raw in input_["runs"]}

    def runs(ids: list[str]) -> list[dict[str, Any]]:
        try:
            return [pool[run_id] for run_id in ids]
        except KeyError as exc:
            raise ContractError(f"{case['case_id']}: undeclared run ID {exc}") from None

    def planned(step: dict) -> Plan:
        try:
            extraction = llm.parse_response(step["raw"])
        except ValueError as exc:
            raise ContractError(f"{case['case_id']}: scripted response rejected: {exc}") from None
        return plan_burst(
            extraction,
            anchor=datetime.fromisoformat(step["anchor"]),
            tz=tz,
            channel_runs=runs(step["channel_runs"]),
            guild_runs=runs(step["guild_runs"]),
            burst_order=step["burst_order"],
            author_ids=step["author_ids"],
            min_confidence=step["min_confidence"],
            now=datetime.fromisoformat(step["now"]),
            boss_table=table if step["use_boss_table"] else None,
            burst_messages=[SimpleNamespace(**m) for m in step["burst_messages"]],
        )

    def consolidated(step: dict) -> dict[str, Any]:
        plans = [planned(burst) for burst in step["bursts"]]
        entries = consolidate([entry for plan in plans for entry in plan.planned])
        return {
            "plans": [dump_plan(plan) for plan in plans],
            "consolidated": [dump_planned(entry) for entry in entries],
        }

    handlers = {
        "plan_burst": lambda s: dump_plan(planned(s)),
        "consolidate": consolidated,
    }
    return replay_steps(FAMILY, case, handlers)


SUNDAY = "2026-08-30T13:07:00+08:00"
PARTY = [
    run(RUN_A, ["HMaleficStar", "HFA"], "2026-08-31T21:30:00+08:00", ["11", "22", "33"]),
    run(RUN_B, ["HCarling", "XKalos"], "2026-09-01T22:00:00+08:00", ["11", "22", "44"]),
    run(
        RUN_D,
        ["HMaleficStar", "HFA"],
        "2026-09-07T21:30:00+08:00",
        ["11", "22", "33"],
        week_start=WEEK2,
    ),
]
HOME = [RUN_A, RUN_B]


def raw(*amendments: dict[str, Any], summary: str = "a burst") -> str:
    return json.dumps({"amendments": list(amendments), "summary": summary}, ensure_ascii=False)


def burst(
    response: str,
    messages: list[tuple[str, str, str]],
    *,
    channel: list[str] = HOME,
    guild: list[str] = (),
    min_confidence: float = 0.0,
    anchor: str = SUNDAY,
    now: str = SUNDAY,
    context: list[tuple[str, str]] = (),
    boss_table: bool = True,
) -> dict[str, Any]:
    """One burst's plan inputs, derived the way ``Pipeline._extract`` derives them."""
    order = [mid for mid, _ in context] + [mid for mid, _, _ in messages]
    authors = {mid: uid for mid, uid in context} | {mid: uid for mid, uid, _ in messages}
    return {
        "raw": response,
        "anchor": anchor,
        "now": now,
        "min_confidence": min_confidence,
        "channel_runs": list(channel),
        "guild_runs": list(guild),
        "burst_order": order,
        "author_ids": authors,
        "burst_messages": [{"id": m, "author_id": u, "content": c} for m, u, c in messages],
        "use_boss_table": boss_table,
    }


def step(**kwargs: Any) -> dict[str, Any]:
    return {"op": "plan_burst", **burst(**kwargs)}


def cases() -> list[dict[str, Any]]:
    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {
            "case_id": case_id,
            "input": {
                "timezone": fixtures.TIMEZONE,
                "catalog": fixtures.CATALOG,
                "runs": PARTY,
                "steps": steps,
            },
        }

    worked = [
        ("1", "11", "mon cannot leh, can change to wed?"),
        ("2", "22", "okay for wed"),
        ("3", "11", "and we add our nstar tmr 930?"),
    ]
    return [
        case(
            "worked-example-question-rsvp-and-add",
            [
                step(
                    response=raw(
                        amendment(
                            "move",
                            bosses=["HMaleficStar", "HFA"],
                            day_ref="wed",
                            participants=["11"],
                            is_question=True,
                            confidence=0.8,
                            evidence_message_ids=["1"],
                            target_run_hint="#a1a1",
                        ),
                        amendment(
                            "rsvp",
                            day_ref="wed",
                            participants=["22"],
                            rsvp="yes",
                            evidence_message_ids=["2"],
                            target_run_hint="#a1a1",
                        ),
                        amendment(
                            "add",
                            bosses=["NMaleficStar"],
                            day_ref="tmr",
                            time_ref="930",
                            participants=["11"],
                            is_question=True,
                            confidence=0.8,
                            evidence_message_ids=["3"],
                        ),
                        summary="HMaleficStar+HFA proposed for Wed; NMaleficStar tomorrow 930",
                    ),
                    messages=worked,
                ),
            ],
        ),
        case(
            "normalised-bosses-and-injected-rsvps",
            [
                step(
                    response=raw(
                        amendment(
                            "move",
                            bosses=["hstar", "HFA", "HMaleficStar", "hkalos"],
                            day_ref="wed",
                            time_ref="10pm",
                            evidence_message_ids=["1"],
                        ),
                        amendment(
                            "rsvp", participants=["33"], rsvp="yes", evidence_message_ids=["3"]
                        ),
                    ),
                    messages=[
                        ("1", "11", "hstar wed 10pm"),
                        ("2", "22", "Can"),
                        ("3", "33", "ok"),
                        ("4", "44", "tue kenot sry"),
                        ("5", "22", "can?"),
                    ],
                ),
                step(
                    response=raw(
                        amendment(
                            "move",
                            bosses=["hstar"],
                            day_ref="wed",
                            time_ref="10pm",
                            evidence_message_ids=["1"],
                        ),
                    ),
                    messages=[("1", "11", "hstar wed 10pm")],
                    boss_table=False,
                ),
            ],
        ),
        case(
            "drop-reasons",
            [
                step(
                    response=raw(
                        amendment(
                            "move",
                            bosses=["HCarling"],
                            day_ref="wed",
                            time_ref="9pm",
                            confidence=0.4,
                            evidence_message_ids=["1"],
                        ),
                        amendment("cancel", bosses=["NBaldrix"], evidence_message_ids=["1"]),
                        amendment("add", bosses=["HLimbo"], evidence_message_ids=["2"]),
                        amendment(
                            "add", bosses=["NLimbo"], is_question=True, evidence_message_ids=["3"]
                        ),
                        amendment(
                            "move",
                            bosses=["HMaleficStar"],
                            day_ref="ytd",
                            time_ref="9pm",
                            evidence_message_ids=["2"],
                        ),
                        amendment(
                            "move",
                            bosses=["XKalos"],
                            day_ref="tue",
                            time_ref="10pm",
                            evidence_message_ids=["3"],
                        ),
                        amendment(
                            "add",
                            bosses=["HFA"],
                            day_ref="mon",
                            time_ref="9:30pm",
                            evidence_message_ids=["3"],
                        ),
                    ),
                    messages=[("1", "11", "x"), ("2", "22", "y"), ("3", "33", "z")],
                    min_confidence=0.5,
                ),
            ],
        ),
        case(
            "ambiguous-matches",
            [
                step(
                    response=raw(
                        amendment("move", day_ref="thu", evidence_message_ids=["1"]),
                        amendment(
                            "sub", participants=["11"], is_question=True, evidence_message_ids=["1"]
                        ),
                        amendment(
                            "rsvp", participants=["11"], rsvp="no", evidence_message_ids=["2"]
                        ),
                    ),
                    messages=[
                        ("1", "11", "can change to thu? find temp for me"),
                        ("2", "11", "idk"),
                    ],
                ),
                step(
                    response=raw(amendment("move", day_ref="thu", evidence_message_ids=["1"])),
                    messages=[("1", "99", "can change to thu?")],
                ),
            ],
        ),
        case(
            "no-run-here-becomes-an-add",
            [
                step(
                    response=raw(
                        amendment(
                            "move",
                            bosses=["NBaldrix"],
                            day_ref="wed",
                            time_ref="9pm",
                            evidence_message_ids=["1"],
                        ),
                        amendment(
                            "split",
                            bosses=["NLimbo"],
                            day_ref="thu",
                            time_ref="10pm",
                            evidence_message_ids=["1"],
                        ),
                        amendment(
                            "move", bosses=["HLimbo"], time_ref="9pm", evidence_message_ids=["2"]
                        ),
                    ),
                    messages=[("1", "11", "nbald wed 9pm, nlimbo thu"), ("2", "11", "hlimbo 9pm")],
                ),
                step(
                    response=raw(
                        amendment(
                            "add",
                            bosses=["NBaldrix"],
                            day_ref="wed",
                            is_question=True,
                            evidence_message_ids=["1"],
                        ),
                        amendment(
                            "move",
                            bosses=["NBaldrix"],
                            day_ref="thu",
                            time_ref="9pm",
                            evidence_message_ids=["2"],
                        ),
                    ),
                    messages=[("1", "11", "nbald wed?"), ("2", "22", "no, thu 9pm")],
                    channel=[],
                ),
            ],
        ),
        case(
            "spans-runs-payloads-and-inheritance",
            [
                step(
                    response=raw(
                        amendment(
                            "sub",
                            bosses=["HMaleficStar", "HCarling"],
                            participants=["11"],
                            is_question=True,
                            evidence_message_ids=["1", "2"],
                        ),
                        amendment("cancel", bosses=["HFA", "XKalos"], evidence_message_ids=["3"]),
                    ),
                    messages=[
                        ("1", "11", "mon and tue got stuff, find temp for me?"),
                        ("2", "44", "I can take"),
                        ("3", "22", "or we skip both"),
                    ],
                ),
                step(
                    response=raw(
                        amendment(
                            "split",
                            bosses=["XKalos"],
                            participants=["22"],
                            evidence_message_ids=["1"],
                        ),
                        amendment(
                            "fix",
                            bosses=["HLimbo", "NBaldrix"],
                            day_ref="tue",
                            time_ref="1030pm onwards",
                            evidence_message_ids=["2"],
                        ),
                        amendment(
                            "fix", bosses=["NKalos"], day_ref="fri", evidence_message_ids=["2"]
                        ),
                        amendment(
                            "move",
                            bosses=["HMaleficStar"],
                            time_ref="10pm",
                            evidence_message_ids=["3"],
                        ),
                    ),
                    messages=[
                        ("1", "22", "i duo xkalos"),
                        ("2", "11", "hlimbo nbald lock in tue 1030pm onwards"),
                        ("3", "33", "hstar amend to 10pm"),
                    ],
                ),
            ],
        ),
        case(
            "one-change-per-run-and-later-weeks",
            [
                step(
                    response=raw(
                        amendment(
                            "move",
                            bosses=["HCarling"],
                            day_ref="wed",
                            is_question=True,
                            evidence_message_ids=["1"],
                        ),
                        amendment("otot", bosses=["HCarling"], evidence_message_ids=["2"]),
                        amendment(
                            "cancel",
                            bosses=["HMaleficStar"],
                            is_question=True,
                            evidence_message_ids=["1"],
                        ),
                        amendment(
                            "move",
                            bosses=["HMaleficStar"],
                            day_ref="wed",
                            time_ref="9pm",
                            evidence_message_ids=["2"],
                        ),
                    ),
                    messages=[("1", "11", "hcarl wed? hstar off?"), ("2", "22", "we otot hcarl")],
                ),
                step(
                    response=raw(
                        amendment(
                            "move",
                            bosses=["HMaleficStar"],
                            day_ref="wed",
                            time_ref="9pm",
                            evidence_message_ids=["1"],
                        ),
                    ),
                    messages=[("1", "11", "hstar wed 9pm")],
                    channel=[RUN_D],
                ),
                step(
                    response=raw(
                        amendment(
                            "move",
                            bosses=["HMaleficStar"],
                            day_ref="next mon",
                            time_ref="10pm",
                            evidence_message_ids=["1"],
                        ),
                    ),
                    messages=[("1", "11", "hstar next mon 10pm")],
                    channel=[RUN_A, RUN_D],
                ),
            ],
        ),
        case(
            "rescan-consolidates-to-one-entry-per-target",
            [
                {
                    "op": "consolidate",
                    "bursts": [
                        burst(
                            raw(
                                amendment(
                                    "move",
                                    bosses=["HMaleficStar"],
                                    day_ref="wed",
                                    is_question=True,
                                    evidence_message_ids=["1"],
                                ),
                                amendment(
                                    "add",
                                    bosses=["NLimbo"],
                                    day_ref="fri",
                                    time_ref="9pm",
                                    is_question=True,
                                    evidence_message_ids=["1"],
                                ),
                                summary="sunday",
                            ),
                            [("1", "11", "hstar wed? nlimbo fri 9pm?")],
                        ),
                        burst(
                            raw(
                                amendment(
                                    "move",
                                    bosses=["HMaleficStar"],
                                    day_ref="wed",
                                    time_ref="10pm",
                                    evidence_message_ids=["2"],
                                ),
                                amendment(
                                    "otot", bosses=["HMaleficStar"], evidence_message_ids=["3"]
                                ),
                                amendment(
                                    "add",
                                    bosses=["NLimbo"],
                                    day_ref="fri",
                                    time_ref="10pm",
                                    evidence_message_ids=["2"],
                                ),
                                summary="monday",
                            ),
                            [("2", "22", "wed 10pm then, nlimbo fri 10"), ("3", "33", "or otot")],
                            anchor="2026-08-31T10:00:00+08:00",
                            now="2026-08-31T10:00:00+08:00",
                        ),
                    ],
                },
            ],
        ),
    ]


FAMILY = Family(
    name="plan",
    version="v5-extract-plan-v1",
    provenance={
        "oracle": "legacy/python bot.extract.pipeline",
        "functions": [
            "bot.extract.llm.parse_response",
            "bot.extract.pipeline.plan_burst",
            "bot.extract.pipeline.one_per_run",
            "bot.extract.pipeline.inherit_from_run",
            "bot.extract.pipeline.already_passed",
            "bot.extract.pipeline.is_no_op",
            "bot.extract.pipeline.volunteers_for",
            "bot.extract.pipeline.consolidate",
        ],
        "source_tests": ["tests/test_extract_pipeline.py"],
        "inventory_surfaces": ["extract.pipeline-and-backfill", "extract.match", "extract.merge"],
    },
    context={"timezone": STR, "catalog": ref("catalog"), "runs": arr(ref("run"))},
    defs={
        "catalog": fixtures.CATALOG_DEF,
        "run": fixtures.RUN_DEF,
        "amendment": fixtures.AMENDMENT_DEF,
        "resolved": fixtures.RESOLVED_DEF,
        "planned": fixtures.PLANNED_DEF,
        "burst_message": BURST_MESSAGE,
        "burst": closed(BURST),
        "plan": PLAN,
    },
    ops={
        "plan_burst": Op(BURST, ref("plan")),
        "consolidate": Op(
            {"bursts": arr(ref("burst"), 1)},
            closed({"plans": arr(ref("plan")), "consolidated": arr(ref("planned"))}),
        ),
    },
    cases=cases,
    replay=replay,
)
