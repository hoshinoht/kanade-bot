"""Should the bot answer this message at all?

One pure function, because every one of these checks is a security boundary and
a boundary that can only be exercised through a live gateway is a boundary
nobody tests.  :func:`decide` takes the message, the settings and the limiter
and returns a decision with a reason; :class:`bot.chat.agent.ChatPilot` does
what it says.

The order matters and is the order below: the cheapest, least revealing checks
come first, so a message from another guild never reaches the roster lookup, and
somebody without the role never costs a rate-limit slot. The two budgets -- the
asker's own window and the guild's shared pool -- come last and together, so a
question refused by one of them is never charged to the other.

**Silence is the default.** Every refusal except a spent budget produces no
reply and no reaction -- a bot that says "you may not use me" in a channel is a
bot that can be used to spam a channel by anyone who cannot use it. A spent
budget is the exception because the person *is* allowed to be here: they get a
⏳, and once per episode a sentence saying when to come back (see
:meth:`bot.chat.agent.ChatPilot._say_limited`, which composes it from a constant
-- a refusal that cost a generation would defeat the limit it is explaining).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from bot.infrastructure.config import Settings
from bot.infrastructure.watch import is_watched

#: The three things the bot ever says with a reaction, and only ever to somebody
#: whose message it accepted or would have. Every other refusal stays silent.
#:
#: * :data:`SEEN_REACTION` -- heard you, thinking. An answer can take 10-30 s of
#:   GPU, which is long enough to look like being ignored.
#: * :data:`RATE_LIMITED_REACTION` -- you have had your answers for now, or the
#:   guild has had its. The first one of an episode is also said in words, with
#:   the wait in it; the rest are the reaction alone.
#: * :data:`CHANNEL_BUSY_REACTION` -- the model is in use, here or elsewhere.
#:
#: The last two are different emoji on purpose: one clears in minutes and one in
#: seconds, and "wait" is useless advice if you cannot tell which wait it is.
SEEN_REACTION = "👀"
RATE_LIMITED_REACTION = "⏳"
CHANNEL_BUSY_REACTION = "💬"

#: The guild-wide answer pool is a :class:`bot.chat.ratelimit.RateLimiter` like
#: the per-person one, so it still needs a key -- everybody shares this one. It
#: lives here rather than beside the limiter that holds it because this is the
#: only place the pool is ever read or spent.
GLOBAL_KEY = "guild"

#: Why the guild-wide pool refused, kept distinct from "rate limited" so a log
#: line says which of the two ran out. Both wear ⏳: the difference matters to
#: whoever reads the logs, not to somebody waiting for the window to roll.
POOL_SPENT = "the guild's answer budget is spent"

__all__ = [
    "CHANNEL_BUSY_REACTION",
    "GLOBAL_KEY",
    "POOL_SPENT",
    "RATE_LIMITED_REACTION",
    "SEEN_REACTION",
    "ChatDecision",
    "decide",
    "is_chat_channel",
    "mentions_bot",
    "would_check_mention",
]


def is_chat_channel(channel: Any, settings: Settings) -> bool:
    """Is this one of the pilot's own channels?

    The pilot's allow-list, read from ``CHAT_PILOT_*`` and never from the
    extractor's ``CHAT_*`` watch list. Named rather than inlined because
    :mod:`bot.chat.followup` has to apply the same rule to a channel it was
    handed rather than to a message, and two spellings of an allow-list is one
    too many.

    ``is_watched`` is reused rather than reimplemented so both features resolve
    a category and a thread identically: a channel under an allowed category is
    allowed (including one added to that category later), and a thread counts as
    its parent channel.
    """
    return is_watched(
        channel,
        settings.chat_pilot_channel_id_list,
        settings.chat_pilot_category_id_list,
    )


@dataclass(frozen=True)
class ChatDecision:
    """Answer or not, and why -- the reason is for the log, never for Discord."""

    act: bool
    reason: str
    #: Rate limited: react :data:`RATE_LIMITED_REACTION` and drop. Never set
    #: together with ``act``.
    busy: bool = False
    #: With ``busy``, how long until the refusing budget has room again -- so
    #: the bot can say *when* rather than only *no*. Zero for every other
    #: outcome, including the refusals that say nothing at all.
    retry_after_s: float = 0.0

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


def mentions_bot(
    message: Any,
    bot_user_id: int | str | None,
    self_role_id: int | str | None = None,
    replied_author_id: int | str | None = None,
) -> bool:
    """Was the bot actually mentioned, or replied to?

    Three ways, all read from Discord's own **resolved** lists rather than from
    the message text, so typing ``<@000000000000000000>`` or the bot's name into
    a message is text and nothing more:

    1. ``message.mentions`` contains the bot.
    2. ``message.role_mentions`` contains the bot's own **managed** role --
       ``guild.self_role``, the integration role Discord creates for this bot and
       nobody else can hold. Discord's autocomplete offers that role to anybody
       typing the bot's name, so "@YuukiSakuna what's on?" routinely arrives as
       a role mention. Ignoring it dropped real questions on the floor.
    3. ``replied_author_id`` is the bot: a reply continues a conversation, and
       it is checked separately so a reply still works with the ping turned off.
       It is passed in as data rather than read off ``message.reference``,
       because resolving a reply may need an API call -- see
       :meth:`bot.chat.agent.ChatPilot.replied_author_id`.

    Every **other** role mention still does not count, and neither does
    ``@everyone``/``@here``. The bot holds ordinary guild roles like anybody
    else, and a channel-wide ping -- or a ping of a role it happens to be in --
    must not summon it. Only the role that *is* the bot does.
    """
    if bot_user_id is None:
        return False
    wanted = str(bot_user_id)
    for user in getattr(message, "mentions", None) or ():
        if str(getattr(user, "id", user)) == wanted:
            return True
    if self_role_id is not None:
        managed = str(self_role_id)
        for role in getattr(message, "role_mentions", None) or ():
            if str(getattr(role, "id", role)) == managed:
                return True
    return replied_author_id is not None and str(replied_author_id) == wanted


def _before_the_mention_check(
    message: Any, settings: Settings, bot_user_id: int | str | None, enabled: bool
) -> ChatDecision | None:
    """The cheap checks that run before the mention test; ``None`` to carry on.

    Split out so :func:`would_check_mention` can ask "would this message even
    reach the mention test?" without duplicating the order, and without the
    caller paying for a reference lookup on every message in a busy channel.
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

    # The pilot's own allow-list (:func:`is_chat_channel`). It is independent of
    # the extractor's watch list rather than mutually exclusive with it --
    # pointing the pilot's category list at the bossing category *does* make the
    # bot answer in every party channel under it, which is a deliberate choice
    # for whoever writes `.env`, not something this gate second-guesses.
    if not is_chat_channel(getattr(message, "channel", None), settings):
        return ChatDecision(False, "not a chat channel")
    return None


