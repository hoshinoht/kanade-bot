"""Who owns a message when the chatbot and the extractor both want it.

Live, the pilot's channel turned out to sit under a category in the extractor's
`CHAT_CATEGORY_IDS`, so one "@bot move hstar to wednesday" produced a chat reply
*and* an extractor proposal card for the same sentence. A message addressed to
the bot is a conversation, not ambient party chat.

The subtlety these tests exist to protect: the verdict is computed **once**, by
the gate, inside `ChatPilot.offer`. Asking again in `on_message` would consult
the rate limiter a second time and spend two of somebody's four answers on one
message.
"""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime

import pytest

from bot.chat.agent import Handling
from bot.client import BossBot
from bot.db import Repo

from .chat_support import (
    ADOPTED_CHANNEL,
    CHAT_CHANNEL,
    OTHER_ROLE,
    chat_settings,
    message,
)
from .fake_bot import UNWATCHED_CHANNEL, WATCHED_CHANNEL


class Recorder:
    """Stands in for either offer, remembering what it was given."""

    def __init__(self, handling: Handling | None = None):
        self.seen: list = []
        self.handling = handling

    async def offer(self, msg):
        self.seen.append(msg)
        return self.handling


class Exploding:
    async def offer(self, _msg):
        raise RuntimeError("the model exploded")


def wire(repo: Repo, handling: Handling, **overrides):
    """A real client with only what `on_message` reaches for."""
    client = BossBot.__new__(BossBot)
    client.repo = repo
    client.settings = chat_settings(**overrides)
    client.extractor = Recorder()
    client.chat = Recorder(handling)
    return client


def deliver(client, msg):
    asyncio.run(BossBot.on_message(client, msg))


def dated(msg):
    msg.created_at = datetime.now(UTC)
    return msg


# ---------------------------------------------------------------------------
# a handled message is the pilot's alone
# ---------------------------------------------------------------------------


def test_a_mention_the_pilot_answers_is_not_offered_to_the_extractor(repo, chat_bot):
    """The live bug: one sentence, one reply, and no stray proposal card."""
    client = wire(repo, Handling(True, "ok"))
    # The channel is watched *and* chat-allowed, which is the collision.
    msg = dated(message(chat_bot, "@bot move hstar to wednesday", channel_id=WATCHED_CHANNEL))
    deliver(client, msg)

    assert client.chat.seen == [msg]
    assert client.extractor.seen == []
    # Recording is unconditional for a watched channel, whoever acts on it.
    assert client.repo.get_message(msg.id) is not None


def test_a_rate_limited_mention_is_still_the_pilots(repo, chat_bot):
    """It reacted ⏳; the extractor must not pick the sentence up instead."""
    client = wire(repo, Handling(True, "rate limited"))
    msg = dated(message(chat_bot, "@bot what's on?", channel_id=WATCHED_CHANNEL))
    deliver(client, msg)

    assert client.chat.seen == [msg]
    assert client.extractor.seen == []
    assert client.repo.get_message(msg.id) is not None


def test_a_busy_channel_drop_is_still_the_pilots(repo, chat_bot):
    client = wire(repo, Handling(True, "already answering"))
    deliver(client, dated(message(chat_bot, "@bot again", channel_id=WATCHED_CHANNEL)))
    assert client.extractor.seen == []


# ---------------------------------------------------------------------------
# every other refusal leaves the extractor exactly as it was
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "reason",
    [
        "not a chat channel",
        "the bot was not mentioned",
        "the author does not hold the chat role",
        "chat_mode is off",
        "the chat pilot is not configured",
    ],
)
def test_an_unhandled_message_still_reaches_the_extractor(repo, chat_bot, reason):
    client = wire(repo, Handling(False, reason))
    msg = dated(message(chat_bot, "can wed?", channel_id=WATCHED_CHANNEL, mentions=()))
    deliver(client, msg)

    assert client.extractor.seen == [msg]
    assert client.repo.get_message(msg.id) is not None


