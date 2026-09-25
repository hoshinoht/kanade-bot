"""Who the pilot answers: ``bot.chat.gate`` over synthetic messages and two budgets.

A case holds one pair of limiters (per-person and guild pool) whose monotonic
clock each ``decide`` step sets, so budget spending is observable across steps.
Message descriptors carry only Discord's *resolved* lists; text never grants a
mention or a role.
"""

from __future__ import annotations

import copy
from types import SimpleNamespace
from typing import Any

from bot.agent.util import is_bot_admin
from bot.chat import agent, gate
from bot.chat.ratelimit import RateLimiter

from .. import fixtures, host
from ..contract import (
    BOOL,
    INT,
    NUM,
    STR,
    Family,
    Op,
    arr,
    closed,
    nullable,
    ref,
    replay_steps,
)

ID, NID = fixtures.ID, fixtures.NID
MESSAGE = closed(
    {
        "author": nullable(closed({"id": ID, "bot": BOOL, "roles": arr(ID)})),
        "guild_id": NID,
        "channel_id": NID,
        "mentions": arr(ID),
        "role_mentions": arr(ID),
    }
)
DECISION = closed({"act": BOOL, "reason": STR, "busy": BOOL, "retry_after_s": NUM})
LIMITS = closed({"count": INT, "window_s": NUM, "pool_count": INT, "pool_window_s": NUM})


class Clock:
    def __init__(self) -> None:
        self.now = 0.0

    def __call__(self) -> float:
        return self.now


def _channel(channels: dict[str, dict], channel_id: str | None) -> Any:
    if channel_id is None:
        return None
    spec = channels.get(channel_id, {"id": channel_id, "category_id": None, "parent_id": None})
    parent = _channel(channels, spec["parent_id"]) if spec["parent_id"] else None
    return SimpleNamespace(
        id=int(spec["id"]),
        category_id=int(spec["category_id"]) if spec["category_id"] else None,
        parent=parent,
    )


def _message(channels: dict[str, dict], raw: dict[str, Any]) -> Any:
    author = raw["author"]
    return SimpleNamespace(
        author=SimpleNamespace(
            id=int(author["id"]),
            bot=author["bot"],
            roles=[SimpleNamespace(id=int(role)) for role in author["roles"]],
        )
        if author is not None
        else None,
        guild=SimpleNamespace(id=int(raw["guild_id"])) if raw["guild_id"] is not None else None,
        channel=_channel(channels, raw["channel_id"]),
        mentions=[SimpleNamespace(id=int(uid)) for uid in raw["mentions"]],
        role_mentions=[SimpleNamespace(id=int(rid)) for rid in raw["role_mentions"]],
    )