def would_check_mention(
    message: Any, settings: Settings, *, bot_user_id: int | str | None, enabled: bool = True
) -> bool:
    """Would this message get as far as the mention test?

    Asked before spending an API call resolving a reply: a watched party channel
    that is not a chat channel produces hundreds of messages a day, and none of
    them is worth a ``fetch_message`` to find out who was replied to.
    """
    return _before_the_mention_check(message, settings, bot_user_id, enabled) is None


def _spend_an_answer(
    limiter: Any | None, global_limiter: Any | None, author_id: Any
) -> ChatDecision | None:
    """Take one answer from each pool, or say which pool is empty.

    Both pools are *read* before either is spent, because
    :meth:`bot.chat.ratelimit.RateLimiter.allow` records an allowance as it
    answers: spending the personal slot and only then discovering the guild's
    pool is dry would charge somebody for an answer they never got.

    Check-then-act is safe here for the same reason it is safe in
    :meth:`bot.chat.agent.ChatPilot.on_rejection`: one event loop, and nothing
    between the reads and the writes awaits.
    """
    if limiter is not None and limiter.remaining(author_id) <= 0:
        return ChatDecision(
            False, "rate limited", busy=True, retry_after_s=limiter.retry_after(author_id)
        )
    if global_limiter is not None and global_limiter.remaining(GLOBAL_KEY) <= 0:
        return ChatDecision(
            False, POOL_SPENT, busy=True, retry_after_s=global_limiter.retry_after(GLOBAL_KEY)
        )
    if limiter is not None:
        limiter.allow(author_id)
    if global_limiter is not None:
        global_limiter.allow(GLOBAL_KEY)
    return None


def decide(
    message: Any,
    settings: Settings,
    *,
    bot_user_id: int | str | None,
    enabled: bool = True,
    is_admin: bool = False,
    limiter: Any | None = None,
    global_limiter: Any | None = None,
    self_role_id: int | str | None = None,
    replied_author_id: int | str | None = None,
) -> ChatDecision:
    """Whether to answer ``message``.

    ``enabled`` is the runtime kill switch (``chat_mode``), passed in rather than
    read from the bot so this stays a function of its arguments. The two
    limiters are consulted last and only when everything else passed, so they
    record an allowance exactly when one is about to be spent.

    ``global_limiter`` is the guild's shared budget: the per-person window stops
    one member monopolising the model, and this stops *twenty* members doing it
    between them. The host has one model, so widening who holds the pilot role
    must not widen how much of the machine the guild can consume in an hour.

    ``is_admin`` is the existing "who runs this bot" rule
    (:func:`bot.agent.util.is_bot_admin`) and does two things here: it stands in for
    the chat role, and it exempts the holder from both budgets. Staff being
    silently ignored by their own bot is a support ticket nobody can debug from
    inside Discord -- and anyone who can already `/say`, `/debug` and approve
    every card gains nothing by also being made to hold the pilot role. Their
    answers do not drain the community's pool either, for the same reason. The
    pilot role stays the knob for everybody else.

    ``self_role_id`` and ``replied_author_id`` are the two facts about a mention
    that this function cannot work out for itself -- one needs the live guild,
    the other may need an API call -- so they arrive as data, exactly as
    ``bot_user_id`` does. See :func:`mentions_bot`.
    """
    early = _before_the_mention_check(message, settings, bot_user_id, enabled)
    if early is not None:
        return early

    if not mentions_bot(message, bot_user_id, self_role_id, replied_author_id):
        return ChatDecision(False, "the bot was not mentioned")

    role_id = settings.chat_pilot_role_id
    holds_role = role_id is not None and int(role_id) in _role_ids(getattr(message, "author", None))
    if not holds_role and not is_admin:
        # Silent on purpose: the person may not even know the bot is here, and
        # telling them would make the bot a way to get a reply out of it.
        return ChatDecision(False, "the author does not hold the chat role")

    if not is_admin:
        author = getattr(message, "author", None)
        refused = _spend_an_answer(limiter, global_limiter, getattr(author, "id", author))
        if refused is not None:
            return refused

    return ChatDecision(True, "ok")
