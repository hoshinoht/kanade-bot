"""The single authority boundary for cards about existing schedule objects."""

from __future__ import annotations

from typing import Any

from bot.domain.ids import short_id

from .. import gate
from .contracts import ToolContext, ToolError
from .rendering import channel_reference

#: Refusing a change to somebody else's run. It names the run because the asker
#: already named it, and nothing else: who is on it is not this refusal's to say.
NOT_THEIRS_RUN = (
    "They are not on run {sid} and do not own the weekly timing behind it, so it is not "
    "theirs to change. Say that only the people on a run -- or the owner of the weekly "
    "timing it comes from -- can propose a change to it, and that putting somebody on a "
    "run is not something you can do. Do not name anybody on it."
)

#: The same rule for a weekly timing, which is owned as well as attended.
NOT_THEIRS_FIXED = (
    "They are not on the weekly timing {sid} and do not own it, so it is not theirs to "
    "remove. Say that only the people on it, or whoever owns it, can propose that. Do "
    "not name anybody on it."
)

#: Refusing a change to a run that belongs to a different channel. The channel
#: name is the *only* thing this may leak: it is what makes the refusal
#: actionable ("ask there"), and the bot answers in that channel too.
ELSEWHERE = (
    "That {noun} lives in {where}, and changes to it are proposed from its own channel. "
    "Tell them which channel it lives in and to ask there. Say nothing else about it."
)


def _fixed_owner(bot: Any, run: dict) -> str | None:
    """Who owns the weekly timing this run was materialised from, if any."""
    fixed_id = run.get("fixed_run_id")
    if not fixed_id:
        return None
    fixed = bot.repo.get_fixed_run(str(fixed_id))
    return str(fixed["owner_id"]) if fixed else None


def _pilot_channel(bot: Any, channel_id: str) -> bool:
    """Whether this is a channel the pilot answers questions in."""
    try:
        channel = bot.get_channel(int(channel_id))
    except (TypeError, ValueError):  # pragma: no cover - defensive
        return False
    return gate.is_chat_channel(channel, bot.settings)


def require_authority(
    ctx: ToolContext, *, run: dict | None = None, fixed: dict | None = None
) -> None:
    """Refuse cross-party and cross-chat-channel cards unless the asker is an admin."""
    if ctx.is_admin:
        return
    subject = run if run is not None else fixed
    if subject is None:  # pragma: no cover - every caller names one
        return
    owner = _fixed_owner(ctx.bot, run) if run is not None else str(fixed["owner_id"])
    if ctx.author_id not in [str(p) for p in subject["participants"]] and ctx.author_id != owner:
        sid = short_id(subject["id"])
        raise ToolError(
            NOT_THEIRS_RUN.format(sid=sid) if run is not None else NOT_THEIRS_FIXED.format(sid=sid)
        )
    home = str(subject["channel_id"] or "")
    if home and home != str(ctx.channel_id) and _pilot_channel(ctx.bot, home):
        raise ToolError(
            ELSEWHERE.format(
                noun="run" if run is not None else "weekly timing",
                where=channel_reference(ctx.bot, home) or "another channel",
            )
        )