def _decision(value: gate.ChatDecision) -> dict[str, Any]:
    return {
        "act": value.act,
        "reason": value.reason,
        "busy": value.busy,
        "retry_after_s": value.retry_after_s,
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    input_ = case["input"]
    settings = host.settings(input_)
    channels = {spec["id"]: spec for spec in input_["channels"]}
    clock = Clock()
    limits = input_["limits"]
    limiter = RateLimiter(limits["count"], limits["window_s"], clock=clock)
    pool = RateLimiter(limits["pool_count"], limits["pool_window_s"], clock=clock)
    bot_user_id = input_["bot_user_id"]
    self_role_id = input_["self_role_id"]

    def decide(s: dict) -> dict[str, Any]:
        clock.now = s["now"]
        return _decision(
            gate.decide(
                _message(channels, s["message"]),
                settings,
                bot_user_id=bot_user_id,
                enabled=s["enabled"],
                is_admin=s["is_admin"],
                limiter=limiter,
                global_limiter=pool,
                self_role_id=self_role_id,
                replied_author_id=s["replied_author_id"],
            )
        )

    def access(s: dict) -> dict[str, Any]:
        return _decision(
            gate.access_decide(
                _message(channels, s["message"]),
                settings,
                bot_user_id=bot_user_id,
                enabled=s["enabled"],
                is_admin=s["is_admin"],
                self_role_id=self_role_id,
                replied_author_id=s["replied_author_id"],
            )
        )

    return replay_steps(
        FAMILY,
        case,
        {
            "decide": decide,
            "access_decide": access,
            "would_check_mention": lambda s: gate.would_check_mention(
                _message(channels, s["message"]),
                settings,
                bot_user_id=bot_user_id,
                enabled=s["enabled"],
            ),
            "mentions_bot": lambda s: gate.mentions_bot(
                _message(channels, s["message"]),
                s["bot_user_id"],
                s["self_role_id"],
                s["replied_author_id"],
            ),
            "is_chat_channel": lambda s: gate.is_chat_channel(
                _channel(channels, s["channel_id"]), settings
            ),
            "is_bot_admin": lambda s: is_bot_admin(
                s["administrator"],
                s["owner"],
                [int(role) for role in s["roles"]],
                int(s["admin_role_id"]) if s["admin_role_id"] else None,
            ),
            "retry_note": lambda s: agent.retry_note(s["seconds"]),
        },
    )


# --- cases --------------------------------------------------------------------------

MEMBER, OTHER = "11", "22"


def msg(
    author: str | None = MEMBER,
    *,
    roles: tuple[str, ...] = (fixtures.CHAT_ROLE,),
    bot: bool = False,
    guild: str | None = fixtures.GUILD,
    channel: str | None = fixtures.CHAT_CHANNEL,
    mentions: tuple[str, ...] = (fixtures.BOT_USER,),
    role_mentions: tuple[str, ...] = (),
) -> dict[str, Any]:
    return {
        "author": {"id": author, "bot": bot, "roles": list(roles)} if author else None,
        "guild_id": guild,
        "channel_id": channel,
        "mentions": list(mentions),
        "role_mentions": list(role_mentions),
    }


def decide(
    message: dict[str, Any],
    now: float = 0.0,
    *,
    is_admin: bool = False,
    enabled: bool = True,
    replied: str | None = None,
) -> dict[str, Any]:
    return {
        "op": "decide",
        "message": message,
        "enabled": enabled,
        "is_admin": is_admin,
        "replied_author_id": replied,
        "now": now,
    }


def access(message: dict[str, Any], *, is_admin: bool = False, replied: str | None = None):
    return {
        "op": "access_decide",
        "message": message,
        "enabled": True,
        "is_admin": is_admin,
        "replied_author_id": replied,
    }


def context(**settings: Any) -> dict[str, Any]:
    return {
        "timezone": fixtures.TIMEZONE,
        "reset_weekday": 3,
        "reset_time": "00:00",
        "settings": {**copy.deepcopy(fixtures.SETTINGS), **settings},
        "channels": copy.deepcopy(fixtures.CHANNELS),
        "bot_user_id": fixtures.BOT_USER,
        "self_role_id": fixtures.SELF_ROLE,
        "limits": {"count": 2, "window_s": 300.0, "pool_count": 3, "pool_window_s": 900.0},
    }


def case(case_id: str, steps: list[dict[str, Any]], **settings: Any) -> dict[str, Any]:
    return {"case_id": case_id, "input": {**context(**settings), "steps": steps}}


def cases() -> list[dict[str, Any]]:
    bot, role = fixtures.BOT_USER, fixtures.SELF_ROLE
    return [
        case(
            "refusal-order",
            [
                access(msg()),
                access(msg(bot=True)),
                access(msg(author=bot)),
                access(msg(author=None)),
                access(msg(guild=None)),
                access(msg(guild="1001")),
                access(msg(channel=fixtures.GENERAL)),
                access(msg(channel=fixtures.PARTY_STAR)),
                access(msg(mentions=())),
                access(msg(mentions=(OTHER,))),
                access(msg(roles=())),
                access(msg(roles=(fixtures.OTHER_ROLE,))),
                access(msg(roles=(), mentions=()), is_admin=True),
                access(msg(roles=()), is_admin=True),
                access(msg(roles=(), channel=fixtures.GENERAL), is_admin=True),
                access(msg(mentions=(), guild="1001")),
                {**decide(msg(), enabled=False)},
            ],
        ),
        case(
            "channels-categories-threads",
            [
                {"op": "is_chat_channel", "channel_id": fixtures.CHAT_CHANNEL},
                {"op": "is_chat_channel", "channel_id": fixtures.ADOPTED},
                {"op": "is_chat_channel", "channel_id": fixtures.THREAD},
                {"op": "is_chat_channel", "channel_id": fixtures.ORPHAN},
                {"op": "is_chat_channel", "channel_id": fixtures.GENERAL},
                {"op": "is_chat_channel", "channel_id": fixtures.PARTY_STAR},
                {"op": "is_chat_channel", "channel_id": "799"},
                {"op": "is_chat_channel", "channel_id": None},
                access(msg(channel=fixtures.ADOPTED)),
                access(msg(channel=fixtures.THREAD)),
                access(msg(channel=fixtures.ORPHAN)),
            ],
        ),
        case(
            "mentions-replies-managed-role",
            [
                {
                    "op": "mentions_bot",
                    "message": msg(mentions=()),
                    "bot_user_id": bot,
                    "self_role_id": role,
                    "replied_author_id": None,
                },
                {
                    "op": "mentions_bot",
                    "message": msg(mentions=(), role_mentions=(role,)),
                    "bot_user_id": bot,
                    "self_role_id": role,
                    "replied_author_id": None,
                },
                {
                    "op": "mentions_bot",
                    "message": msg(mentions=(), role_mentions=(fixtures.CHAT_ROLE,)),
                    "bot_user_id": bot,
                    "self_role_id": role,
                    "replied_author_id": None,
                },
                {
                    "op": "mentions_bot",
                    "message": msg(mentions=(), role_mentions=(role,)),
                    "bot_user_id": bot,
                    "self_role_id": None,
                    "replied_author_id": None,
                },
                {
                    "op": "mentions_bot",
                    "message": msg(mentions=()),
                    "bot_user_id": bot,
                    "self_role_id": role,
                    "replied_author_id": bot,
                },
                {
                    "op": "mentions_bot",
                    "message": msg(mentions=()),
                    "bot_user_id": bot,
                    "self_role_id": role,
                    "replied_author_id": OTHER,
                },
                {
                    "op": "mentions_bot",
                    "message": msg(),
                    "bot_user_id": None,
                    "self_role_id": role,
                    "replied_author_id": None,
                },
                access(msg(mentions=()), replied=bot),
                access(msg(mentions=()), replied=OTHER),
                access(msg(mentions=(), role_mentions=(role,))),
                {"op": "would_check_mention", "message": msg(mentions=()), "enabled": True},
                {
                    "op": "would_check_mention",
                    "message": msg(mentions=(), channel=fixtures.PARTY_STAR),
                    "enabled": True,
                },
                {"op": "would_check_mention", "message": msg(), "enabled": False},
                {"op": "would_check_mention", "message": msg(roles=()), "enabled": True},
            ],
        ),
        case(
            "budgets-person-then-pool",
            [
                decide(msg(), 0.0),
                decide(msg(), 10.0),
                decide(msg(), 20.0),
                decide(msg(mentions=()), 25.0),
                decide(msg(roles=()), 26.0),
                decide(msg(roles=()), 27.0, is_admin=True),
                decide(msg(author=OTHER), 30.0),
                decide(msg(author="33"), 40.0),
                decide(msg(author="33"), 50.0),
                decide(msg(), 300.0),
                decide(msg(), 900.0),
                decide(msg(author="33"), 910.0),
                decide(msg(author="33"), 920.0),
            ],
        ),
        case(
            "half-configured-pilot",
            [access(msg()), {"op": "is_chat_channel", "channel_id": fixtures.CHAT_CHANNEL}],
            chat_pilot_role_id=None,
        ),
        case(
            "category-alone-configures",
            [
                access(msg(channel=fixtures.ADOPTED)),
                access(msg(channel=fixtures.CHAT_CHANNEL)),
            ],
            chat_pilot_channel_ids="",
        ),
        case(
            "admin-rule-and-retry-note",
            [
                {
                    "op": "is_bot_admin",
                    "administrator": False,
                    "owner": False,
                    "roles": [fixtures.CHAT_ROLE],
                    "admin_role_id": "6002",
                },
                {
                    "op": "is_bot_admin",
                    "administrator": False,
                    "owner": False,
                    "roles": ["6002"],
                    "admin_role_id": "6002",
                },
                {
                    "op": "is_bot_admin",
                    "administrator": True,
                    "owner": False,
                    "roles": [],
                    "admin_role_id": None,
                },
                {
                    "op": "is_bot_admin",
                    "administrator": False,
                    "owner": True,
                    "roles": [],
                    "admin_role_id": None,
                },
                *(
                    {"op": "retry_note", "seconds": seconds}
                    for seconds in (0.0, 0.2, 1.0, 59.5, 120.0, 120.4, 121.0, 290.0, 3600.0)
                ),
            ],
        ),
    ]


def _message_step(extra: dict[str, Any]) -> dict[str, Any]:
    return {"message": ref("message"), **extra}


FAMILY = Family(
    name="gate",
    version="v5-chat-gate-v1",
    provenance={
        "oracle": "legacy/python bot.chat.gate",
        "functions": [
            "bot.chat.gate.decide",
            "bot.chat.gate.access_decide",
            "bot.chat.gate.would_check_mention",
            "bot.chat.gate.mentions_bot",
            "bot.chat.gate.is_chat_channel",
            "bot.chat.ratelimit.RateLimiter",
            "bot.agent.util.is_bot_admin",
            "bot.chat.agent.retry_note",
            "bot.infrastructure.watch.is_watched",
        ],
        "source_tests": [
            "tests/test_chat_gate.py",
            "tests/test_chat_mentions.py",
            "tests/test_chat_ratelimit.py",
            "tests/test_chat_injection.py",
        ],
        "inventory_surfaces": [],
    },
    context={
        "timezone": STR,
        "reset_weekday": INT,
        "reset_time": STR,
        "settings": ref("settings"),
        "channels": arr(ref("channel")),
        "bot_user_id": ID,
        "self_role_id": NID,
        "limits": LIMITS,
    },
    defs={
        "settings": fixtures.SETTINGS_DEF,
        "channel": fixtures.CHANNEL_DEF,
        "message": MESSAGE,
    },
    ops={
        "decide": Op(
            _message_step(
                {"enabled": BOOL, "is_admin": BOOL, "replied_author_id": NID, "now": NUM}
            ),
            DECISION,
        ),
        "access_decide": Op(
            _message_step({"enabled": BOOL, "is_admin": BOOL, "replied_author_id": NID}),
            DECISION,
        ),
        "would_check_mention": Op(_message_step({"enabled": BOOL}), BOOL),
        "mentions_bot": Op(
            _message_step({"bot_user_id": NID, "self_role_id": NID, "replied_author_id": NID}),
            BOOL,
        ),
        "is_chat_channel": Op({"channel_id": NID}, BOOL),
        "is_bot_admin": Op(
            {"administrator": BOOL, "owner": BOOL, "roles": arr(ID), "admin_role_id": NID}, BOOL
        ),
        "retry_note": Op({"seconds": NUM}, STR),
    },
    cases=cases,
    replay=replay,
)
