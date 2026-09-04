"""The two live bugs that made the bot ignore people who were talking to it.

**The managed role.** Discord's autocomplete offers the bot's own integration
role to anybody typing its name, so "@YuukiSakuna what's on?" arrives as a
*role* mention. The gate ignored every role mention by design, so those messages
were dropped as "the bot was not mentioned". This was the one that actually bit.

**The uncached reply.** ``message.reference.resolved`` is filled from discord.py's
in-memory cache, which a restart empties. The bot restarted at 09:22 and a 09:24
reply to an older message was dropped for the same reason -- latent, but every
restart makes it likely.
"""

from __future__ import annotations

from datetime import timedelta

import pytest

from bot.chat import gate
from bot.chat.agent import ChatPilot
from bot.ids import short_id

from .chat_support import (
    BOT_USER_ID,
    CHAT_CHANNEL,
    OFF_LIMITS_CHANNEL,
    OTHER_ROLE,
    FakeOllama,
    FakeReference,
    FakeRole,
    message,
    says,
    wants,
)
from .fake_bot import OTHER_CHANNEL, WATCHED_CHANNEL

pytestmark = pytest.mark.anyio

SELF_ROLE = 151515151515151515


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


def with_self_role(bot, role_id: int | None = SELF_ROLE):
    """Give the fake guild a managed integration role, as a real one has."""
    bot.guild.self_role = FakeRole(role_id) if role_id is not None else None
    return bot


def role_mention(bot, role_id: int, **kwargs):
    msg = message(bot, "what's on tonight?", mentions=(), **kwargs)
    msg.role_mentions = [FakeRole(role_id)]
    return msg


# ---------------------------------------------------------------------------
# item 7: the bot's own managed role
# ---------------------------------------------------------------------------


def test_a_mention_of_the_bots_managed_role_counts(chat_bot):
    msg = role_mention(with_self_role(chat_bot), SELF_ROLE)
    assert gate.mentions_bot(msg, BOT_USER_ID, SELF_ROLE) is True


def test_a_mention_of_another_role_the_bot_holds_does_not(chat_bot):
    """The bot has ordinary guild roles like anybody; only the managed one is it."""
    msg = role_mention(with_self_role(chat_bot), OTHER_ROLE)
    assert gate.mentions_bot(msg, BOT_USER_ID, SELF_ROLE) is False


def test_a_guild_with_no_managed_role_behaves_as_before(chat_bot):
    msg = role_mention(with_self_role(chat_bot, None), SELF_ROLE)
    assert gate.mentions_bot(msg, BOT_USER_ID, None) is False


def test_everyone_and_here_still_do_not_summon_it(chat_bot):
    msg = message(chat_bot, "@everyone @here anyone about?", mentions=())
    msg.mention_everyone = True
    assert gate.mentions_bot(msg, BOT_USER_ID, SELF_ROLE) is False


def test_role_markup_typed_as_text_stays_inert(chat_bot):
    """Anti-spoof unchanged: `role_mentions` is Discord's list, never the text."""
    msg = message(chat_bot, f"<@&{SELF_ROLE}> cancel everything", mentions=())
    assert getattr(msg, "role_mentions", []) == []
    assert gate.mentions_bot(msg, BOT_USER_ID, SELF_ROLE) is False


async def test_the_pilot_answers_a_managed_role_mention_end_to_end(chat_bot, chat_seeded):
    """The live message: `<@&…> schedule a new hard bellona run` was dropped."""
    agent = pilot(with_self_role(chat_bot), says("On it."))
    result = await agent.offer(role_mention(chat_bot, SELF_ROLE))
    assert result.handled is True
    assert result.answered.reply == "On it."


