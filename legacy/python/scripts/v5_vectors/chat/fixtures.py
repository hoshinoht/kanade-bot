"""Synthetic guild, roster, channels and schedule shared by the chat families.

Every ID is a short synthetic digit string or a synthetic UUID; the catalog is
the extraction vectors' public-name subset. The clock is Wednesday
2026-09-09 12:00 in Kuala Lumpur, mid boss week (reset Thursday 00:00), so the
world spans two boss weeks and one calendar week.
"""

from __future__ import annotations

from typing import Any

from ..extract.fixtures import CATALOG, CATALOG_DEF, RUN_STATUSES
from .contract import BOOL, INSTANT, INT, NSTR, STR, STRS, arr, closed, nullable

TIMEZONE = "Asia/Kuala_Lumpur"
CLOCK = "2026-09-09T12:00:00+08:00"
WEEK1 = "2026-09-03T00:00:00+08:00"
WEEK2 = "2026-09-10T00:00:00+08:00"

GUILD = "1000"
BOT_USER = "5000"
SELF_ROLE = "5001"
CHAT_ROLE = "6000"
OTHER_ROLE = "6001"
CHAT_CHANNEL = "700"
GENERAL = "701"
ADOPTED = "702"
ORPHAN = "703"
THREAD = "704"
CHAT_CATEGORY = "800"
OTHER_CATEGORY = "801"
PARTY_STAR = "900"
PARTY_KALOS = "901"

CHANNELS: list[dict[str, Any]] = [
    {"id": CHAT_CHANNEL, "name": "ask-the-bot", "category_id": None, "parent_id": None},
    {"id": GENERAL, "name": "general", "category_id": None, "parent_id": None},
    {"id": ADOPTED, "name": "bot-chatter", "category_id": CHAT_CATEGORY, "parent_id": None},
    {"id": ORPHAN, "name": "somewhere-else", "category_id": OTHER_CATEGORY, "parent_id": None},
    {"id": THREAD, "name": "a-thread", "category_id": None, "parent_id": CHAT_CHANNEL},
    {"id": PARTY_STAR, "name": "party-star", "category_id": None, "parent_id": None},
    {"id": PARTY_KALOS, "name": "party-kalos", "category_id": None, "parent_id": None},
]

SETTINGS: dict[str, Any] = {
    "guild_id": GUILD,
    "chat_pilot_role_id": CHAT_ROLE,
    "chat_pilot_channel_ids": CHAT_CHANNEL,
    "chat_pilot_category_ids": CHAT_CATEGORY,
    "watched_channel_ids": f"{PARTY_STAR},{PARTY_KALOS}",
    "post_channel_id": None,
    "chat_pilot_model": "synthetic-chat",
    "model_context_tokens": 16384,
    "chat_pilot_history_ttl_s": 2700.0,
    "chat_pilot_timeout": 60.0,
    "chat_pilot_temperature": 0.7,
    "chat_pilot_think": "low",
}

FIXED_STAR = "f1f1f1f1-0000-4000-8000-000000000001"
FIXED_KALOS = "f2f2f2f2-0000-4000-8000-000000000002"
FIXED_BALD = "f3f3f3f3-0000-4000-8000-000000000003"
RUN_STAR = "a1a1a1a1-0000-4000-8000-000000000001"
RUN_KALOS = "b2b2b2b2-0000-4000-8000-000000000002"
RUN_CARL = "c3c3c3c3-0000-4000-8000-000000000003"
RUN_LIMBO = "d4d4d4d4-0000-4000-8000-000000000004"
RUN_BALD = "e5e5e5e5-0000-4000-8000-000000000005"

WORLD: dict[str, Any] = {
    "members": [
        {"user_id": "11", "display_name": "Alvin tan", "nickname": None, "has_role": True},
        {"user_id": "22", "display_name": "kanon [AZUR]", "nickname": "kanon", "has_role": True},
        {"user_id": "33", "display_name": "Priya", "nickname": None, "has_role": True},
        {"user_id": "44", "display_name": "Mei", "nickname": None, "has_role": True},
        {"user_id": "99", "display_name": "NotABosser", "nickname": None, "has_role": False},
    ],
    "fixed": [
        {
            "id": FIXED_STAR,
            "owner_id": "11",
            "bosses": ["HMaleficStar", "HFA"],
            "weekday": 2,
            "time": "21:30",
            "participants": ["11", "22"],
            "channel_id": PARTY_STAR,
        },
        {
            "id": FIXED_KALOS,
            "owner_id": "22",
            "bosses": ["XKalos"],
            "weekday": 1,
            "time": "23:00",
            "participants": ["22", "33"],
            "channel_id": PARTY_KALOS,
        },
        {
            "id": FIXED_BALD,
            "owner_id": "44",
            "bosses": ["HBaldrix"],
            "weekday": 4,
            "time": "21:00",
            "participants": ["11"],
            "channel_id": CHAT_CHANNEL,
        },
    ],
    "runs": [
        {
            "id": RUN_STAR,
            "week_start": WEEK1,
            "bosses": ["HMaleficStar", "HFA"],
            "at": "2026-09-09T21:30:00+08:00",
            "participants": ["11", "22"],
            "status": "planned",
            "fixed_run_id": FIXED_STAR,
            "channel_id": PARTY_STAR,
        },
        {
            "id": RUN_KALOS,
            "week_start": WEEK1,
            "bosses": ["XKalos"],
            "at": "2026-09-08T23:00:00+08:00",
            "participants": ["22", "33"],
            "status": "planned",
            "fixed_run_id": FIXED_KALOS,
            "channel_id": PARTY_KALOS,
        },
        {
            "id": RUN_CARL,
            "week_start": WEEK2,
            "bosses": ["HCarling"],
            "at": "2026-09-12T20:00:00+08:00",
            "participants": ["33", "44"],
            "status": "confirmed",
            "fixed_run_id": None,
            "channel_id": ADOPTED,
        },
        {
            "id": RUN_LIMBO,
            "week_start": WEEK1,
            "bosses": ["NLimbo"],
            "at": "2026-09-09T22:00:00+08:00",
            "participants": ["11", "44"],
            "status": "cancelled",
            "fixed_run_id": None,
            "channel_id": PARTY_STAR,
        },
        {
            "id": RUN_BALD,
            "week_start": WEEK2,
            "bosses": ["HBaldrix"],
            "at": "2026-09-11T21:00:00+08:00",
            "participants": ["11"],
            "status": "planned",
            "fixed_run_id": FIXED_BALD,
            "channel_id": CHAT_CHANNEL,
        },
    ],
    "rsvps": [
        {"run_id": RUN_STAR, "user_id": "11", "state": "yes"},
        {"run_id": RUN_CARL, "user_id": "33", "state": "no"},
    ],
}

