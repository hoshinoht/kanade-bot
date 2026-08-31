"""Posting survives a network that blinks (item 10).

The first automated morning rescan died on a ``socket.gaierror`` raised inside
``channel.send``: ``_post`` only caught ``discord.HTTPException``, so the DNS
failure went straight up through the extraction and killed the whole job. It
left proposal rows written but cardless -- ``proposed`` with no
``proposal_message_id`` -- which nobody could see and nobody could ✅.

Failing to *reach* Discord is worth retrying; being *refused* by Discord is not.
"""

from __future__ import annotations

import asyncio
import socket
from types import SimpleNamespace

import discord
import pytest

from bot.client import POST_ATTEMPTS, BossBot


class FakeMessage:
    def __init__(self):
        self.reactions: list[str] = []
        self.id = 900000000000000123

    async def add_reaction(self, emoji):
        self.reactions.append(str(emoji))


class FakeChannel:
    """Sends whatever ``script`` says: an exception is raised, anything else returned."""

    def __init__(self, *script):
        self.script = list(script)
        self.attempts = 0

    async def send(self, *args, **kwargs):
        self.attempts += 1
        step = self.script.pop(0) if self.script else FakeMessage()
        if isinstance(step, Exception):
            raise step
        return step


@pytest.fixture
def bot(repo):
    """A client with none of its setup -- ``_post`` needs only its own helpers.

    The repository is real because ``_post`` reads the quiet-mode flag off it;
    left unset, every test here would fail on the wrong thing.
    """
    client = BossBot.__new__(BossBot)
    client.repo = repo
    return client


@pytest.fixture
def no_waiting(monkeypatch):
    """Run the backoff instantly, and record how long it would have waited."""
    waits: list[float] = []

    async def fake_sleep(seconds):
        waits.append(seconds)

    monkeypatch.setattr(asyncio, "sleep", fake_sleep)
    return waits


def http_error(status=500):
    return discord.HTTPException(SimpleNamespace(status=status, reason="nope"), "refused")


def test_a_dns_failure_is_retried_and_the_card_still_lands(bot, no_waiting):
    channel = FakeChannel(socket.gaierror("Name or service not known"), FakeMessage())
    message = asyncio.run(bot._post(channel, "a card"))
    assert message is not None
    assert channel.attempts == 2
    assert no_waiting == [1.0]


def test_the_backoff_doubles(bot, no_waiting):
    channel = FakeChannel(OSError("down"), OSError("still down"), FakeMessage())
    assert asyncio.run(bot._post(channel, "a card")) is not None
    assert no_waiting == [1.0, 2.0]


def test_a_network_that_never_comes_back_gives_up_quietly(bot, no_waiting):
    channel = FakeChannel(*[OSError("down")] * POST_ATTEMPTS)
    assert asyncio.run(bot._post(channel, "a card")) is None
    assert channel.attempts == POST_ATTEMPTS


def test_a_timeout_is_retried_too(bot, no_waiting):
    channel = FakeChannel(TimeoutError(), FakeMessage())
    assert asyncio.run(bot._post(channel, "a card")) is not None
    assert channel.attempts == 2


def test_a_refusal_from_discord_is_never_retried(bot, no_waiting):
    """A 403 or a 400 is the same answer however many times it is asked."""
    channel = FakeChannel(http_error(403), FakeMessage())
    assert asyncio.run(bot._post(channel, "a card")) is None
    assert channel.attempts == 1
    assert no_waiting == []


def test_a_posted_card_is_not_lost_when_its_reactions_fail(bot, no_waiting):
    """The message is up and its id must be recorded; a missing ✅ is cosmetic."""

    class NoReactions(FakeMessage):
        async def add_reaction(self, emoji):
            raise OSError("dropped")

    channel = FakeChannel(NoReactions())
    message = asyncio.run(bot._post(channel, "a card"))
    assert message is not None
    assert channel.attempts == 1


def test_a_good_post_still_gets_both_reactions(bot, no_waiting):
    from bot.rsvp import EMOJI_NO, EMOJI_YES

    channel = FakeChannel(FakeMessage())
    message = asyncio.run(bot._post(channel, "a card"))
    assert message.reactions == [EMOJI_YES, EMOJI_NO]
    assert no_waiting == []
