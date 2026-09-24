"""Replay serialized pure-domain vector inputs against the v4 oracle."""

from __future__ import annotations

from datetime import datetime, time
from typing import Any
from zoneinfo import ZoneInfo

from bot.domain import ids, timeutil, weeks
from bot.domain.bosses import BossTable


def _instant(value: str) -> datetime:
    return datetime.fromisoformat(value)


def _time(value: str) -> time:
    return time.fromisoformat(value)


def _table(case: dict[str, Any], fixtures: dict[str, Any]) -> BossTable:
    catalog = fixtures["catalogs"][case["input"]["catalog_id"]]
    difficulties = catalog["difficulties"]
    bosses = catalog["bosses"]
    prefixes = [entry["prefix"] for entry in difficulties]
    shorts = [entry["short"] for entry in bosses]
    if len(set(prefixes)) != len(prefixes) or len(set(shorts)) != len(shorts):
        raise ValueError("synthetic catalog has duplicate ordered entries")
    return BossTable.from_dict(
        {
            "difficulties": {entry["prefix"]: entry["label"] for entry in difficulties},
            "bosses": {
                entry["short"]: {key: value for key, value in entry.items() if key != "short"}
                for entry in bosses
            },
        }
    )


def dispatch(case: dict[str, Any], fixtures: dict[str, Any]) -> Any:
    """Invoke exactly the operation and arguments serialized in one case."""
    input_ = case["input"]
    op = case["op"]
    if op == "week_start":
        return weeks.week_start(
            _instant(input_["at"]),
            ZoneInfo(input_["timezone"]),
            input_["reset_weekday"],
            _time(input_["reset_time"]),
        )
    if op == "calendar_week_bounds":
        tz = ZoneInfo(input_["timezone"])
        at = _instant(input_["at"])
        start = weeks.calendar_week_start(at, tz)
        return {
            "start": start,
            "end": weeks.calendar_week_end(start, tz),
            "boss_start": weeks.week_start(
                at, tz, input_["reset_weekday"], _time(input_["reset_time"])
            ),
        }
    if op == "materialised_week_starts":
        return weeks.materialised_week_starts(
            ZoneInfo(input_["timezone"]),
            input_["reset_weekday"],
            _time(input_["reset_time"]),
            _instant(input_["at"]),
        )
    if op == "slot_in_week":
        return weeks.slot_in_week(
            _instant(input_["week_start"]),
            ZoneInfo(input_["timezone"]),
            input_["weekday"],
            _time(input_["time"]),
        )
    if op == "week_end":
        return weeks.week_end(_instant(input_["week_start"]), ZoneInfo(input_["timezone"]))
    if op == "parse_weekday":
        return weeks.parse_weekday(input_["value"])
    if op == "parse_hhmm":
        return weeks.parse_hhmm(input_["value"])
    if op == "to_iso":
        return timeutil.to_iso(_instant(input_["at"]))
    if op == "from_iso":
        return timeutil.from_iso(input_["value"])
    if op == "local_naive":
        return timeutil.local_naive(_instant(input_["at"]), ZoneInfo(input_["timezone"]))
    if op == "canonical":
        return ids.canonical(input_["value"])
    if op == "short_id":
        return ids.short_id(input_["value"])
    if op == "tag":
        return ids.tag(input_["value"])
    if op == "resolve_id":
        return ids.resolve_id(input_["text"], input_["candidates"])
    table = _table(case, fixtures)
    if op == "parse_token":
        return table.parse_token(input_["token"])
    if op == "parse":
        return table.parse(input_["text"])
    if op == "resolve_reference":
        reference = table.resolve_reference(input_["text"])
        return {"short": reference.short, "difficulty": reference.difficulty}
    if op == "names_in":
        return table.names_in(input_["text"])
    if op == "describe":
        return table.describe(input_["canonical"])
    if op == "detail":
        return table.detail(input_["canonical"])
    raise ValueError(f"unknown vector operation {op!r}")
