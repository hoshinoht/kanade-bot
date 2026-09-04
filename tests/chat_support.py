"""Stand-ins for the speech pilot's tests: a guild, a member, and a scripted model.

Every id here is synthetic and local to the suite, in the same range as the ones
in :mod:`tests.fake_bot`. Nothing in this file corresponds to a real Discord
object, a real member, or the deployment's real roles.

The model is scripted rather than mocked: :class:`FakeOllama` hands back a queue
of responses in the shape ``ollama.AsyncClient.chat`` returns, so the agent's
tool loop is exercised for real -- including the tool dispatch, which is where
every security property of the feature actually lives.
"""

from __future__ import annotations

from typing import Any

from bot.domain.bosses import BossTable
from bot.infrastructure.db import Repo

from .fake_bot import FakeBot, FakeChannel, FakeMe, make_settings

#: The channel the chatbot answers in. Deliberately not one of `fake_bot`'s
#: watched channels: the pilot's allow-list is its own.
CHAT_CHANNEL = 777777777777777777
#: A channel that exists and is not on the pilot's allow-list.
OFF_LIMITS_CHANNEL = 888888888888888888
#: A category the chatbot answers in, and a channel that sits under it. The
#: channel is on no explicit list -- it is allowed purely by its category.
CHAT_CATEGORY = 404040404040404040
ADOPTED_CHANNEL = 505050505050505050
#: A category that is on no list, and a channel under it.
OTHER_CATEGORY = 606060606060606060
ORPHAN_CHANNEL = 707070707070707070
#: The role a member must hold to talk to the bot.
CHAT_ROLE = 101010101010101010
#: A role used to exercise portal-managed behaviour plugins.
PLUGIN_ROLE = 111111111111111111
#: The existing "who runs this bot" role, which is exempt from the rate limit.
ADMIN_ROLE = 202020202020202020
#: A role that is not either of the above.
OTHER_ROLE = 303030303030303030

BOT_USER_ID = FakeMe.id


class FakeRole:
    def __init__(self, role_id: int):
        self.id = role_id


class FakePermissions:
    def __init__(self, administrator: bool = False):
        self.administrator = administrator


class FakeAuthor:
    """A guild member as the gate sees one."""

    def __init__(
        self,
        user_id: int,
        roles: tuple[int, ...] = (CHAT_ROLE,),
        bot: bool = False,
        administrator: bool = False,
        display_name: str = "Someone",
    ):
        self.id = user_id
        self.roles = [FakeRole(r) for r in roles]
        self.bot = bot
        self.display_name = display_name
        self.guild_permissions = FakePermissions(administrator)


class FakeReference:
    def __init__(self, resolved: Any):
        self.resolved = resolved


class FakeIncoming:
    """A message arriving at ``on_message``."""

    _next_id = 950000000000000000

    def __init__(
        self,
        content: str,
        author: FakeAuthor,
        channel: Any,
        guild: Any,
        mentions: tuple[int, ...] = (BOT_USER_ID,),
        reference: Any = None,
    ):
        FakeIncoming._next_id += 1
        self.id = FakeIncoming._next_id
        self.content = content
        self.author = author
        self.channel = channel
        self.guild = guild
        # Discord's *resolved* mention list, which is what the gate reads. A
        # member typing "<@...>" as text does not land here.
        self.mentions = [FakeAuthor(uid, roles=()) for uid in mentions]
        #: Discord's resolved ROLE mention list. Empty unless a test sets it --
        #: markup typed into `content` never lands here, which is the point.
        self.role_mentions: list[FakeRole] = []
        self.reference = reference
        self.reactions: list[str] = []

    async def add_reaction(self, emoji: str) -> None:
        self.reactions.append(emoji)

    async def remove_reaction(self, emoji: str, _member: Any = None) -> None:
        if emoji in self.reactions:
            self.reactions.remove(emoji)


# ---------------------------------------------------------------------------
# the model
# ---------------------------------------------------------------------------


def says(text: str) -> dict:
    """A model response that answers in words."""
    return {"message": {"content": text, "tool_calls": []}}


def wants(name: str, **arguments: Any) -> dict:
    """A model response that calls one tool."""
    return {
        "message": {
            "content": "",
            "tool_calls": [{"function": {"name": name, "arguments": arguments}}],
        }
    }


def costed(response: dict, prompt: int, completion: int) -> dict:
    """A scripted response that also reports Ollama's token counts.

    Separate from :func:`says` and :func:`wants` because the counts are
    *optional* in the wire format -- a response without them is the normal case
    for a stand-in and has to keep working.
    """
    return {**response, "prompt_eval_count": prompt, "eval_count": completion}


class FakeOllama:
    """Hands back scripted responses and records what it was asked."""

    def __init__(self, *responses: Any):
        self.responses = list(responses)
        self.calls: list[dict] = []
        self.closed = False

    async def chat(self, **kwargs: Any) -> Any:
        self.calls.append(kwargs)
        if not self.responses:
            return says("(nothing scripted)")
        response = self.responses.pop(0)
        if isinstance(response, Exception):
            raise response
        return response

    async def close(self) -> None:
        self.closed = True

    @property
    def prompts(self) -> list[list[dict]]:
        return [call["messages"] for call in self.calls]

    @property
    def system(self) -> str:
        """The system prompt of the first call."""
        return self.calls[0]["messages"][0]["content"]

    def conversation(self, index: int = 0) -> list[dict]:
        """One call's messages without the trailing voice reminder.

        Every call ends with that reminder by design (it is the last thing the
        model reads before composing), so tests about the *conversation* -- what
        was remembered, what came back from a tool -- drop it rather than
        counting round it.
        """
        return self.prompts[index][:-1]

    def reminder(self, index: int = -1) -> dict:
        """The trailing voice reminder of one call."""
        return self.calls[index]["messages"][-1]


