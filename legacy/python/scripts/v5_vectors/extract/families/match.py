"""Run matching: ``match_run``, ``runs_spanned``, ``reachable`` and ``needs_run``."""

from __future__ import annotations

from datetime import date
from typing import Any

from bot.extract import match

from .. import fixtures
from ..contract import (
    BOOL,
    NDATE,
    NSTR,
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
from ..fixtures import RUN_A, RUN_B, RUN_C, RUN_D, WEEK2, amendment, run

RESULT = closed(
    {
        "run_id": NSTR,
        "reason": STR,
        "candidate_ids": STRS,
        "ambiguous": BOOL,
        "reason_code": STR,
        "matched": BOOL,
    }
)


def replay(case: dict[str, Any]) -> dict[str, Any]:
    input_ = case["input"]
    tz = fixtures.zone(input_["timezone"])
    pool = {raw["id"]: fixtures.run_in(raw) for raw in input_["runs"]}

    def runs(ids: list[str]) -> list[dict[str, Any]]:
        missing = [run_id for run_id in ids if run_id not in pool]
        if missing:
            raise ContractError(f"{case['case_id']}: undeclared run IDs {missing}")
        return [pool[run_id] for run_id in ids]

    def match_run(step: dict) -> dict[str, Any]:
        result = match.match_run(
            fixtures.amendment_in(step["amendment"]),
            runs(step["channel_runs"]),
            runs(step["guild_runs"]),
            author_id=step["author_id"],
            mentioned=step["mentioned"],
        )
        return {
            "run_id": result.run["id"] if result.run else None,
            "reason": result.reason,
            "candidate_ids": [candidate["id"] for candidate in result.candidates],
            "ambiguous": result.ambiguous,
            "reason_code": result.reason_code,
            "matched": result.matched,
        }

    def spanned(step: dict) -> list[str]:
        found = match.runs_spanned(
            fixtures.amendment_in(step["amendment"]),
            runs(step["channel_runs"]),
            author_id=step["author_id"],
        )
        return [row["id"] for row in found]

    def reachable(step: dict) -> list[str]:
        day = date.fromisoformat(step["day"]) if step["day"] else None
        return [row["id"] for row in match.reachable(runs(step["runs"]), day, tz)]

    handlers = {
        "match_run": match_run,
        "runs_spanned": spanned,
        "reachable": reachable,
        "needs_run": lambda s: match.needs_run(s["kind"]),
    }
    return replay_steps(FAMILY, case, handlers)


#: Two runs in channel 900, one in 901, one next week in 900, one cancelled.
RUNS = [
    run(RUN_A, ["HMaleficStar", "HFA"], "2026-08-31T21:30:00+08:00", ["11", "22", "33"]),
    run(RUN_B, ["HCarling", "XKalos"], "2026-09-01T22:00:00+08:00", ["11", "22", "44"]),
    run(
        RUN_C,
        ["NMaleficStar"],
        "2026-09-02T21:00:00+08:00",
        ["33", "44"],
        channel_id="901",
    ),
    run(
        RUN_D,
        ["HMaleficStar", "HFA"],
        "2026-09-07T21:30:00+08:00",
        ["11", "22", "33"],
        week_start=WEEK2,
    ),
    run(
        "e5e5e5e5-0000-4000-8000-000000000005",
        ["NBaldrix"],
        "2026-08-30T21:00:00+08:00",
        ["11"],
        status="cancelled",
    ),
]
DEAD = "e5e5e5e5-0000-4000-8000-000000000005"
HOME = [RUN_A, RUN_B, DEAD]


def _m(amend: dict, channel: list[str], guild: list[str] = (), author=None, mentioned=()):
    return {
        "op": "match_run",
        "amendment": amend,
        "channel_runs": list(channel),
        "guild_runs": list(guild),
        "author_id": author,
        "mentioned": list(mentioned),
    }


def cases() -> list[dict[str, Any]]:
    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {
            "case_id": case_id,
            "input": {"timezone": fixtures.TIMEZONE, "runs": RUNS, "steps": steps},
        }

    move = amendment("move", day_ref="wed")
    return [
        case(
            "boss-and-participant-scoring",
            [
                _m(amendment("move", bosses=["HMaleficStar"]), HOME, author="33"),
                _m(amendment("move", bosses=["HCarling"]), HOME, author="11"),
                _m(amendment("move", bosses=["HMaleficStar", "HCarling"]), HOME, author="11"),
                _m(amendment("move", bosses=["NBaldrix"]), HOME, author="11"),
                _m(move, HOME, author="33"),
                _m(move, HOME, author="11"),
                _m(move, [RUN_A], author="99"),
                _m(move, [DEAD], [RUN_C], author="99"),
                _m(move, [DEAD], [RUN_C], author="44"),
                _m(amendment("cancel", bosses=["NMaleficStar"]), [], [RUN_C], author="44"),
                _m(amendment("cancel"), [], [], author="44"),
                _m(amendment("rsvp", participants=["44"]), HOME, mentioned=["44"]),
            ],
        ),
        case(
            "model-hints-are-checked",
            [
                _m(amendment("move", target_run_hint="#b2b2"), HOME),
                _m(amendment("move", target_run_hint="b2b2b2b2"), HOME),
                _m(amendment("move", target_run_hint="#b2b"), HOME, author="33"),
                _m(amendment("move", bosses=["HFA"], target_run_hint="#b2b2b2"), HOME, author="33"),
                _m(amendment("move", target_run_hint="#e5e5e5"), HOME, author="11"),
                _m(amendment("move", target_run_hint="#ffff"), HOME, author="11"),
            ],
        ),
        case(
            "spanning-reachability-and-kinds",
            [
                {
                    "op": "runs_spanned",
                    "amendment": amendment("sub", bosses=["HMaleficStar", "HCarling"]),
                    "channel_runs": HOME,
                    "author_id": "11",
                },
                {
                    "op": "runs_spanned",
                    "amendment": amendment("sub", bosses=["HMaleficStar", "HCarling"]),
                    "channel_runs": HOME,
                    "author_id": "33",
                },
                {
                    "op": "runs_spanned",
                    "amendment": amendment("sub", bosses=["HMaleficStar", "HCarling"]),
                    "channel_runs": HOME,
                    "author_id": "99",
                },
                {
                    "op": "runs_spanned",
                    "amendment": amendment("sub"),
                    "channel_runs": HOME,
                    "author_id": None,
                },
                {"op": "reachable", "runs": [RUN_A, RUN_D], "day": "2026-09-02"},
                {"op": "reachable", "runs": [RUN_A, RUN_D], "day": "2026-09-03"},
                {"op": "reachable", "runs": [RUN_A, RUN_D], "day": None},
            ]
            + [{"op": "needs_run", "kind": kind} for kind in fixtures.KINDS],
        ),
    ]


FAMILY = Family(
    name="match",
    version="v5-extract-match-v1",
    provenance={
        "oracle": "legacy/python bot.extract.match",
        "functions": [
            "bot.extract.match.match_run",
            "bot.extract.match.runs_spanned",
            "bot.extract.match.reachable",
            "bot.extract.match.needs_run",
        ],
        "source_tests": ["tests/test_extract_match.py"],
        "inventory_surfaces": ["extract.match"],
    },
    context={"timezone": STR, "runs": arr(ref("run"))},
    defs={"run": fixtures.RUN_DEF, "amendment": fixtures.AMENDMENT_DEF},
    ops={
        "match_run": Op(
            {
                "amendment": ref("amendment"),
                "channel_runs": STRS,
                "guild_runs": STRS,
                "author_id": NSTR,
                "mentioned": STRS,
            },
            RESULT,
        ),
        "runs_spanned": Op(
            {"amendment": ref("amendment"), "channel_runs": STRS, "author_id": NSTR}, STRS
        ),
        "reachable": Op({"runs": STRS, "day": NDATE}, STRS),
        "needs_run": Op({"kind": {"enum": fixtures.KINDS}}, BOOL),
    },
    cases=cases,
    replay=replay,
)