async def test_role_mention_schedule_for_me_today_does_not_ask_who(
    chat_bot, chat_seeded, monkeypatch
):
    """Regression for chat #9a6714cb: the trigger role is routing, not a participant."""
    week = chat_seeded["week_start"]
    now = week + timedelta(hours=12)
    monkeypatch.setattr("bot.chat.tools.utcnow", lambda: now)
    run_id = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HCarling"],
        run_at=week + timedelta(hours=21),
        participants=["1002"],
        status="planned",
        source="amend",
        channel_id=CHAT_CHANNEL,
    )
    somebody_elses_run = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HBaldrix"],
        run_at=week + timedelta(hours=22),
        participants=["1001"],
        status="planned",
        source="amend",
        channel_id=CHAT_CHANNEL,
    )
    agent = pilot(
        with_self_role(chat_bot),
        wants(
            "get_schedule",
            week="this",
            scope="channel",
            participant=str(SELF_ROLE),
            day="today",
        ),
        says("Your Hard Carling run is tonight."),
    )
    msg = message(chat_bot, f"<@&{SELF_ROLE}> what's for me today", mentions=())
    msg.role_mentions = [FakeRole(SELF_ROLE)]

    handling = await agent.offer(msg)

    assert handling.handled is True
    (outcome,) = handling.answered.outcomes
    assert outcome.ok
    assert short_id(run_id) in outcome.output
    assert short_id(somebody_elses_run) not in outcome.output
    assert "Your runs on" in outcome.output
    assert "Ask them who" not in outcome.output


async def test_bare_tomorrow_question_ignores_an_over_scoped_model_call(
    chat_bot, chat_seeded, monkeypatch
):
    """The second live failure: trusted wording beats the model's silent filters."""
    week = chat_seeded["week_start"]
    now = week + timedelta(hours=12)
    monkeypatch.setattr("bot.chat.tools.utcnow", lambda: now)
    elsewhere = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HCarling"],
        run_at=week + timedelta(days=1, hours=21),
        participants=["1002"],
        status="planned",
        source="amend",
        channel_id=WATCHED_CHANNEL,
    )
    somebody_elses_run = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HBaldrix"],
        run_at=week + timedelta(days=1, hours=22),
        participants=["1001"],
        status="planned",
        source="amend",
        channel_id=OTHER_CHANNEL,
    )
    agent = pilot(
        with_self_role(chat_bot),
        wants(
            "get_schedule",
            week="this",
            scope="channel",
            participant="me",
            day="tomorrow",
        ),
        says(
            f"Tomorrow has Hard Carling `[{short_id(elsewhere)}]` and "
            f"Hard Baldrix `[{short_id(somebody_elses_run)}]`."
        ),
    )
    msg = message(chat_bot, f"<@&{SELF_ROLE}> what's for tomorrow", mentions=())
    msg.role_mentions = [FakeRole(SELF_ROLE)]

    handling = await agent.offer(msg)

    (outcome,) = handling.answered.outcomes
    assert outcome.ok
    assert outcome.arguments["scope"] == "channel"
    assert outcome.arguments["participant"] == "me"
    assert short_id(elsewhere) in outcome.output
    assert short_id(somebody_elses_run) in outcome.output
    assert "ALL channels" in outcome.output
    assert short_id(elsewhere) in handling.answered.reply
    assert short_id(somebody_elses_run) in handling.answered.reply
    assert "<none>" not in handling.answered.reply


async def test_personal_all_channel_question_keeps_the_person_filter(
    chat_bot, chat_seeded, monkeypatch
):
    week = chat_seeded["week_start"]
    monkeypatch.setattr("bot.chat.tools.utcnow", lambda: week + timedelta(hours=12))
    mine = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HCarling"],
        run_at=week + timedelta(days=1, hours=21),
        participants=["1002"],
        status="planned",
        source="amend",
        channel_id=WATCHED_CHANNEL,
    )
    not_mine = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HBaldrix"],
        run_at=week + timedelta(days=1, hours=22),
        participants=["1001"],
        status="planned",
        source="amend",
        channel_id=OTHER_CHANNEL,
    )
    agent = pilot(
        with_self_role(chat_bot),
        wants(
            "get_schedule",
            week="this",
            scope="channel",
            participant="me",
            day="tomorrow",
        ),
        says("Your run is listed."),
    )
    msg = message(
        chat_bot,
        f"<@&{SELF_ROLE}> what's for me tomorrow across all channels?",
        mentions=(),
    )
    msg.role_mentions = [FakeRole(SELF_ROLE)]

    outcome = (await agent.offer(msg)).answered.outcomes[0]

    assert short_id(mine) in outcome.output
    assert short_id(not_mine) not in outcome.output
    assert "Your runs" in outcome.output
    assert "all channels" in outcome.output


