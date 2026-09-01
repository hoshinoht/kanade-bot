"""Should the bot answer this message at all?

One pure function, because every one of these checks is a security boundary and
a boundary that can only be exercised through a live gateway is a boundary
nobody tests.  :func:`decide` takes the message, the settings and the limiter
and returns a decision with a reason; :class:`bot.chat.agent.ChatPilot` does
what it says.

The order matters and is the order below: the cheapest, least revealing checks
come first, so a message from another guild never reaches the roster lookup, and
somebody without the role never costs a rate-limit slot.

**Silence is the default.** Every refusal except a rate limit produces no reply
and no reaction -- a bot that says "you may not use me" in a channel is a bot
that can be used to spam a channel by anyone who cannot use it.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from ..config import Settings
from ..watch import is_watched

#: Put on a message the bot is deliberately not answering *this time* -- the
#: only refusal anybody is told about, because the person is allowed to be here
#: and would otherwise be left waiting for an answer that is never coming.
BUSY_REACTION = "⏳"

__all__ = ["BUSY_REACTION", "ChatDecision", "decide", "mentions_bot"]


@dataclass(frozen=True)
class ChatDecision:
    """Answer or not, and why -- the reason is for the log, never for Discord."""

    act: bool
    reason: str
    #: React :data:`BUSY_REACTION` and drop. Never set together with ``act``.
    busy: bool = False

    def __bool__(self) -> bool:  # pragma: no cover - clarity at call sites
        return self.act


def _role_ids(user: Any) -> list[int]:
    """The role ids on a live member object, or none for anything else.

    A ``discord.User`` (a DM, a webhook, an uncached author) has no ``roles`` at
    all, and "no roles" is the right reading: the role gate must fail closed.
    """
    out: list[int] = []
    for role in getattr(user, "roles", None) or ():
        try:
            out.append(int(getattr(role, "id", role)))
        except (TypeError, ValueError):
            continue
    return out


def mentions_bot(message: Any, bot_user_id: int | str | None) -> bool:
    """Was the bot actually mentioned, or replied to?

    Read from ``message.mentions`` -- Discord's own resolved list -- rather than
    from the message text, so writing ``<@000000000000000000>`` or the bot's
    name into a message is text and nothing more. A reply to one of the bot's
    own messages counts, because that is how a conversation continues and
    Discord attaches the mention for it anyway; it is checked separately so a
    reply still works when the replier turned the ping off.

    ``@everyone``, ``@here`` and role mentions deliberately do not count. The bot
    holds roles, and a channel-wide ping must not summon it.
    """
    if bot_user_id is None:
        return False
    wanted = str(bot_user_id)
    for user in getattr(message, "mentions", None) or ():
        if str(getattr(user, "id", user)) == wanted:
            return True
    reference = getattr(message, "reference", None)
    replied = getattr(reference, "resolved", None) if reference is not None else None
    author = getattr(replied, "author", None)
    return author is not None and str(getattr(author, "id", "")) == wanted


def decide(
    message: Any,
    settings: Settings,
    *,
    bot_user_id: int | str | None,
    enabled: bool = True,
    is_admin: bool = False,
    limiter: Any | None = None,
) -> ChatDecision:
    """Whether to answer ``message``.

    ``enabled`` is the runtime kill switch (``chat_mode``), passed in rather than
    read from the bot so this stays a function of its arguments. ``limiter`` is
    consulted last and only when everything else passed, so it records an
    allowance exactly when one is about to be spent.
    """
    author = getattr(message, "author", None)
    if author is None or getattr(author, "bot", False):
        return ChatDecision(False, "the author is a bot")
    if bot_user_id is not None and str(getattr(author, "id", "")) == str(bot_user_id):
        return ChatDecision(False, "the bot's own message")

    guild = getattr(message, "guild", None)
    if guild is None:
        return ChatDecision(False, "not a guild message")
    if int(getattr(guild, "id", 0)) != int(settings.guild_id):
        return ChatDecision(False, "another guild")

    if not enabled:
        return ChatDecision(False, "chat_mode is off")
    if not settings.chat_pilot_configured:
        return ChatDecision(False, "the chat pilot is not configured")

    # The pilot's own allow-list: its own channels and its own categories, read
    # from `CHAT_PILOT_*` and never from the extractor's `CHAT_*` watch list. The
    # two are independent rather than mutually exclusive -- pointing the pilot's
    # category list at the bossing category *does* make the bot answer in every
    # party channel under it, which is a deliberate choice for whoever writes
    # `.env`, not something this gate second-guesses.
    #
    # `is_watched` is reused rather than reimplemented so both features resolve a
    # category and a thread identically: a channel under an allowed category is
    # allowed (including one added to that category later), and a thread counts
    # as its parent channel.
    if not is_watched(
        getattr(message, "channel", None),
        settings.chat_pilot_channel_id_list,
        settings.chat_pilot_category_id_list,
    ):
        return ChatDecision(False, "not a chat channel")

    if not mentions_bot(message, bot_user_id):
        return ChatDecision(False, "the bot was not mentioned")

    role_id = settings.chat_pilot_role_id
    if role_id is None or int(role_id) not in _role_ids(author):
        # Silent on purpose: the person may not even know the bot is here, and
        # telling them would make the bot a way to get a reply out of it.
        return ChatDecision(False, "the author does not hold the chat role")

    if limiter is not None and not limiter.allow(getattr(author, "id", author), exempt=is_admin):
        return ChatDecision(False, "rate limited", busy=True)

    return ChatDecision(True, "ok")
