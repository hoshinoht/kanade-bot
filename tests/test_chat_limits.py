"""`/limits`: what one member has left of their own chat allowance.

Two properties matter more than the wording. The command must *consume*
nothing -- a member checking how many answers are left, and thereby spending
one, is a joke the bot only gets to make once -- and it must show the allowance
that person is actually on, overrides included, because being confidently wrong
about somebody's own case is worse than saying nothing.

The clock is fake throughout, for the reason `test_chat_ratelimit` gives: the
window is five minutes in production and a test that waits it out is a test
nobody runs.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

from bot.chat.gate import GLOBAL_KEY
from bot.chat.ratelimit import RateLimiter
from bot.commands import (
    BAR_EMPTY,
    BAR_FILLED,
    BAR_SEGMENTS,
    STAFF_LIMITS_REPLY,
    limits,
    register_commands,
    usage_bar,
)

from .chat_support import ADMIN_ROLE, CHAT_CHANNEL, CHAT_ROLE, FakeAuthor
from .fake_bot import OWNER_ID
from .test_chat_ratelimit import Clock

MEMBER_ID = 1002


class FakeInteraction:
    """Only what `/limits` reaches for."""

    def __init__(self, bot, user=None):
        self.client = bot
        self.user = user if user is not None else FakeAuthor(MEMBER_ID)
        self.guild = bot.guild
        self.channel = bot.channels[CHAT_CHANNEL]
        self.channel_id = CHAT_CHANNEL
        self.sent: list[tuple[str, bool]] = []
        self.response = SimpleNamespace(send_message=self._send)

    async def _send(self, content: str, ephemeral: bool = False) -> None:
        self.sent.append((content, ephemeral))


def run_limits(bot, user=None) -> FakeInteraction:
    interaction = FakeInteraction(bot, user=user)
    asyncio.run(limits.callback(interaction))
    return interaction


def reply(bot, user=None) -> str:
    return run_limits(bot, user=user).sent[0][0]


def personal(bot, count: int = 5, window: float = 300.0) -> Clock:
    """Put the pilot's per-person limiter on a clock a test can wind forward."""
    clock = Clock()
    bot.chat.limiter = RateLimiter(count, window, clock)
    return clock


# --- staff ------------------------------------------------------------------


def test_staff_are_told_they_have_no_limits(chat_bot):
    """They are exempt before the limiter is consulted, so there is no bar to draw."""
    said = reply(chat_bot, FakeAuthor(MEMBER_ID, roles=(CHAT_ROLE, ADMIN_ROLE)))

    assert said == STAFF_LIMITS_REPLY
    assert BAR_FILLED not in said and BAR_EMPTY not in said


def test_a_server_admin_is_staff_here_too(chat_bot):
    said = reply(chat_bot, FakeAuthor(MEMBER_ID, administrator=True))
    assert said == STAFF_LIMITS_REPLY


def test_the_guild_owner_is_staff_here_too(chat_bot):
    assert reply(chat_bot, FakeAuthor(OWNER_ID)) == STAFF_LIMITS_REPLY


def test_the_staff_line_says_no_numbers_at_all(chat_bot):
    """Their window is empty and stays empty; quoting one would invent a limit."""
    personal(chat_bot, count=5)
    said = reply(chat_bot, FakeAuthor(MEMBER_ID, roles=(CHAT_ROLE, ADMIN_ROLE)))

    assert "5" not in said and "used" not in said


# --- the bar ----------------------------------------------------------------


def test_an_untouched_window_draws_an_empty_bar(chat_bot):
    personal(chat_bot, count=5)
    said = reply(chat_bot)

    assert usage_bar(0, 5) in said
    assert BAR_FILLED not in said
    assert "0 of 5 used" in said
    assert "whole allowance" in said


def test_a_partly_used_window_shows_what_is_left_and_when_one_comes_back(chat_bot):
    clock = personal(chat_bot, count=5, window=300)
    chat_bot.chat.limiter.allow(MEMBER_ID)
    chat_bot.chat.limiter.allow(MEMBER_ID)
    clock.advance(60)

    said = reply(chat_bot)

    assert "2 of 5 used" in said
    assert usage_bar(2, 5) in said
    # The oldest hit is 60 s into a 300 s window, so its slot is back in 4 min.
    assert "3 left — a spent one comes back in about 4 min." in said


def test_a_full_window_says_when_the_next_answer_is(chat_bot):
    clock = personal(chat_bot, count=2, window=300)
    chat_bot.chat.limiter.allow(MEMBER_ID)
    chat_bot.chat.limiter.allow(MEMBER_ID)
    clock.advance(250)

    said = reply(chat_bot)

    assert "2 of 2 used" in said
    assert said.count(BAR_FILLED) == BAR_SEGMENTS
    assert BAR_EMPTY not in said
    # Under two minutes, so `retry_note` counts it in seconds.
    assert "None left — ask me again in about 50s." in said


