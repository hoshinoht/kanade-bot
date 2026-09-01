"""What the bot puts on your message while it thinks.

An answer is 10-30 s of GPU. Without a marker that is indistinguishable from
being ignored, which is the one thing the gate's silence is supposed to mean.

Three reactions, and the tests here are mostly about telling them apart:

* 👀 -- heard you, working on it. Removed when the reply lands.
* ⏳ -- you have had your answers for now (minutes).
* 💬 -- I am still answering somebody else in this channel (seconds).
"""

from __future__ import annotations

import asyncio

import pytest

from bot.chat import gate
from bot.chat.agent import ChatPilot

from .chat_support import (
    ADMIN_ROLE,
    CHAT_ROLE,
    OFF_LIMITS_CHANNEL,
    OTHER_ROLE,
    FakeOllama,
    message,
    says,
)

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


# ---------------------------------------------------------------------------
# 👀 while working
# ---------------------------------------------------------------------------


async def test_an_accepted_message_gets_the_seen_reaction_and_loses_it(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("Wed 21:30."))
    msg = message(chat_bot)
    result = await agent.offer(msg)

    assert result.handled is True
    assert msg.reactions == []  # added, then taken back off


async def test_the_seen_reaction_is_on_while_the_answer_is_being_written(chat_bot, chat_seeded):
    """The point of it: it must be visible *during* the slow part."""
    seen_midway: list[str] = []
    released = asyncio.Event()
    msg = message(chat_bot)
    agent = pilot(chat_bot)

    async def slow(**_kwargs):
        seen_midway.extend(msg.reactions)
        await released.wait()
        return says("done")

    agent._client.chat = slow
    task = asyncio.create_task(agent.offer(msg))
    await asyncio.sleep(0)
    released.set()
    await task

    assert seen_midway == [gate.SEEN_REACTION]
    assert msg.reactions == []


async def test_the_seen_reaction_comes_off_even_when_the_model_fails(chat_bot, chat_seeded):
    agent = pilot(chat_bot, ConnectionError("ollama is down"))
    msg = message(chat_bot)
    await agent.offer(msg)
    assert msg.reactions == []


async def test_a_refused_message_gets_no_reaction_at_all(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("never"))
    for msg in (
        message(chat_bot, roles=(OTHER_ROLE,)),
        message(chat_bot, mentions=()),
        message(chat_bot, channel_id=OFF_LIMITS_CHANNEL),
    ):
        await agent.offer(msg)
        assert msg.reactions == []


# ---------------------------------------------------------------------------
# ⏳ and 💬 are tellable apart
# ---------------------------------------------------------------------------


async def test_rate_limited_gets_the_hourglass(chat_bot, chat_seeded):
    agent = pilot(chat_bot, *[says("ok")] * 4)
    agent.limiter.count = 1
    await agent.offer(message(chat_bot))
    limited = message(chat_bot)
    await agent.offer(limited)
    assert limited.reactions == [gate.RATE_LIMITED_REACTION]


async def test_a_busy_channel_gets_the_speech_bubble(chat_bot, chat_seeded):
    released = asyncio.Event()
    agent = pilot(chat_bot)

    async def slow(**_kwargs):
        await released.wait()
        return says("first")

    agent._client.chat = slow
    first = asyncio.create_task(agent.offer(message(chat_bot)))
    await asyncio.sleep(0)

    busy = message(chat_bot, author_id=1001)
    await agent.offer(busy)
    released.set()
    await first

    assert busy.reactions == [gate.CHANNEL_BUSY_REACTION]
    assert gate.CHANNEL_BUSY_REACTION != gate.RATE_LIMITED_REACTION


async def test_an_admin_is_never_given_the_hourglass(chat_bot, chat_seeded):
    agent = pilot(chat_bot, *[says("ok")] * 4)
    agent.limiter.count = 1
    for _ in range(3):
        msg = message(chat_bot, roles=(CHAT_ROLE, ADMIN_ROLE))
        await agent.offer(msg)
        assert gate.RATE_LIMITED_REACTION not in msg.reactions


# ---------------------------------------------------------------------------
# nothing about reactions may break an answer
# ---------------------------------------------------------------------------


async def test_a_message_that_cannot_be_reacted_to_is_still_answered(chat_bot, chat_seeded):
    """No Add Reactions permission, or the message was deleted mid-answer."""

    async def boom(*_args, **_kwargs):
        raise RuntimeError("missing permissions")

    agent = pilot(chat_bot, says("Wed 21:30."))
    msg = message(chat_bot)
    msg.add_reaction = boom
    msg.remove_reaction = boom

    result = await agent.offer(msg)
    assert result.answered.reply == "Wed 21:30."


async def test_a_message_object_without_reactions_is_tolerated(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("ok"))
    msg = message(chat_bot)
    msg.add_reaction = msg.remove_reaction = None  # a message type that has neither
    assert (await agent.offer(msg)).answered.reply == "ok"


async def test_the_bot_removes_its_own_reaction_not_anybody_elses(chat_bot, chat_seeded):
    """Removing your own needs no Manage Messages; removing others' does."""
    removed: list[tuple] = []
    agent = pilot(chat_bot, says("ok"))
    msg = message(chat_bot)

    async def record(emoji, member=None):
        removed.append((emoji, getattr(member, "id", member)))

    msg.remove_reaction = record
    await agent.offer(msg)
    assert removed == [(gate.SEEN_REACTION, chat_bot.user.id)]