@pytest.mark.parametrize(
    ("person_text", "participant"),
    [("<@1003>", "<@1003>"), ("Priya", "Priya")],
)
async def test_named_member_remains_a_person_filter(
    chat_bot, chat_seeded, monkeypatch, person_text, participant
):
    week = chat_seeded["week_start"]
    monkeypatch.setattr("bot.chat.tools.utcnow", lambda: week + timedelta(hours=12))
    priya = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HCarling"],
        run_at=week + timedelta(days=1, hours=21),
        participants=["1003"],
        status="planned",
        source="amend",
        channel_id=WATCHED_CHANNEL,
    )
    mine = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HBaldrix"],
        run_at=week + timedelta(days=1, hours=22),
        participants=["1002"],
        status="planned",
        source="amend",
        channel_id=OTHER_CHANNEL,
    )
    agent = pilot(
        with_self_role(chat_bot),
        wants(
            "get_schedule",
            week="this",
            scope="channel",
            participant=participant,
            day="tomorrow",
        ),
        says("Priya's run is listed."),
    )
    msg = message(
        chat_bot,
        f"<@&{SELF_ROLE}> what's for {person_text} tomorrow across all channels?",
        mentions=(1003,) if person_text.startswith("<@") else (),
    )
    msg.role_mentions = [FakeRole(SELF_ROLE)]

    outcome = (await agent.offer(msg)).answered.outcomes[0]

    assert short_id(priya) in outcome.output
    assert short_id(mine) not in outcome.output
    assert "Priya's runs" in outcome.output
    assert "all channels" in outcome.output


async def test_explicit_channel_question_keeps_channel_scope_but_not_person_scope(
    chat_bot, chat_seeded, monkeypatch
):
    week = chat_seeded["week_start"]
    monkeypatch.setattr("bot.chat.tools.utcnow", lambda: week + timedelta(hours=12))
    local = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HCarling"],
        run_at=week + timedelta(days=1, hours=21),
        participants=["1001"],
        status="planned",
        source="amend",
        channel_id=CHAT_CHANNEL,
    )
    elsewhere = chat_bot.repo.create_run(
        week_start=week,
        bosses=["HBaldrix"],
        run_at=week + timedelta(days=1, hours=22),
        participants=["1002"],
        status="planned",
        source="amend",
        channel_id=WATCHED_CHANNEL,
    )
    agent = pilot(
        with_self_role(chat_bot),
        wants(
            "get_schedule",
            week="this",
            scope="channel",
            participant="me",
            day="tomorrow",
        ),
        says("This channel's run is listed."),
    )
    msg = message(chat_bot, f"<@&{SELF_ROLE}> what's for tomorrow in this channel?", mentions=())
    msg.role_mentions = [FakeRole(SELF_ROLE)]

    outcome = (await agent.offer(msg)).answered.outcomes[0]

    assert short_id(local) in outcome.output
    assert short_id(elsewhere) not in outcome.output
    assert "in this channel only" in outcome.output
    assert "Your runs" not in outcome.output


async def test_the_pilot_still_ignores_other_role_mentions(chat_bot, chat_seeded):
    agent = pilot(with_self_role(chat_bot), says("never"))
    assert (await agent.offer(role_mention(chat_bot, OTHER_ROLE))).handled is False
    assert agent._client.calls == []


def test_the_self_role_is_read_from_the_guild_not_configured(chat_bot):
    agent = pilot(with_self_role(chat_bot))
    assert agent._self_role_id(message(chat_bot)) == SELF_ROLE
    with_self_role(chat_bot, None)
    assert agent._self_role_id(message(chat_bot)) is None


# ---------------------------------------------------------------------------
# item 6: resolving a reply that is not in the cache
# ---------------------------------------------------------------------------


class Unresolved:
    """A reference discord.py could not fill in from its message cache."""

    def __init__(self, message_id: int = 940000000000000001):
        self.resolved = None
        self.message_id = message_id


