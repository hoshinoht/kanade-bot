"""Synthetic fixtures, shared schema definitions, and oracle-value normalisation.

Everything here is anonymised: user IDs are short digit strings, run IDs are
synthetic UUIDs, and the catalog is a small public-game-name subset rebuilt
through ``BossTable.from_dict`` exactly as the scheduler vectors rebuild theirs.
"""

from __future__ import annotations

from datetime import datetime
from typing import Any
from zoneinfo import ZoneInfo

from bot.domain.bosses import BossTable
from bot.extract.resolve import Resolved
from bot.extract.schema import Amendment

from ..scheduler.mutations.host import catalog as build_catalog
from .contract import (
    BOOL,
    INSTANT,
    NCLOCK,
    NDATE,
    NINSTANT,
    NSTR,
    NUM,
    STR,
    STRS,
    arr,
    closed,
    nullable,
)

TIMEZONE = "Asia/Kuala_Lumpur"
RESET_WEEKDAY = 3
RESET_TIME = "00:00:00"

CATALOG: dict[str, Any] = {
    "difficulties": [
        {"prefix": "e", "label": "Easy"},
        {"prefix": "n", "label": "Normal"},
        {"prefix": "h", "label": "Hard"},
        {"prefix": "c", "label": "Chaos"},
        {"prefix": "x", "label": "Extreme"},
    ],
    "bosses": [
        {
            "short": "MaleficStar",
            "full": "Malefic Star",
            "difficulties": ["n", "h"],
            "aliases": ["star", "mstar"],
        },
        {
            "short": "FA",
            "full": "First Adversary",
            "difficulties": ["n", "h", "x"],
            "aliases": ["fa"],
        },
        {
            "short": "Carling",
            "full": "Chief Carling",
            "difficulties": ["n", "h", "x"],
            "aliases": ["carl", "carling"],
        },
        {
            "short": "Kalos",
            "full": "Kalos the Guardian",
            "difficulties": ["e", "n", "c", "x"],
            "aliases": ["kalos"],
        },
        {
            "short": "Baldrix",
            "full": "Baldrix",
            "difficulties": ["n", "h"],
            "aliases": ["bald", "baldguy"],
        },
        {"short": "Limbo", "full": "Limbo", "difficulties": ["n", "h"], "aliases": ["limbo"]},
    ],
}

#: Roster rows as ``Repo.list_members`` returns them (the fields the prompt reads).
ROSTER: list[dict[str, Any]] = [
    {"user_id": "11", "display_name": "Alvin tan", "nickname": None},
    {"user_id": "22", "display_name": "kanon [AZUR]", "nickname": "kanon"},
    {"user_id": "33", "display_name": "Priya", "nickname": None},
    {"user_id": "44", "display_name": "Mei", "nickname": None},
]

RUN_A = "a1a1a1a1-0000-4000-8000-000000000001"
RUN_B = "b2b2b2b2-0000-4000-8000-000000000002"
RUN_C = "c3c3c3c3-0000-4000-8000-000000000003"
RUN_D = "d4d4d4d4-0000-4000-8000-000000000004"
WEEK1 = "2026-08-27T00:00:00+08:00"
WEEK2 = "2026-09-03T00:00:00+08:00"


def run(
    run_id: str,
    bosses: list[str],
    at: str,
    participants: list[str],
    status: str = "planned",
    channel_id: str | None = "900",
    week_start: str | None = WEEK1,
) -> dict[str, Any]:
    return {
        "id": run_id,
        "bosses": bosses,
        "datetime": at,
        "participants": participants,
        "status": status,
        "channel_id": channel_id,
        "week_start": week_start,
    }


