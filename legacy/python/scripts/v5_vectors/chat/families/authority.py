"""Who may raise a card about an existing run or weekly timing: ``require_authority``.

Replayed against the shared seeded world: party membership, weekly-timing
ownership (directly and through a materialised run), the home-channel rule
that only applies when that home is itself a pilot channel, and the admin
exemption. A refusal is the model-readable ``ToolError`` text.
"""

from __future__ import annotations

from typing import Any

from bot.chat.tools import ToolContext, ToolError
from bot.chat.tools.authority import require_authority

from .. import fixtures, host
from ..contract import BOOL, Family, Op, closed, run_steps, validate_input

ALLOWED = closed({"allowed": {"const": True}})
ERRORS: dict[str, type[Exception]] = {"ToolError": ToolError}


def replay(case: dict[str, Any]) -> dict[str, Any]:
    validate_input(FAMILY, case)
    with host.session(case) as s:

        def check(step: dict) -> dict[str, Any]:
            ctx = ToolContext(
                bot=s.host,
                author_id=step["author_id"],
                channel_id=step["channel_id"],
                message_id="990001",
                is_admin=step["is_admin"],
            )
            if "run_id" in step:
                require_authority(ctx, run=s.repo.get_run(step["run_id"]))
            else:
                require_authority(ctx, fixed=s.repo.get_fixed_run(step["fixed_id"]))
            return {"allowed": True}

        return run_steps(case, {"require_authority": check}, ERRORS)


def _run(run_id: str, author: str, channel: str, admin: bool = False) -> dict[str, Any]:
    step = {
        "op": "require_authority",
        "author_id": author,
        "channel_id": channel,
        "is_admin": admin,
        "run_id": run_id,
    }
    return step


def _fixed(fixed_id: str, author: str, channel: str, admin: bool = False) -> dict[str, Any]:
    return {
        "op": "require_authority",
        "author_id": author,
        "channel_id": channel,
        "is_admin": admin,
        "fixed_id": fixed_id,
    }


def refused(step: dict[str, Any]) -> dict[str, Any]:
    return {**step, "error_type": "ToolError"}


def cases() -> list[dict[str, Any]]:
    f = fixtures
    chat, adopted, star, general = f.CHAT_CHANNEL, f.ADOPTED, f.PARTY_STAR, f.GENERAL

    def case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
        return {"case_id": case_id, "input": {**f.base_context(), "steps": steps}}

    return [
        case(
            "runs",
            [
                _run(f.RUN_STAR, "11", star),
                _run(f.RUN_STAR, "22", chat),
                refused(_run(f.RUN_STAR, "33", chat)),
                refused(_run(f.RUN_STAR, "99", star)),
                # 44 owns the weekly Baldrix but is not on its run.
                _run(f.RUN_BALD, "44", chat),
                _run(f.RUN_BALD, "11", chat),
                refused(_run(f.RUN_BALD, "22", chat)),
                # A manual run has no weekly owner to fall back on.
                refused(_run(f.RUN_CARL, "11", adopted)),
                _run(f.RUN_STAR, "33", general, admin=True),
            ],
        ),
        case(
            "home-channel",
            [
                # Carling lives in a pilot channel (by category): elsewhere is refused.
                refused(_run(f.RUN_CARL, "33", chat)),
                refused(_run(f.RUN_CARL, "44", f.THREAD)),
                _run(f.RUN_CARL, "33", adopted),
                _run(f.RUN_CARL, "33", chat, admin=True),
                # Baldrix lives in the explicit pilot channel.
                refused(_run(f.RUN_BALD, "11", adopted)),
                # Star lives in a party channel the pilot does not answer in.
                _run(f.RUN_STAR, "11", chat),
                _run(f.RUN_STAR, "22", general),
            ],
        ),
        case(
            "weekly-timings",
            [
                _fixed(f.FIXED_STAR, "11", star),
                _fixed(f.FIXED_STAR, "22", star),
                refused(_fixed(f.FIXED_STAR, "33", chat)),
                _fixed(f.FIXED_BALD, "44", chat),
                _fixed(f.FIXED_BALD, "11", chat),
                refused(_fixed(f.FIXED_BALD, "44", adopted)),
                refused(_fixed(f.FIXED_KALOS, "11", chat)),
                _fixed(f.FIXED_KALOS, "11", general, admin=True),
            ],
        ),
    ]


FAMILY = Family(
    name="authority",
    version="v5-chat-authority-v1",
    provenance={
        "oracle": "legacy/python bot.chat.tools.authority",
        "functions": [
            "bot.chat.tools.authority.require_authority",
            "bot.chat.tools.authority.NOT_THEIRS_RUN",
            "bot.chat.tools.authority.NOT_THEIRS_FIXED",
            "bot.chat.tools.authority.ELSEWHERE",
            "bot.chat.tools.rendering.channel_reference",
        ],
        "source_tests": ["tests/test_chat_authority.py", "tests/test_chat_injection.py"],
        "inventory_surfaces": [],
    },
    context=fixtures.HOST_CONTEXT,
    defs=fixtures.HOST_DEFS,
    ops={
        "require_authority": Op(
            {
                "author_id": fixtures.ID,
                "channel_id": fixtures.ID,
                "is_admin": BOOL,
                "run_id": fixtures.UUID,
                "fixed_id": fixtures.UUID,
            },
            ALLOWED,
            errors=("ToolError",),
            optional=("run_id", "fixed_id"),
        )
    },
    cases=cases,
    replay=replay,
)
