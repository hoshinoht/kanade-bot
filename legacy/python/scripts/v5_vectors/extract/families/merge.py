"""Merging one burst's per-message amendments into one candidate per target."""

from __future__ import annotations

from typing import Any

from bot.extract.merge import merge

from .. import fixtures
from ..contract import STRS, Family, Op, arr, ref, replay_steps
from ..fixtures import amendment as a


def replay(case: dict[str, Any]) -> dict[str, Any]:
    def run(step: dict) -> list[dict[str, Any]]:
        merged = merge(
            [fixtures.amendment_in(raw) for raw in step["amendments"]],
            step["message_order"],
            step["existing_bosses"],
        )
        return [fixtures.dump_amendment(item) for item in merged]

    return replay_steps(FAMILY, case, {"merge": run})


def _merge(amendments: list[dict], order: list[str], existing: list[list[str]] = ()):
    return {
        "op": "merge",
        "amendments": amendments,
        "message_order": order,
        "existing_bosses": [list(item) for item in existing],
    }


def cases() -> list[dict[str, Any]]:
    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {"case_id": case_id, "input": {"steps": steps}}

    add = a(
        "add",
        bosses=["NMaleficStar", "NCarling"],
        day_ref="tonight",
        participants=["11"],
        is_question=True,
        confidence=0.7,
        evidence_message_ids=["401"],
    )
    return [
        case(
            "latest-explicit-value-wins",
            [
                _merge(
                    [
                        a(
                            "move",
                            bosses=["HMaleficStar"],
                            time_ref="9pm",
                            evidence_message_ids=["2"],
                        ),
                        a(
                            "move",
                            bosses=["HMaleficStar"],
                            time_ref="9:45pm",
                            confidence=0.6,
                            evidence_message_ids=["3"],
                            participants=["22"],
                        ),
                        a(
                            "move",
                            bosses=["HMaleficStar"],
                            day_ref="wed",
                            is_question=True,
                            evidence_message_ids=["1"],
                            participants=["11"],
                        ),
                    ],
                    ["1", "2", "3"],
                    [["HMaleficStar", "HFA"]],
                ),
                _merge(
                    [
                        a(
                            "move",
                            bosses=["HMaleficStar"],
                            time_ref="9pm",
                            evidence_message_ids=["9"],
                        ),
                        a(
                            "move",
                            bosses=["HMaleficStar"],
                            time_ref="10pm",
                            evidence_message_ids=["8"],
                        ),
                    ],
                    [],
                    [["HMaleficStar"]],
                ),
                _merge([], ["1"]),
            ],
        ),
        case(
            "rsvps-are-keyed-by-person",
            [
                _merge(
                    [
                        a("rsvp", participants=["11"], rsvp="yes", evidence_message_ids=["1"]),
                        a("rsvp", participants=["22"], rsvp="yes", evidence_message_ids=["2"]),
                        a("rsvp", participants=["11"], rsvp="no", evidence_message_ids=["3"]),
                    ],
                    ["1", "2", "3"],
                ),
            ],
        ),
        case(
            "time-changes-fold-into-the-add",
            [
                _merge(
                    [
                        add,
                        a(
                            "move",
                            bosses=["NMaleficStar"],
                            time_ref="9pm",
                            participants=["22"],
                            evidence_message_ids=["402"],
                            confidence=0.8,
                        ),
                        a(
                            "move",
                            bosses=["NMaleficStar"],
                            time_ref="9:45pm",
                            evidence_message_ids=["403"],
                            confidence=0.85,
                        ),
                    ],
                    ["401", "402", "403"],
                    [["HMaleficStar", "HFA"]],
                ),
                _merge(
                    [
                        add,
                        a(
                            "move",
                            bosses=["NMaleficStar"],
                            time_ref="9pm",
                            evidence_message_ids=["402"],
                        ),
                    ],
                    ["401", "402"],
                    [["NMaleficStar"]],
                ),
                _merge(
                    [
                        a(
                            "move",
                            bosses=["NMaleficStar"],
                            time_ref="9pm",
                            evidence_message_ids=["400"],
                        ),
                        add,
                    ],
                    ["400", "401"],
                    [],
                ),
                _merge(
                    [add, a("move", time_ref="9pm", evidence_message_ids=["402"])],
                    ["401", "402"],
                    [],
                ),
            ],
        ),
        case(
            "one-time-carries-across-moves-to-the-same-day",
            [
                _merge(
                    [
                        a(
                            "move",
                            bosses=["HMaleficStar", "HFA"],
                            day_ref="wed",
                            is_question=True,
                            evidence_message_ids=["1"],
                        ),
                        a(
                            "move",
                            bosses=["HCarling", "XKalos"],
                            day_ref="Wed",
                            is_question=True,
                            evidence_message_ids=["1"],
                        ),
                        a(
                            "move",
                            bosses=["HCarling", "XKalos"],
                            day_ref="wed",
                            time_ref="9:30pm",
                            evidence_message_ids=["2"],
                        ),
                    ],
                    ["1", "2"],
                    [["HMaleficStar", "HFA"], ["HCarling", "XKalos"]],
                ),
                _merge(
                    [
                        a(
                            "move",
                            bosses=["HMaleficStar"],
                            day_ref="wed",
                            time_ref="9pm",
                            evidence_message_ids=["1"],
                        ),
                        a(
                            "move",
                            bosses=["HCarling"],
                            day_ref="wed",
                            time_ref="10pm",
                            evidence_message_ids=["2"],
                        ),
                        a("move", bosses=["XKalos"], day_ref="wed", evidence_message_ids=["3"]),
                    ],
                    ["1", "2", "3"],
                    [["HMaleficStar"], ["HCarling", "XKalos"]],
                ),
            ],
        ),
    ]


FAMILY = Family(
    name="merge",
    version="v5-extract-merge-v1",
    provenance={
        "oracle": "legacy/python bot.extract.merge",
        "functions": ["bot.extract.merge.merge"],
        "source_tests": ["tests/test_extract_merge.py"],
        "inventory_surfaces": ["extract.merge"],
    },
    context={},
    defs={"amendment": fixtures.AMENDMENT_DEF},
    ops={
        "merge": Op(
            {
                "amendments": arr(ref("amendment")),
                "message_order": STRS,
                "existing_bosses": arr(STRS),
            },
            arr(ref("amendment")),
        )
    },
    cases=cases,
    replay=replay,
)