def test_the_bar_lights_a_segment_for_a_single_answer(chat_bot):
    """Rounded up: a bar reading as untouched when it is not is the one lie here."""
    personal(chat_bot, count=100)
    chat_bot.chat.limiter.allow(MEMBER_ID)

    assert usage_bar(1, 100).startswith(BAR_FILLED)
    assert BAR_FILLED in reply(chat_bot)


def test_the_bar_only_fills_completely_when_the_window_is():
    assert usage_bar(11, 12).count(BAR_EMPTY) == 1
    assert usage_bar(12, 12) == BAR_FILLED * BAR_SEGMENTS
    assert usage_bar(0, 12) == BAR_EMPTY * BAR_SEGMENTS


def test_the_bar_is_always_the_same_width():
    for used in range(8):
        assert len(usage_bar(used, 7)) == BAR_SEGMENTS


# --- whose allowance --------------------------------------------------------


def test_an_overridden_member_is_shown_their_own_numbers(chat_bot):
    """The guild's default is 5; this one has been granted 9."""
    personal(chat_bot, count=5, window=300)
    chat_bot.chat.limiter.set_override(MEMBER_ID, 9, 600)
    chat_bot.chat.limiter.allow(MEMBER_ID)

    said = reply(chat_bot)

    assert "1 of 9 used" in said
    assert "8 left — a spent one comes back in about 10 min." in said


def test_the_member_next_to_them_still_sees_the_guild_default(chat_bot):
    personal(chat_bot, count=5, window=300)
    chat_bot.chat.limiter.set_override(MEMBER_ID, 9, 600)

    said = reply(chat_bot, FakeAuthor(1003))

    assert "0 of 5 used" in said


# --- the guild's shared pool ------------------------------------------------


def test_a_spent_pool_is_said_as_well(chat_bot):
    """Their own bar being green is a lie of omission if the pool would refuse them."""
    personal(chat_bot, count=5, window=300)
    clock = Clock()
    chat_bot.chat.global_limiter = RateLimiter(1, 600, clock)
    chat_bot.chat.global_limiter.allow(GLOBAL_KEY)

    said = reply(chat_bot)

    assert "0 of 5 used" in said, "their own allowance is untouched"
    assert said.endswith(
        "-# The guild's shared pool is spent too, so nobody is being answered for about 10 min."
    )


def test_a_pool_with_room_is_not_mentioned(chat_bot):
    personal(chat_bot, count=5)
    chat_bot.chat.global_limiter = RateLimiter(2, 600, Clock())
    chat_bot.chat.global_limiter.allow(GLOBAL_KEY)

    assert "shared pool" not in reply(chat_bot)


# --- what it costs ----------------------------------------------------------


def test_asking_spends_nothing(chat_bot):
    """The joke about checking your quota and thereby spending it, only once."""
    personal(chat_bot, count=5, window=300)
    chat_bot.chat.limiter.allow(MEMBER_ID)
    before = chat_bot.chat.limiter.remaining(MEMBER_ID)

    for _ in range(5):
        run_limits(chat_bot)

    assert chat_bot.chat.limiter.remaining(MEMBER_ID) == before == 4


def test_asking_spends_nothing_of_the_shared_pool_either(chat_bot):
    personal(chat_bot, count=5)
    chat_bot.chat.global_limiter = RateLimiter(3, 600, Clock())

    for _ in range(5):
        run_limits(chat_bot)

    assert chat_bot.chat.global_limiter.remaining(GLOBAL_KEY) == 3


def test_staff_asking_spends_nothing_either(chat_bot):
    personal(chat_bot, count=5)

    run_limits(chat_bot, FakeAuthor(MEMBER_ID, roles=(CHAT_ROLE, ADMIN_ROLE)))

    assert chat_bot.chat.limiter.remaining(MEMBER_ID) == 5


# --- how it is delivered ----------------------------------------------------


def test_the_reply_is_ephemeral(chat_bot):
    """Personal, and not something to spray into the party channel."""
    personal(chat_bot, count=5)
    assert run_limits(chat_bot).sent[0][1] is True


def test_the_staff_reply_is_ephemeral_too(chat_bot):
    interaction = run_limits(chat_bot, FakeAuthor(MEMBER_ID, roles=(CHAT_ROLE, ADMIN_ROLE)))
    assert interaction.sent[0][1] is True


def test_it_takes_no_options():
    """Nobody reads anybody else's allowance from here."""
    assert limits.parameters == []


def test_it_needs_the_bossing_role():
    assert limits.checks, "the same gate every other member command is behind"


def test_it_is_registered(chat_bot):
    added = []
    chat_bot.tree = SimpleNamespace(
        add_command=added.append, on_error=None, copy_global_to=lambda **_: None
    )
    register_commands(chat_bot)

    assert any(getattr(c, "name", None) == "limits" for c in added)