#: Boss shorts the stand-in strategy store has a guide for.
GUIDES = ["MaleficStar", "Kalos"]

#: Distinct eight-hex prefixes, so every generated row has its own short ID.
UUIDS = [f"a{n:07d}-0000-4000-8009-{n:012d}" for n in range(1, 41)]


def base_context() -> dict[str, Any]:
    """The context every stateful chat case starts from (a fresh copy)."""
    import copy

    return copy.deepcopy(
        {
            "timezone": TIMEZONE,
            "reset_weekday": 3,
            "reset_time": "00:00",
            "clock": CLOCK,
            "catalog": CATALOG,
            "settings": SETTINGS,
            "channels": CHANNELS,
            "bot_user": {"id": BOT_USER, "name": "Kanade"},
            "self_role_id": SELF_ROLE,
            "guides": GUIDES,
            "world": WORLD,
            "uuid_sequence": UUIDS,
        }
    )


# --- schema definitions -------------------------------------------------------------

ID = {"type": "string", "pattern": "^[0-9]+$"}
NID = nullable(ID)
UUID = {"type": "string", "format": "uuid"}
CHANNEL_DEF = closed({"id": ID, "name": STR, "category_id": NID, "parent_id": NID})
SETTINGS_DEF = closed(
    {
        "guild_id": ID,
        "chat_pilot_role_id": NID,
        "chat_pilot_channel_ids": STR,
        "chat_pilot_category_ids": STR,
        "watched_channel_ids": STR,
        "post_channel_id": NID,
        "chat_pilot_model": STR,
        "model_context_tokens": INT,
        "chat_pilot_history_ttl_s": {"type": "number"},
        "chat_pilot_timeout": {"type": "number"},
        "chat_pilot_temperature": {"type": "number"},
        "chat_pilot_think": STR,
    }
)
WORLD_DEF = closed(
    {
        "members": arr(
            closed({"user_id": ID, "display_name": STR, "nickname": NSTR, "has_role": BOOL})
        ),
        "fixed": arr(
            closed(
                {
                    "id": UUID,
                    "owner_id": ID,
                    "bosses": STRS,
                    "weekday": {"type": "integer", "minimum": 0, "maximum": 6},
                    "time": STR,
                    "participants": STRS,
                    "channel_id": NID,
                }
            )
        ),
        "runs": arr(
            closed(
                {
                    "id": UUID,
                    "week_start": INSTANT,
                    "bosses": STRS,
                    "at": INSTANT,
                    "participants": STRS,
                    "status": {"enum": RUN_STATUSES},
                    "fixed_run_id": nullable(UUID),
                    "channel_id": NID,
                }
            )
        ),
        "rsvps": arr(closed({"run_id": UUID, "user_id": ID, "state": {"enum": ["yes", "no"]}})),
    }
)

#: Context fields of every family replayed against a seeded host.
HOST_CONTEXT: dict[str, Any] = {
    "timezone": STR,
    "reset_weekday": {"type": "integer", "minimum": 0, "maximum": 6},
    "reset_time": STR,
    "clock": INSTANT,
    "catalog": {"$ref": "#/$defs/catalog"},
    "settings": {"$ref": "#/$defs/settings"},
    "channels": arr({"$ref": "#/$defs/channel"}),
    "bot_user": closed({"id": ID, "name": STR}),
    "self_role_id": NID,
    "guides": STRS,
    "world": {"$ref": "#/$defs/world"},
    "uuid_sequence": arr(UUID),
}
HOST_DEFS: dict[str, Any] = {
    "catalog": CATALOG_DEF,
    "settings": SETTINGS_DEF,
    "channel": CHANNEL_DEF,
    "world": WORLD_DEF,
}

#: A trusted tool context as ``ChatPilot._answer`` builds one from the message.
TOOL_CONTEXT: dict[str, Any] = {
    "author_id": ID,
    "channel_id": ID,
    "is_admin": BOOL,
    "read_only": BOOL,
    "force_all_channels": BOOL,
    "force_channel_scope": BOOL,
    "force_group_schedule": BOOL,
    "upcoming_only": BOOL,
}
TOOL_CONTEXT_OPTIONAL = tuple(key for key in TOOL_CONTEXT if key not in ("author_id", "channel_id"))

OUTCOME_DEF = closed(
    {
        "name": STR,
        "output": STR,
        "arguments": {"type": "object"},
        "ok": BOOL,
        "error": nullable({"enum": ["refused", "unknown tool", "failed"]}),
        "created": arr(UUID),
        "posted": arr(UUID),
    }
)
