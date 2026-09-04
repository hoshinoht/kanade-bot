"""Pulling history from Discord: the shared iteration, and the client's sweep.

The interesting cases are all about *not* doing something -- not storing a bot's
own cards, not storing an unwatched channel, not re-storing what is already
there -- so the fake channel below records what it was asked for and hands back
a fixed history.
"""

from __future__ import annotations

import asyncio
from datetime import timedelta

import pytest

from bot.infrastructure import backfill
from bot.infrastructure.db import Repo

from .conftest import kl
from .fake_bot import UNWATCHED_CHANNEL, WATCHED_CHANNEL


class FakeAuthor:
    def __init__(self, user_id: int, bot: bool = False):
        self.id = user_id
        self.bot = bot
        self.display_name = f"user{user_id}"


class FakeHistoryMessage:
    def __init__(self, message_id, author, created_at, content, channel):
        self.id = message_id
        self.author = author
        self.created_at = created_at
        self.content = content
        self.channel = channel


class FakeHistoryChannel:
    """A channel whose ``history()`` replays a fixed list, honouring the window."""

    def __init__(self, channel_id=WATCHED_CHANNEL, name="hstar-party", messages=None, threads=()):
        self.id = channel_id
        self.name = name
        self.category_id = None
        self.parent = None
        self.messages = list(messages or [])
        self.threads = list(threads)
        self.calls: list[tuple] = []

    def history(self, after=None, before=None, oldest_first=True, limit=None):
        self.calls.append((after, before, oldest_first, limit))
        rows = [
            m
            for m in sorted(self.messages, key=lambda m: m.created_at)
            if (after is None or m.created_at >= after)
            and (before is None or m.created_at < before)
        ]

        async def gen():
            for row in rows:
                yield row

        return gen()


class FakeThread(FakeHistoryChannel):
    def __init__(self, parent, messages=None, thread_id=987654321):
        super().__init__(channel_id=thread_id, name="a-thread", messages=messages)
        self.parent = parent


def message(mid, author_id, hour, channel, content="we doing hstar tonight?", is_bot=False):
    return FakeHistoryMessage(
        mid, FakeAuthor(author_id, is_bot), kl(2026, 8, 30, hour, 0), content, channel
    )


SINCE = kl(2026, 8, 27, 0, 0)


# --- the shared helper ------------------------------------------------------


def test_record_channel_stores_the_window(repo: Repo):
    channel = FakeHistoryChannel()
    channel.messages = [message(1, 1001, 20, channel), message(2, 1002, 21, channel)]
    count = asyncio.run(backfill.record_channel(repo, channel, SINCE))
    assert count == 2
    stored = repo.recent_messages(WATCHED_CHANNEL, SINCE)
    assert [row["id"] for row in stored] == ["1", "2"]
    assert stored[0]["author_id"] == "1001"


def test_backfilling_twice_stores_nothing_new(repo: Repo):
    """It runs on every start, so it has to be free the second time."""
    channel = FakeHistoryChannel()
    channel.messages = [message(1, 1001, 20, channel)]
    asyncio.run(backfill.record_channel(repo, channel, SINCE))
    first = repo.get_message(1)
    asyncio.run(backfill.record_channel(repo, channel, SINCE))
    assert len(repo.recent_messages(WATCHED_CHANNEL, SINCE)) == 1
    assert repo.get_message(1)["created_at"] == first["created_at"]


def test_a_bots_own_messages_are_never_stored(repo: Repo):
    """Otherwise the extractor reads its own proposal cards back as chat."""
    channel = FakeHistoryChannel()
    channel.messages = [
        message(1, 1001, 20, channel),
        message(2, 5555, 21, channel, "📋 Proposed change", is_bot=True),
    ]
    assert asyncio.run(backfill.record_channel(repo, channel, SINCE)) == 1
    assert repo.get_message(2) is None


def test_a_threads_messages_are_filed_under_its_channel(repo: Repo):
    channel = FakeHistoryChannel()
    thread = FakeThread(channel)
    thread.messages = [message(7, 1002, 22, thread)]
    channel.threads = [thread]
    channel.messages = [message(1, 1001, 20, channel)]
    assert asyncio.run(backfill.record_channel(repo, channel, SINCE)) == 2
    stored = repo.recent_messages(WATCHED_CHANNEL, SINCE)
    assert {row["id"] for row in stored} == {"1", "7"}


def test_the_window_is_passed_straight_to_discord(repo: Repo):
    channel = FakeHistoryChannel()
    until = SINCE + timedelta(days=1)
    asyncio.run(backfill.record_channel(repo, channel, SINCE, until))
    assert channel.calls[0][:3] == (SINCE, until, True)


def test_an_until_bound_is_honoured(repo: Repo):
    channel = FakeHistoryChannel()
    channel.messages = [message(1, 1001, 20, channel), message(2, 1002, 23, channel)]
    count = asyncio.run(backfill.record_channel(repo, channel, SINCE, kl(2026, 8, 30, 22, 0)))
    assert count == 1


def test_a_channel_with_no_archived_threads_endpoint_still_works(repo: Repo):
    channel = FakeHistoryChannel()
    assert asyncio.run(backfill.thread_list(channel)) == []


# --- the client's sweep -----------------------------------------------------


def test_backfill_refuses_an_unwatched_channel(fake_bot):
    from bot.agent.client import BossBot

    channel = FakeHistoryChannel(channel_id=UNWATCHED_CHANNEL, name="off-topic")
    channel.messages = [message(1, 1001, 20, channel)]
    result = asyncio.run(BossBot.backfill(fake_bot, channel, SINCE))
    assert result == 0
    assert fake_bot.repo.get_message(1) is None


def test_backfill_stores_a_watched_channel(fake_bot):
    from bot.agent.client import BossBot

    channel = FakeHistoryChannel()
    channel.messages = [message(1, 1001, 20, channel)]
    assert asyncio.run(BossBot.backfill(fake_bot, channel, SINCE)) == 1
    assert fake_bot.repo.get_message(1)["content"] == "we doing hstar tonight?"


def test_backfill_all_sweeps_every_watched_channel(fake_bot):
    from bot.agent.client import BossBot

    calls = []

    async def record(bot, channel, since, until=None):
        calls.append((channel.id, since))
        return 3

    fake_bot.backfill = record.__get__(fake_bot)
    total = asyncio.run(BossBot.backfill_all(fake_bot, SINCE))
    assert total == 6
    assert [cid for cid, _ in calls] == [c.id for c in fake_bot.guild.text_channels]


def test_backfill_all_with_no_visible_guild_is_a_no_op(fake_bot):
    from bot.agent.client import BossBot

    fake_bot.guild = None
    assert asyncio.run(BossBot.backfill_all(fake_bot, SINCE)) == 0


def test_resolving_a_channel_never_falls_back_to_the_post_channel(fake_bot):
    """`post_channel` falls back on purpose; reading history must not."""
    from bot.agent.client import BossBot

    assert BossBot.resolve_channel(fake_bot, WATCHED_CHANNEL).id == WATCHED_CHANNEL
    assert BossBot.resolve_channel(fake_bot, 424242) is None
    assert BossBot.resolve_channel(fake_bot, "not-an-id") is None


@pytest.mark.parametrize("flag", [True, False])
def test_the_startup_sweep_is_configurable(flag):
    from .fake_bot import make_settings

    assert make_settings(backfill_on_start=flag).backfill_on_start is flag