# ---------------------------------------------------------------------------
# the guild
# ---------------------------------------------------------------------------


def chat_settings(**overrides: Any):
    values: dict[str, Any] = {
        "chat_pilot_role_id": CHAT_ROLE,
        "chat_pilot_channel_ids": str(CHAT_CHANNEL),
        "chat_pilot_category_ids": str(CHAT_CATEGORY),
        "admin_role_id": ADMIN_ROLE,
        # Empty means "no file was configured", so the loader falls back to the
        # tracked template and the tests never depend on a developer's own
        # persona being present.
        "persona_path": "",
    }
    values.update(overrides)
    return make_settings(**values)


def build_bot(repo: Repo, bosses: BossTable, **overrides: Any) -> FakeBot:
    """A :class:`~tests.fake_bot.FakeBot` with a chat channel and a real pipeline.

    The extractor is the *real* :class:`bot.extract.pipeline.Pipeline` rather
    than the stand-in, because the whole point of the write tools is that they
    go through it: a card raised in chat has to be the same object a rescan
    raises, and a faked ``apply_plan`` would prove nothing about that.
    """
    from bot.extract.pipeline import Pipeline

    bot = FakeBot(repo, bosses, chat_settings(**overrides))
    for channel_id, name, category in (
        (CHAT_CHANNEL, "ask-the-bot", None),
        (OFF_LIMITS_CHANNEL, "general", None),
        # Allowed by its category alone -- it is on no explicit channel list.
        (ADOPTED_CHANNEL, "bot-chatter", CHAT_CATEGORY),
        # Under a category nobody allowed.
        (ORPHAN_CHANNEL, "somewhere-else", OTHER_CATEGORY),
    ):
        channel = FakeChannel(channel_id, name, category_id=category)
        channel.guild = bot.guild
        bot.channels[channel_id] = channel
        bot.guild.text_channels.append(channel)
    bot.extractor = Pipeline(bot)
    return bot


def thread_in(bot: FakeBot, parent_id: int, thread_id: int = 909000000000000001) -> FakeChannel:
    """A thread whose ``.parent`` is ``parent_id``, as discord.py presents one."""
    thread = FakeChannel(thread_id, "a-thread")
    thread.parent = bot.channels[parent_id]
    thread.guild = bot.guild
    bot.channels[thread_id] = thread
    return thread


def message(
    bot: FakeBot,
    content: str = "@bot what's on tonight?",
    *,
    author_id: int = 1002,
    roles: tuple[int, ...] = (CHAT_ROLE,),
    channel_id: int = CHAT_CHANNEL,
    mentions: tuple[int, ...] = (BOT_USER_ID,),
    guild: Any = None,
    is_bot: bool = False,
    administrator: bool = False,
    reference: Any = None,
) -> FakeIncoming:
    """A message that would be answered, unless a keyword says otherwise."""
    return FakeIncoming(
        content,
        FakeAuthor(author_id, roles=roles, bot=is_bot, administrator=administrator),
        bot.channels[channel_id],
        bot.guild if guild is None else guild,
        mentions=mentions,
        reference=reference,
    )


def seed(bot: FakeBot) -> dict:
    """Two parties and a materialised week -- the same shape as `conftest.seeded`.

    Returned as ids so a test can name what it means. The roster names are the
    suite's existing placeholders; nothing here is a real member.
    """
    from bot.agent.materialise import materialise_week
    from bot.domain.weeks import current_week_start

    from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ
    from .fake_bot import OTHER_CHANNEL, WATCHED_CHANNEL

    repo = bot.repo
    for user_id, name, nick in (
        (1001, "Alvin tan", None),
        (1002, "kanon [AZUR]", "kanon"),
        (1003, "Priya", None),
    ):
        repo.upsert_member(user_id, name, nick, True)
    repo.upsert_member(1009, "NotABosser", None, False)

    ws = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME)
    repo.add_fixed_run(
        1001, ["HStar", "HFA"], 0, "21:30", ["1001", "1002"], channel_id=WATCHED_CHANNEL
    )
    repo.add_fixed_run(1002, ["XKalos"], 1, "23:00", ["1002", "1003"], channel_id=OTHER_CHANNEL)
    materialise_week(repo, ws, TZ, PING_TIME, COUNTDOWNS, now=ws)
    runs = repo.list_runs(week_start=ws)
    star = next(r for r in runs if "HStar" in r["bosses"])
    kalos = next(r for r in runs if "XKalos" in r["bosses"])
    return {"week_start": ws, "star": star["id"], "kalos": kalos["id"]}


__all__ = [
    "ADMIN_ROLE",
    "ADOPTED_CHANNEL",
    "BOT_USER_ID",
    "CHAT_CATEGORY",
    "CHAT_CHANNEL",
    "CHAT_ROLE",
    "OFF_LIMITS_CHANNEL",
    "ORPHAN_CHANNEL",
    "OTHER_CATEGORY",
    "OTHER_ROLE",
    "FakeAuthor",
    "FakeIncoming",
    "FakeOllama",
    "FakeReference",
    "FakeRole",
    "build_bot",
    "chat_settings",
    "costed",
    "message",
    "says",
    "seed",
    "thread_in",
    "wants",
]
