"""Day/time resolution of the model's literal ``day_ref``/``time_ref``."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from bot.extract import resolve

from .. import fixtures
from ..contract import (
    BOOL,
    CLOCK,
    INSTANT,
    INT,
    NSTR,
    STR,
    Family,
    Op,
    closed,
    nullable,
    ref,
    replay_steps,
)

REF = {"time_ref": NSTR}


def replay(case: dict[str, Any]) -> dict[str, Any]:
    tz = fixtures.zone(case["input"]["timezone"])

    def parse_clock(step: dict) -> dict[str, Any] | None:
        found = resolve.parse_clock(step["time_ref"])
        if found is None:
            return None
        return {"clock": found[0].isoformat(), "assumed_pm": found[1]}

    def resolved(step: dict) -> dict[str, Any]:
        anchor = datetime.fromisoformat(step["anchor"])
        return fixtures.dump_resolved(
            resolve.resolve(step["day_ref"], step["time_ref"], anchor, tz)
        )

    handlers = {
        "pm_cutoff": lambda s: resolve.PM_CUTOFF,
        "parse_clock": parse_clock,
        "resolve": resolved,
    }
    return replay_steps(FAMILY, case, handlers)


CLOCKS = [
    "9:30pm",
    "930",
    "9",
    "10",
    "at 11",
    "12",
    "12am",
    "12pm",
    "13",
    "2130",
    "1030~11+pm",
    "8~1130",
    "9-10pm",
    "9pm-11pm",
    "11 to 1145pm",
    "9.30 pm",
    "9 a.m.",
    "11pm onward",
    "9pm ish",
    "around 10",
    "2530",
    "990",
    "night",
    "later",
    "after boss",
    "",
    None,
    "24",
    "0",
    "00:30",
]

#: Sunday 2026-08-30 13:07 in Kuala Lumpur.
SUNDAY = "2026-08-30T13:07:00+08:00"
#: Wednesday 2026-09-02 22:10 in Kuala Lumpur.
LATE_WED = "2026-09-02T22:10:00+08:00"


def _r(day: str | None, time: str | None, anchor: str = SUNDAY) -> dict[str, Any]:
    return {"op": "resolve", "day_ref": day, "time_ref": time, "anchor": anchor}


def cases() -> list[dict[str, Any]]:
    def case(case_id: str, steps: list[dict[str, Any]], timezone: str = fixtures.TIMEZONE):
        return {"case_id": case_id, "input": {"timezone": timezone, "steps": steps}}

    return [
        case(
            "clock-parsing-and-pm-rule",
            [{"op": "pm_cutoff"}] + [{"op": "parse_clock", "time_ref": ref_} for ref_ in CLOCKS],
        ),
        case(
            "relative-days-and-weekdays",
            [
                _r("tonight", "930"),
                _r("today", "9pm"),
                _r("tmr", "930"),
                _r("tomorrow night", None),
                _r("ytd", "10pm"),
                _r("weds", "9:30pm"),
                _r("wed", None),
                _r("this sunday", None),
                _r("sun", "10pm"),
                _r("sun", "11am"),
                _r("next sun", "10pm"),
                _r("next wed", None),
                _r("coming thurs", "1030~11+pm"),
                _r("2026-09-04", "21:30"),
                _r("2026-02-30", "21:30"),
                _r("later", "at 11"),
                _r("later", "1pm"),
                _r("sometime", "after boss"),
                _r(None, None),
                _r(None, "11am"),
                _r(None, "9pm"),
                _r("  ", "  "),
            ],
        ),
        case(
            "past-times-roll-forward-unless-explicit",
            [
                _r("today", "9pm", LATE_WED),
                _r("tonight", "10", LATE_WED),
                _r("wed", "9:30pm", LATE_WED),
                _r("wed", "11pm", LATE_WED),
                _r("later", "9pm", LATE_WED),
                _r(None, "9pm", LATE_WED),
                _r("tmr", "9pm", "2026-09-02T14:10:00+00:00"),
            ],
        ),
        case(
            "wall-clock-gap-in-dst-zone",
            [
                _r("today", "2:30am", "2026-03-08T00:30:00-05:00"),
                _r("sun", "2:30am", "2026-03-07T12:00:00-05:00"),
                _r("tonight", "1:30am", "2026-11-01T00:10:00-04:00"),
            ],
            timezone="America/New_York",
        ),
    ]


FAMILY = Family(
    name="resolve",
    version="v5-extract-resolve-v1",
    provenance={
        "oracle": "legacy/python bot.extract.resolve",
        "functions": ["bot.extract.resolve.parse_clock", "bot.extract.resolve.resolve"],
        "source_tests": ["tests/test_extract_resolve.py"],
        "inventory_surfaces": ["extract.resolve"],
    },
    context={"timezone": STR},
    defs={"resolved": fixtures.RESOLVED_DEF},
    ops={
        "pm_cutoff": Op({}, INT),
        "parse_clock": Op(REF, nullable(closed({"clock": CLOCK, "assumed_pm": BOOL}))),
        "resolve": Op({"day_ref": NSTR, **REF, "anchor": INSTANT}, ref("resolved")),
    },
    cases=cases,
    replay=replay,
)
