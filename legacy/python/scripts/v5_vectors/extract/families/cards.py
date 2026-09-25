"""Extraction card rendering: one proposal/suggestion card per burst.

The rows are amendment rows as ``Pipeline._post_card`` hands them to
``formatting.proposal_card`` (``also_mentioned`` already lifted out of the
payload); ``unanswered`` is ``Pipeline._unanswered`` over the same rows.
"""

from __future__ import annotations

from typing import Any

from bot.agent import formatting
from bot.extract.pipeline import Pipeline

from .. import fixtures
from ..contract import (
    BOOL,
    INT,
    NINSTANT,
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
from ..fixtures import RUN_A, RUN_B, run

ROW = closed(
    {
        "id": STR,
        "kind": {"enum": fixtures.KINDS},
        "bosses": STRS,
        "run_id": NSTR,
        "new_datetime": NINSTANT,
        "participants": STRS,
        "is_question": BOOL,
        "confidence": nullable({"type": "number"}),
        "rsvp": NSTR,
        "day_ref": NSTR,
        "time_ref": NSTR,
        "summary": NSTR,
        "payload": {"type": "object"},
        "also_mentioned": STRS,
    }
)
AUDIENCE = closed({"names": {"type": "object", "additionalProperties": STR}, "mentioned": STRS})
CARD = closed(
    {
        "content": STR,
        "title": NSTR,
        "description": NSTR,
        "fields": arr({"type": "array", "prefixItems": [STR, STR], "items": False}),
        "footer": NSTR,
        "colour": INT,
        "mention_users": STRS,
    }
)
ROWS = {"amendments": arr(ref("row")), "runs": arr(ref("run"))}


def _row(raw: dict[str, Any]) -> dict[str, Any]:
    return {**raw, "new_datetime": fixtures.at(raw["new_datetime"])}


def _who(raw: dict[str, Any] | None) -> formatting.Audience | None:
    if raw is None:
        return None
    return formatting.Audience(names=dict(raw["names"]), mentioned=tuple(raw["mentioned"]))


def _card(card: formatting.Card) -> dict[str, Any]:
    return {
        "content": card.content,
        "title": card.title,
        "description": card.description,
        "fields": [list(item) for item in card.fields],
        "footer": card.footer,
        "colour": card.colour,
        "mention_users": list(card.mention_users),
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    tz = fixtures.zone(case["input"]["timezone"])

    def runs(raw: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
        return {item["id"]: fixtures.run_in(item) for item in raw}

    def line(step: dict) -> list[str]:
        target = fixtures.run_in(step["run"]) if step["run"] else None
        return list(formatting.proposal_line(_row(step["row"]), target, tz, _who(step["audience"])))

    def card(step: dict) -> dict[str, Any]:
        return _card(
            formatting.proposal_card(
                [_row(r) for r in step["amendments"]],
                runs(step["runs"]),
                tz,
                unanswered=step["unanswered"],
                confidence=step["confidence"],
                who=_who(step["audience"]),
            )
        )

    handlers = {
        "when_text": lambda s: formatting.when_text(_row(s["row"]), tz),
        "proposal_line": line,
        "card_kind": lambda s: formatting.card_kind([_row(r) for r in s["amendments"]]),
        "unanswered": lambda s: Pipeline._unanswered(  # noqa: SLF001 - the card's own rule
            [_row(r) for r in s["amendments"]], runs(s["runs"])
        ),
        "proposal_card": card,
        "notices": lambda s: {
            "superseded": formatting.SUPERSEDED_NOTICE,
            "confirm_hint": formatting.CONFIRM_HINT,
            "tbd": formatting.TBD,
            "applied": formatting.applied_notice(s["name"]),
            "rejected": formatting.rejected_notice(s["name"]),
        },
    }
    return replay_steps(FAMILY, case, handlers)


RA = run(RUN_A, ["HMaleficStar", "HFA"], "2026-08-31T21:30:00+08:00", ["1", "2", "3"])
RB = run(RUN_B, ["HCarling", "XKalos"], "2026-09-01T22:00:00+08:00", ["1", "4"])


def row(kind: str, n: int = 1, **fields: Any) -> dict[str, Any]:
    out: dict[str, Any] = {
        "id": f"f0f0f0f0-0000-4000-8000-{n:012d}",
        "kind": kind,
        "bosses": [],
        "run_id": None,
        "new_datetime": None,
        "participants": [],
        "is_question": False,
        "confidence": 0.9,
        "rsvp": None,
        "day_ref": None,
        "time_ref": None,
        "summary": None,
        "payload": {},
        "also_mentioned": [],
    }
    out.update(fields)
    return out


WHO = {"names": {"1": "Alvin", "2": "kanon", "3": "Priya", "4": "Mei"}, "mentioned": ["1", "4"]}


def cases() -> list[dict[str, Any]]:
    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {"case_id": case_id, "input": {"timezone": fixtures.TIMEZONE, "steps": steps}}

    def when(**fields: Any) -> dict[str, Any]:
        return {"op": "when_text", "row": row("move", **fields)}

    def line(r: dict, target: dict | None = None, audience: dict | None = None):
        return {"op": "proposal_line", "row": r, "run": target, "audience": audience}

    move = row(
        "move",
        run_id=RUN_A,
        new_datetime="2026-09-02T21:30:00+08:00",
        bosses=["HMaleficStar"],
        participants=["1"],
        summary="moving to wed",
        also_mentioned=["otot", "cancel"],
    )
    return [
        case(
            "when-text-never-invents",
            [
                when(new_datetime="2026-09-02T21:30:00+08:00"),
                when(new_datetime="2026-09-02T16:05:00+00:00"),
                when(day_ref="wed"),
                when(time_ref="9:30pm"),
                when(day_ref="sometime", time_ref="after boss"),
                when(),
            ],
        ),
        case(
            "one-line-per-kind",
            [
                line(move, RA),
                line(move, RA, WHO),
                line(row("move", day_ref="wed", bosses=["HCarling"])),
                line(
                    row(
                        "add",
                        bosses=["NMaleficStar"],
                        day_ref="tmr",
                        time_ref="930",
                        is_question=True,
                        participants=["2"],
                    )
                ),
                line(
                    row(
                        "split",
                        run_id=RUN_A,
                        bosses=["HFA"],
                        new_datetime="2026-09-01T21:00:00+08:00",
                    ),
                    RA,
                ),
                line(row("cancel", run_id=RUN_B), RB),
                line(row("otot", run_id=RUN_B), RB, WHO),
                line(
                    row(
                        "sub",
                        run_id=RUN_B,
                        participants=["4"],
                        payload={"remove": ["4"], "add": ["3"]},
                    ),
                    RB,
                    WHO,
                ),
                line(
                    row("sub", run_id=RUN_B, participants=["4"], payload={"remove": [], "add": []}),
                    RB,
                ),
                line(row("rsvp", run_id=RUN_A, participants=["3"], rsvp="maybe"), RA),
                line(row("rsvp", run_id=RUN_A, participants=["3"], rsvp="later"), RA),
                line(
                    row(
                        "fix",
                        bosses=["HLimbo", "NBaldrix"],
                        participants=["1"],
                        payload={"weekday": 1, "time": "22:30"},
                    )
                ),
                line(row("fix", bosses=["HLimbo"], day_ref="tue", payload={})),
                line(
                    row(
                        "fix",
                        bosses=["NKalos"],
                        participants=["1", "2"],
                        payload={
                            "op": "edit",
                            "weekday": 4,
                            "time": "20:00",
                            "weekly_when": "Thu 21:00",
                            "participants": ["1", "3"],
                        },
                    )
                ),
                line(
                    row(
                        "fix",
                        bosses=["NKalos"],
                        participants=["1"],
                        payload={"op": "edit", "weekly_when": "Thu 21:00"},
                    )
                ),
                line(
                    row(
                        "fix",
                        bosses=["NKalos"],
                        payload={"op": "remove", "weekly_when": "Thu 21:00"},
                    )
                ),
                line(row("add", bosses=[])),
            ],
        ),
        case(
            "card-kinds-and-waiting-people",
            [
                {"op": "card_kind", "amendments": [row("fix", payload={"weekday": 1})]},
                {"op": "card_kind", "amendments": [move]},
                {"op": "card_kind", "amendments": [move, row("fix", 2)]},
                {
                    "op": "card_kind",
                    "amendments": [
                        row("move", is_question=True, new_datetime="2026-09-02T21:30:00+08:00")
                    ],
                },
                {"op": "card_kind", "amendments": []},
                {"op": "unanswered", "amendments": [move], "runs": [RA]},
                {
                    "op": "unanswered",
                    "amendments": [row("move", run_id=RUN_A, day_ref="wed", participants=["1"])],
                    "runs": [RA],
                },
                {
                    "op": "unanswered",
                    "amendments": [
                        row("move", run_id=RUN_A, is_question=True, participants=["2"]),
                        row("otot", 2, run_id=RUN_B, participants=[]),
                    ],
                    "runs": [RA, RB],
                },
                {"op": "notices", "name": "kanon"},
            ],
        ),
        case(
            "one-card-per-burst",
            [
                {
                    "op": "proposal_card",
                    "amendments": [
                        move,
                        row("otot", 2, run_id=RUN_B, participants=[]),
                        row(
                            "add",
                            3,
                            bosses=["NMaleficStar"],
                            day_ref="tmr",
                            participants=["2"],
                            is_question=True,
                            confidence=0.6,
                        ),
                    ],
                    "runs": [RA, RB],
                    "unanswered": ["2", "3"],
                    "confidence": 0.6,
                    "audience": WHO,
                },
                {
                    "op": "proposal_card",
                    "amendments": [move],
                    "runs": [RA],
                    "unanswered": None,
                    "confidence": None,
                    "audience": None,
                },
                {
                    "op": "proposal_card",
                    "amendments": [
                        row("fix", bosses=["HLimbo"], payload={"weekday": 1, "time": "22:30"})
                    ],
                    "runs": [],
                    "unanswered": [],
                    "confidence": 0.9,
                    "audience": {"names": {}, "mentioned": []},
                },
            ],
        ),
    ]


FAMILY = Family(
    name="cards",
    version="v5-extract-cards-v1",
    provenance={
        "oracle": "legacy/python bot.agent.formatting, bot.extract.pipeline.Pipeline._unanswered",
        "functions": [
            "bot.agent.formatting.when_text",
            "bot.agent.formatting.proposal_line",
            "bot.agent.formatting.card_kind",
            "bot.agent.formatting.proposal_card",
            "bot.agent.formatting.applied_notice",
            "bot.agent.formatting.rejected_notice",
            "bot.extract.pipeline.Pipeline._unanswered",
        ],
        "source_tests": ["tests/test_extract_cards.py", "tests/test_extract_pipeline.py"],
        "inventory_surfaces": ["discord.reaction-rsvp-and-card-review", "extract.commit.add"],
    },
    context={"timezone": STR},
    defs={"row": ROW, "run": fixtures.RUN_DEF, "audience": AUDIENCE},
    ops={
        "when_text": Op({"row": ref("row")}, STR),
        "proposal_line": Op(
            {"row": ref("row"), "run": nullable(ref("run")), "audience": nullable(ref("audience"))},
            {"type": "array", "prefixItems": [STR, STR], "items": False, "minItems": 2},
        ),
        "card_kind": Op(
            {"amendments": arr(ref("row"))}, {"enum": ["fix", "suggestion", "proposal"]}
        ),
        "unanswered": Op(ROWS, STRS),
        "proposal_card": Op(
            {
                **ROWS,
                "unanswered": nullable(STRS),
                "confidence": nullable({"type": "number"}),
                "audience": nullable(ref("audience")),
            },
            CARD,
        ),
        "notices": Op(
            {"name": STR},
            closed(
                {
                    "superseded": STR,
                    "confirm_hint": STR,
                    "tbd": STR,
                    "applied": STR,
                    "rejected": STR,
                }
            ),
        ),
    },
    cases=cases,
    replay=replay,
)