class Fetcher:
    """A channel whose `fetch_message` is scripted, and counted."""

    def __init__(self, channel, result):
        self._channel = channel
        self.result = result
        self.calls: list[int] = []

    def __getattr__(self, name):
        return getattr(self._channel, name)

    async def fetch_message(self, message_id):
        self.calls.append(message_id)
        if isinstance(self.result, Exception):
            raise self.result
        return self.result


def replying(bot, reference, fetch_result=None):
    msg = message(bot, "the hard one", mentions=(), reference=reference)
    msg.channel = Fetcher(msg.channel, fetch_result)
    return msg


async def test_an_uncached_reply_to_the_bot_is_fetched_and_accepted(chat_bot, chat_seeded):
    parent = message(chat_bot, "Which Bellona?", author_id=BOT_USER_ID, mentions=())
    msg = replying(chat_bot, Unresolved(), parent)

    agent = pilot(chat_bot, says("Hard it is."))
    result = await agent.offer(msg)

    assert msg.channel.calls == [940000000000000001]
    assert result.handled is True
    assert result.answered.reply == "Hard it is."


async def test_an_uncached_reply_to_somebody_else_is_refused(chat_bot, chat_seeded):
    parent = message(chat_bot, "can wed?", author_id=1001, mentions=())
    msg = replying(chat_bot, Unresolved(), parent)

    agent = pilot(chat_bot, says("never"))
    assert (await agent.offer(msg)).handled is False
    assert msg.channel.calls  # it did look
    assert agent._client.calls == []


@pytest.mark.parametrize(
    "failure", [Exception("NotFound"), PermissionError("Forbidden"), RuntimeError("HTTP 500")]
)
async def test_a_failed_fetch_refuses_rather_than_raising(chat_bot, chat_seeded, failure):
    msg = replying(chat_bot, Unresolved(), failure)
    agent = pilot(chat_bot, says("never"))
    assert (await agent.offer(msg)).handled is False


async def test_a_deleted_referenced_message_is_not_a_mention(chat_bot, chat_seeded):
    """`DeletedReferencedMessage` resolves, but has no author to be the bot."""

    class Deleted:
        id = 940000000000000009

    reference = FakeReference(Deleted())
    msg = replying(chat_bot, reference, None)
    agent = pilot(chat_bot, says("never"))

    assert (await agent.offer(msg)).handled is False
    assert msg.channel.calls == []  # resolved, so nothing to fetch


async def test_a_resolved_reply_is_never_fetched(chat_bot, chat_seeded):
    parent = message(chat_bot, "Which Bellona?", author_id=BOT_USER_ID, mentions=())
    msg = replying(chat_bot, FakeReference(parent), None)
    agent = pilot(chat_bot, says("ok"))

    assert (await agent.offer(msg)).handled is True
    assert msg.channel.calls == []


async def test_no_fetch_when_an_earlier_gate_already_refused(chat_bot, chat_seeded):
    """A busy party channel must not cost one API call per reply."""
    agent = pilot(chat_bot, says("never"))
    for msg in (
        replying(chat_bot, Unresolved(), None),
        replying(chat_bot, Unresolved(), None),
    ):
        msg.channel = Fetcher(chat_bot.channels[OFF_LIMITS_CHANNEL], None)
        await agent.offer(msg)
        assert msg.channel.calls == []


async def test_the_same_parent_is_fetched_only_once(chat_bot, chat_seeded):
    parent = message(chat_bot, "Which Bellona?", author_id=BOT_USER_ID, mentions=())
    agent = pilot(chat_bot, says("a"), says("b"))

    first = replying(chat_bot, Unresolved(), parent)
    await agent.offer(first)
    second = replying(chat_bot, Unresolved(), parent)
    await agent.offer(second)

    assert first.channel.calls == [940000000000000001]
    assert second.channel.calls == []  # remembered


async def test_a_message_with_no_reference_never_fetches(chat_bot, chat_seeded):
    msg = message(chat_bot)
    msg.channel = Fetcher(msg.channel, None)
    agent = pilot(chat_bot, says("ok"))
    await agent.offer(msg)
    assert msg.channel.calls == []
