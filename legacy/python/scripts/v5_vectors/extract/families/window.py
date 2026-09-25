"""Burst windows: grouping history into bursts, rescan windows, and their bounds."""

from __future__ import annotations

from datetime import datetime, time, timedelta
from typing import Any

from bot.extract import window

from .. import fixtures
from ..contract import (
    BOOL,
    INSTANT,
    INT,
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

ROW = closed({"id": STR, "created_at": INSTANT})
GROUPS = arr(STRS)
ROWS = {"rows": arr(ref("row"))}


def _rows(raw: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        {"id": row["id"], "created_at": datetime.fromisoformat(row["created_at"])} for row in raw
    ]


def _ids(groups: list[list[dict[str, Any]]]) -> list[list[str]]:
    return [[row["id"] for row in group] for group in groups]


def replay(case: dict[str, Any]) -> dict[str, Any]:
    input_ = case["input"]
    tz = fixtures.zone(input_["timezone"])
    reset_weekday, reset_time = input_["reset_weekday"], time.fromisoformat(input_["reset_time"])

    def group_bursts(step: dict) -> list[list[str]]:
        gap = step["gap_seconds"]
        rows = _rows(step["rows"])
        if gap is None:
            return _ids(window.group_bursts(rows))
        return _ids(window.group_bursts(rows, timedelta(seconds=gap)))

    def group_for_rescan(step: dict) -> list[list[str]]:
        cap = step["cap"]
        rows = _rows(step["rows"])
        if cap is None:
            return _ids(window.group_for_rescan(rows, tz))
        return _ids(window.group_for_rescan(rows, tz, cap=cap))

    def split_until(step: dict) -> list[list[str]]:
        limit = step["max_messages"]
        return _ids(window.split_until(_rows(step["rows"]), lambda chunk: len(chunk) <= limit))

    def since(step: dict) -> str:
        now = datetime.fromisoformat(step["now"])
        return window.window_since(step["window"], tz, reset_weekday, reset_time, now).isoformat()

    def previous(step: dict) -> str:
        this_week = datetime.fromisoformat(step["this_week"])
        return window.previous_week_start(this_week, tz, reset_weekday, reset_time).isoformat()

    handlers = {
        "constants": lambda s: {
            "burst_gap_seconds": int(window.BURST_GAP.total_seconds()),
            "max_burst_messages": window.MAX_BURST_MESSAGES,
            "windows": list(window.WINDOWS),
            "default_window": window.DEFAULT_WINDOW,
            "automated_window": window.AUTOMATED_WINDOW,
        },
        "group_bursts": group_bursts,
        "group_for_rescan": group_for_rescan,
        "split_until": split_until,
        "clamp_window": lambda s: window.clamp_window(s["window"], s["automated"]),
        "window_since": since,
        "previous_week_start": previous,
        "should_widen": lambda s: window.should_widen(s["window"], s["gated"], s["automated"]),
    }
    return replay_steps(FAMILY, case, handlers)


def _row(message_id: str, when: str) -> dict[str, str]:
    return {"id": message_id, "created_at": when}


def _day(n: int, hh: int, mm: int, day: int = 30) -> dict[str, str]:
    return _row(str(n), f"2026-08-{day:02d}T{hh:02d}:{mm:02d}:00+08:00")


PLANNING = [
    _day(1, 11, 50),
    _day(2, 12, 40),
    _day(3, 13, 15),
    _day(4, 16, 20),
    _day(5, 16, 21),
    _day(6, 23, 59),
    _row("7", "2026-08-30T16:00:00+00:00"),  # 00:00 local on the 31st
]
#: Thirteen messages on one local day, so the cap forces a split at the longest pause.
BUSY = [_day(10 + i, 20, i * 2) for i in range(6)] + [_day(20 + i, 21, 30 + i) for i in range(7)]
#: Evenly spaced: ties go to the most central split.
EVEN = [_day(40 + i, 10, i * 5) for i in range(5)]


def cases() -> list[dict[str, Any]]:
    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {
            "case_id": case_id,
            "input": {
                "timezone": fixtures.TIMEZONE,
                "reset_weekday": fixtures.RESET_WEEKDAY,
                "reset_time": fixtures.RESET_TIME,
                "steps": steps,
            },
        }

    return [
        case(
            "burst-grouping",
            [
                {"op": "constants"},
                {"op": "group_bursts", "rows": PLANNING, "gap_seconds": None},
                {"op": "group_bursts", "rows": PLANNING, "gap_seconds": 900},
                {"op": "group_bursts", "rows": [], "gap_seconds": None},
                {
                    "op": "group_bursts",
                    "rows": [_day(1, 10, 0), _row("2", "2026-08-30T13:00:00+08:00")],
                    "gap_seconds": None,
                },
                {
                    "op": "group_bursts",
                    "rows": [_day(1, 10, 0), _row("2", "2026-08-30T13:00:01+08:00")],
                    "gap_seconds": None,
                },
            ],
        ),
        case(
            "rescan-days-cap-and-longest-pause",
            [
                {"op": "group_for_rescan", "rows": PLANNING, "cap": None},
                {"op": "group_for_rescan", "rows": BUSY, "cap": None},
                {"op": "group_for_rescan", "rows": BUSY, "cap": 4},
                {"op": "group_for_rescan", "rows": [], "cap": None},
                {"op": "split_until", "rows": BUSY, "max_messages": 5},
                {"op": "split_until", "rows": EVEN, "max_messages": 2},
                {"op": "split_until", "rows": EVEN[:1], "max_messages": 0},
                {"op": "split_until", "rows": EVEN, "max_messages": 5},
            ],
        ),
        case(
            "rescan-window-bounds",
            [
                {"op": "clamp_window", "window": "week", "automated": False},
                {"op": "clamp_window", "window": " 2WEEKS ", "automated": False},
                {"op": "clamp_window", "window": "week", "automated": True},
                {"op": "clamp_window", "window": "24h", "automated": True},
                {"op": "clamp_window", "window": "", "automated": False},
                {
                    "op": "clamp_window",
                    "window": "month",
                    "automated": False,
                    "error_type": "ValueError",
                },
                {"op": "window_since", "window": "24h", "now": "2026-09-02T10:00:00+08:00"},
                {"op": "window_since", "window": "48h", "now": "2026-09-02T10:00:00+08:00"},
                {"op": "window_since", "window": "week", "now": "2026-09-02T10:00:00+08:00"},
                {"op": "window_since", "window": "2weeks", "now": "2026-09-02T10:00:00+08:00"},
                {"op": "window_since", "window": "week", "now": "2026-09-03T00:00:00+08:00"},
                {"op": "window_since", "window": "week", "now": "2026-09-02T23:59:59+08:00"},
                {
                    "op": "window_since",
                    "window": "fortnight",
                    "now": "2026-09-02T10:00:00+08:00",
                    "error_type": "ValueError",
                },
                {"op": "previous_week_start", "this_week": "2026-09-03T00:00:00+08:00"},
                {"op": "should_widen", "window": "week", "gated": 0, "automated": False},
                {"op": "should_widen", "window": "week", "gated": 1, "automated": False},
                {"op": "should_widen", "window": "week", "gated": 0, "automated": True},
                {"op": "should_widen", "window": "48h", "gated": 0, "automated": False},
            ],
        ),
    ]


FAMILY = Family(
    name="window",
    version="v5-extract-window-v1",
    provenance={
        "oracle": "legacy/python bot.extract.window",
        "functions": [
            "bot.extract.window.group_bursts",
            "bot.extract.window.group_for_rescan",
            "bot.extract.window.split_until",
            "bot.extract.window.clamp_window",
            "bot.extract.window.window_since",
            "bot.extract.window.previous_week_start",
            "bot.extract.window.should_widen",
        ],
        "source_tests": ["tests/test_rescan_window.py", "tests/test_extract_pipeline.py"],
        "inventory_surfaces": ["extract.window-and-rescan-window"],
    },
    context={
        "timezone": STR,
        "reset_weekday": {"type": "integer", "minimum": 0, "maximum": 6},
        "reset_time": STR,
    },
    defs={"row": ROW},
    ops={
        "constants": Op(
            {},
            closed(
                {
                    "burst_gap_seconds": INT,
                    "max_burst_messages": INT,
                    "windows": STRS,
                    "default_window": STR,
                    "automated_window": STR,
                }
            ),
        ),
        "group_bursts": Op({**ROWS, "gap_seconds": nullable(INT)}, GROUPS),
        "group_for_rescan": Op({**ROWS, "cap": nullable(INT)}, GROUPS),
        "split_until": Op({**ROWS, "max_messages": INT}, GROUPS),
        "clamp_window": Op({"window": STR, "automated": BOOL}, STR, ("ValueError",)),
        "window_since": Op({"window": STR, "now": INSTANT}, INSTANT, ("ValueError",)),
        "previous_week_start": Op({"this_week": INSTANT}, INSTANT),
        "should_widen": Op({"window": STR, "gated": INT, "automated": BOOL}, BOOL),
    },
    cases=cases,
    replay=replay,
)