def amendment(kind: str, **fields: Any) -> dict[str, Any]:
    """A canonical ``Amendment`` dump with defaults, as model output normalises to."""
    row: dict[str, Any] = {
        "kind": kind,
        "bosses": [],
        "day_ref": None,
        "time_ref": None,
        "participants": [],
        "rsvp": None,
        "is_question": False,
        "confidence": 0.9,
        "evidence_message_ids": [],
        "target_run_hint": None,
    }
    row.update(fields)
    return row


def msg(message_id: str, author_id: str, created_at: str, content: str) -> dict[str, Any]:
    names = {m["user_id"]: m["nickname"] or m["display_name"] for m in ROSTER}
    return {
        "id": message_id,
        "author_id": author_id,
        "author_name": names.get(author_id, f"user{author_id[-4:]}"),
        "created_at": created_at,
        "content": content,
    }


# --- schema definitions shared by several families ---------------------------------

KINDS = ["move", "add", "cancel", "split", "otot", "sub", "rsvp", "fix"]
RUN_STATUSES = ["planned", "confirmed", "at_risk", "otot", "cancelled", "done"]

CATALOG_DEF = closed(
    {
        "difficulties": arr(closed({"prefix": STR, "label": STR}), 1),
        "bosses": arr(
            closed(
                {"short": STR, "full": STR, "difficulties": STRS, "aliases": STRS},
                ("difficulties",),
            ),
            1,
        ),
    }
)
RUN_DEF = closed(
    {
        "id": STR,
        "bosses": STRS,
        "datetime": INSTANT,
        "participants": STRS,
        "status": {"enum": RUN_STATUSES},
        "channel_id": NSTR,
        "week_start": NINSTANT,
    }
)
MSG_DEF = closed(
    {"id": STR, "author_id": STR, "author_name": STR, "created_at": INSTANT, "content": STR}
)
AMENDMENT_DEF = closed(
    {
        "kind": {"enum": KINDS},
        "bosses": STRS,
        "day_ref": NSTR,
        "time_ref": NSTR,
        "participants": STRS,
        "rsvp": nullable({"enum": ["yes", "no", "maybe"]}),
        "is_question": BOOL,
        "confidence": {"type": "number", "minimum": 0, "maximum": 1},
        "evidence_message_ids": STRS,
        "target_run_hint": NSTR,
    }
)
RESOLVED_DEF = closed(
    {"day": NDATE, "clock": NCLOCK, "at": NINSTANT, "assumed_pm": BOOL, "known": BOOL}
)
EXTRACTION_DEF = closed({"amendments": arr({"$ref": "#/$defs/amendment"}), "summary": STR})
PLANNED_DEF = closed(
    {
        "kind": {"enum": KINDS},
        "amendment": {"$ref": "#/$defs/amendment"},
        "resolved": {"$ref": "#/$defs/resolved"},
        "run_id": NSTR,
        "payload": {"type": "object"},
        "match_reason": STR,
        "match_code": STR,
        "also_mentioned": STRS,
        "ambiguous": BOOL,
        "summary": STR,
        "needs_answer": BOOL,
    }
)
CONFIDENCE = NUM


def table(raw: dict[str, Any]) -> BossTable:
    return build_catalog(raw)


def zone(name: str) -> ZoneInfo:
    return ZoneInfo(name)


def at(value: str | None) -> datetime | None:
    return datetime.fromisoformat(value) if value is not None else None


def run_in(raw: dict[str, Any]) -> dict[str, Any]:
    return {**raw, "datetime": at(raw["datetime"]), "week_start": at(raw["week_start"])}


def amendment_in(raw: dict[str, Any]) -> Amendment:
    return Amendment.model_validate(raw)


def dump_amendment(value: Amendment) -> dict[str, Any]:
    return value.model_dump(mode="json")


def dump_resolved(value: Resolved) -> dict[str, Any]:
    return {
        "day": value.day.isoformat() if value.day else None,
        "clock": value.clock.isoformat() if value.clock else None,
        "at": value.at.isoformat() if value.at else None,
        "assumed_pm": value.assumed_pm,
        "known": value.known,
    }