def test_ambient_chat_in_a_channel_that_is_both_is_still_extracted(repo, chat_bot):
    """No mention, so the gate refuses and the party channel behaves as always."""
    client = wire(repo, Handling(False, "the bot was not mentioned"))
    msg = dated(message(chat_bot, "mon 9:30 can?", channel_id=WATCHED_CHANNEL, mentions=()))
    deliver(client, msg)
    assert client.extractor.seen == [msg]


def test_a_mention_from_somebody_without_the_role_is_extracted_as_before(repo, chat_bot):
    client = wire(repo, Handling(False, "the author does not hold the chat role"))
    msg = dated(
        message(chat_bot, "@bot move hstar", channel_id=WATCHED_CHANNEL, roles=(OTHER_ROLE,))
    )
    deliver(client, msg)
    assert client.extractor.seen == [msg]


# ---------------------------------------------------------------------------
# channels that are only one thing
# ---------------------------------------------------------------------------


def test_the_pilots_own_channel_is_never_recorded_or_extracted(repo, chat_bot):
    """It is not watched, so talking to the bot never becomes proposals."""
    client = wire(repo, Handling(True, "ok"))
    msg = dated(message(chat_bot, "@bot what's on?", channel_id=CHAT_CHANNEL))
    deliver(client, msg)

    assert client.chat.seen == [msg]
    assert client.extractor.seen == []
    assert client.repo.get_message(msg.id) is None


def test_an_unwatched_non_chat_channel_reaches_neither(repo, chat_bot):
    client = wire(repo, Handling(False, "not a chat channel"))
    msg = dated(message(chat_bot, "hello", channel_id=UNWATCHED_CHANNEL))
    deliver(client, msg)

    assert client.chat.seen == [msg]  # the gate is what refuses it
    assert client.extractor.seen == []
    assert client.repo.get_message(msg.id) is None


def test_a_chat_only_category_channel_is_not_extracted(repo, chat_bot):
    client = wire(repo, Handling(True, "ok"))
    deliver(client, dated(message(chat_bot, "@bot hi", channel_id=ADOPTED_CHANNEL)))
    assert client.extractor.seen == []


# ---------------------------------------------------------------------------
# neither half may break the other
# ---------------------------------------------------------------------------


def test_a_broken_chat_pilot_leaves_the_extractor_running(repo, chat_bot):
    """Fail-safe: an exception is "not handled", so ambient chat still works."""
    client = wire(repo, Handling(True, "ok"))
    client.chat = Exploding()
    msg = dated(message(chat_bot, "can wed?", channel_id=WATCHED_CHANNEL, mentions=()))
    deliver(client, msg)
    assert client.extractor.seen == [msg]


def test_a_broken_extractor_does_not_stop_the_pilot(repo, chat_bot):
    client = wire(repo, Handling(False, "the bot was not mentioned"))
    client.extractor = Exploding()
    msg = dated(message(chat_bot, "can wed?", channel_id=WATCHED_CHANNEL, mentions=()))
    deliver(client, msg)  # must not raise
    assert client.chat.seen == [msg]


# ---------------------------------------------------------------------------
# the gate is consulted once
# ---------------------------------------------------------------------------


@pytest.mark.anyio
async def test_one_message_spends_exactly_one_rate_limit_allowance(chat_bot, chat_seeded):
    """`decide()` records an allowance every time it is asked.

    If `on_message` re-derived "was that handled?" by calling the gate again,
    each message would cost two of the four answers a person gets.
    """
    from bot.chat.agent import ChatPilot

    from .chat_support import FakeOllama, says

    agent = ChatPilot(chat_bot, client=FakeOllama(says("ok")))
    before = agent.limiter.remaining(1002)
    handling = await agent.offer(message(chat_bot, author_id=1002))

    assert handling.handled is True
    assert agent.limiter.remaining(1002) == before - 1


@pytest.fixture
def anyio_backend():
    return "asyncio"
