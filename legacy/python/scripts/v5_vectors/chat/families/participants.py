"""Model-supplied parties and bosses, re-validated against trusted runtime state.

``validate_participants`` defaults to the asker, substitutes first-person
words, strips the bot's own mention/ID/name, forgives joining words, and
refuses strangers, ambiguity, non-bossers and invented snowflakes.
``new_party`` is the weekly-timing variant (``None`` keeps the party).
"""

from __future__ import annotations

from typing import Any

from bot.chat.tools import ToolContext, ToolError
from bot.chat.tools.participants import is_true, new_party, validate_bosses, validate_participants

from .. import fixtures, host
from ..contract import ANY, BOOL, STRS, Family, Op, nullable, run_steps, validate_input

ERRORS: dict[str, type[Exception]] = {"ToolError": ToolError}
TEXT = {
    "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}, {"type": "null"}]
}


def replay(case: dict[str, Any]) -> dict[str, Any]:
    validate_input(FAMILY, case)
    with host.session(case) as s:

        def ctx(step: dict) -> ToolContext:
            return ToolContext(
                bot=s.host,
                author_id=step.get("author_id", "11"),
                channel_id=fixtures.CHAT_CHANNEL,
                message_id="990001",
            )

        return run_steps(
            case,
            {
                "validate_participants": lambda st: validate_participants(ctx(st), st["text"]),
                "new_party": lambda st: new_party(ctx(st), st["value"]),
                "validate_bosses": lambda st: validate_bosses(ctx(st), st["text"]),
                "is_true": lambda st: is_true(st["value"]),
            },
            ERRORS,
        )


def people(text: Any, author: str = "11", refused: bool = False) -> dict[str, Any]:
    step = {"op": "validate_participants", "author_id": author, "text": text}
    return {**step, "error_type": "ToolError"} if refused else step


def cases() -> list[dict[str, Any]]:
    bot = fixtures.BOT_USER

    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {"case_id": case_id, "input": {**fixtures.base_context(), "steps": steps}}

    return [
        case(
            "defaults-and-first-person",
            [
                people(""),
                people(None),
                people([]),
                people("me"),
                people("Me"),
                people("myself"),
                people("I"),
                people("me and kanon"),
                people("add me"),
                people("me, kanon & Priya please"),
                people(["me", "kanon"]),
                people("kanon"),
                people("<@22>"),
                people("<@!33>"),
                people("33", refused=True),
                people("Mei", author="44"),
                people("kanon kanon"),
            ],
        ),
        case(
            "the-bot-is-never-a-member",
            [
                people(f"<@{bot}>"),
                people(f"<@!{bot}>"),
                people(bot),
                people("Kanade"),
                people("kanade"),
                people(f"<@{bot}> and kanon"),
                people(f"<@{bot}> me"),
                people("Kanade and Priya"),
                people("Kanadeko", refused=True),
            ],
        ),
        case(
            "refusals-and-forgiven-joining-words",
            [
                people("Zed", refused=True),
                people("kanon and Zed", refused=True),
                people("<@12345>", refused=True),
                people("NotABosser", refused=True),
                people("99", refused=True),
                people("a"),
                people("and", author="22"),
            ],
        ),
        case(
            "weekly-party-replacement",
            [
                {"op": "new_party", "author_id": "11", "value": ""},
                {"op": "new_party", "author_id": "11", "value": None},
                {"op": "new_party", "author_id": "11", "value": f"<@{bot}>"},
                {"op": "new_party", "author_id": "11", "value": "Kanade"},
                {"op": "new_party", "author_id": "11", "value": "me and Mei"},
                {"op": "new_party", "author_id": "11", "value": ["Priya", "Mei"]},
                {
                    "op": "new_party",
                    "author_id": "11",
                    "value": "Zed",
                    "error_type": "ToolError",
                },
            ],
        ),
        case(
            "bosses",
            [
                {"op": "validate_bosses", "text": "hstar"},
                {"op": "validate_bosses", "text": "HMaleficStar, HFA"},
                {"op": "validate_bosses", "text": "xkalos + hcarl"},
                {"op": "validate_bosses", "text": "", "error_type": "ToolError"},
                {"op": "validate_bosses", "text": "   ", "error_type": "ToolError"},
                {"op": "validate_bosses", "text": "star", "error_type": "ToolError"},
                {"op": "validate_bosses", "text": "hkalos", "error_type": "ToolError"},
                {"op": "validate_bosses", "text": "Lotus", "error_type": "ToolError"},
            ],
        ),
        case(
            "tolerant-weekly-boolean",
            [
                {"op": "is_true", "value": value}
                for value in (
                    True,
                    False,
                    None,
                    1,
                    0,
                    0.0,
                    "true",
                    "false",
                    "Yes",
                    " y ",
                    "1",
                    "0",
                    "weekly",
                    "Recurring",
                    "fixed",
                    "no",
                    "",
                    ["x"],
                )
            ],
        ),
    ]


FAMILY = Family(
    name="participants",
    version="v5-chat-participants-v1",
    provenance={
        "oracle": "legacy/python bot.chat.tools.participants",
        "functions": [
            "bot.chat.tools.participants.validate_participants",
            "bot.chat.tools.participants.new_party",
            "bot.chat.tools.participants.validate_bosses",
            "bot.chat.tools.participants.is_true",
            "bot.agent.util.resolve_participant_text",
            "bot.api.service.validate_participants",
            "bot.api.service.validate_bosses",
        ],
        "source_tests": [
            "tests/test_chat_participants.py",
            "tests/test_chat_tools.py",
            "tests/test_chat_weekly.py",
        ],
        "inventory_surfaces": [],
    },
    context=fixtures.HOST_CONTEXT,
    defs=fixtures.HOST_DEFS,
    ops={
        "validate_participants": Op(
            {"author_id": fixtures.ID, "text": TEXT}, STRS, errors=("ToolError",)
        ),
        "new_party": Op(
            {"author_id": fixtures.ID, "value": TEXT}, nullable(STRS), errors=("ToolError",)
        ),
        "validate_bosses": Op({"text": {"type": "string"}}, STRS, errors=("ToolError",)),
        "is_true": Op({"value": ANY}, BOOL),
    },
    cases=cases,
    replay=replay,
)
