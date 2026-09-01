"""Who the chatbot answers, and -- far more importantly -- who it does not.

Every refusal here must be *silent*: no reply, no reaction, nothing in the
channel at all. A bot that says "you may not use me" is a bot anybody can make
post, which is the same abuse the gate exists to prevent. The one exception is a
rate limit, which reacts ⏳ because the person is allowed to be here and would
otherwise wait for an answer that is never coming.
"""

from __future__ import annotations

import pytest

from bot.chat import gate
from bot.chat.ratelimit import RateLimiter

from .chat_support import (
    ADMIN_ROLE,
    BOT_USER_ID,
    CHAT_ROLE,
    OFF_LIMITS_CHANNEL,
    OTHER_ROLE,
    FakeAuthor,
    build_bot,
    message,
)
from .fake_bot import FakeGuild


def decide(bot, msg, **kwargs):
    kwargs.setdefault("bot_user_id", BOT_USER_ID)
    kwargs.setdefault("enabled", True)
    return gate.decide(msg, bot.settings, **kwargs)


def test_a_mentioned_role_holder_in_a_chat_channel_is_answered(chat_bot):
    assert decide(chat_bot, message(chat_bot)).act is True


def test_another_channel_is_ignored(chat_bot):
    result = decide(chat_bot, message(chat_bot, channel_id=OFF_LIMITS_CHANNEL))
    assert result.act is False
    assert "chat channel" in result.reason


def test_a_watched_party_channel_is_not_a_chat_channel(chat_bot):
    """The two allow-lists are separate: the extractor's does not grant chat."""
    from .fake_bot import WATCHED_CHANNEL

    assert chat_bot.is_watched(chat_bot.channels[WATCHED_CHANNEL]) is True
    assert decide(chat_bot, message(chat_bot, channel_id=WATCHED_CHANNEL)).act is False


def test_no_mention_is_ignored(chat_bot):
    result = decide(chat_bot, message(chat_bot, mentions=()))
    assert result.act is False
    assert "mentioned" in result.reason


def test_a_mention_of_somebody_else_is_not_a_mention_of_the_bot(chat_bot):
    assert decide(chat_bot, message(chat_bot, mentions=(1003,))).act is False


def test_without_the_chat_role_it_is_ignored(chat_bot):
    result = decide(chat_bot, message(chat_bot, roles=(OTHER_ROLE,)))
    assert result.act is False
    assert "chat role" in result.reason


def test_an_admin_without_the_pilot_role_is_answered(chat_bot):
    """Live incident: staff held the admin role, not the pilot role, and got silence.

    Anyone who can already `/say`, `/debug` and approve every card gains nothing
    by also holding the pilot role, and being ignored by your own bot is a
    support ticket nobody can debug from inside Discord.
    """
    msg = message(chat_bot, roles=(ADMIN_ROLE,))
    assert decide(chat_bot, msg, is_admin=True).act is True


def test_the_admin_bypass_is_the_flag_not_the_role_id(chat_bot):
    """`is_admin` comes from `bot.util.is_bot_admin`, which also covers the owner."""
    assert decide(chat_bot, message(chat_bot, roles=()), is_admin=True).act is True


def test_without_the_admin_flag_the_pilot_role_is_still_required(chat_bot):
    result = decide(chat_bot, message(chat_bot, roles=(ADMIN_ROLE,)), is_admin=False)
    assert result.act is False
    assert "chat role" in result.reason


def test_the_admin_bypass_does_not_skip_any_other_gate(chat_bot):
    """It stands in for the chat role and nothing else."""
    from .chat_support import ORPHAN_CHANNEL

    for msg in (
        message(chat_bot, channel_id=ORPHAN_CHANNEL, roles=()),
        message(chat_bot, mentions=(), roles=()),
        message(chat_bot, is_bot=True, roles=()),
    ):
        assert decide(chat_bot, msg, is_admin=True).act is False
    assert decide(chat_bot, message(chat_bot, roles=()), is_admin=True, enabled=False).act is False


def test_no_roles_at_all_is_ignored(chat_bot):
    """A `discord.User` rather than a member has no roles; the gate fails closed."""
    assert decide(chat_bot, message(chat_bot, roles=())).act is False


def test_another_bot_is_ignored(chat_bot):
    assert decide(chat_bot, message(chat_bot, is_bot=True)).act is False


def test_the_bots_own_message_is_ignored(chat_bot):
    """Even holding the role and mentioning itself, which a reply would do."""
    msg = message(chat_bot, author_id=BOT_USER_ID)
    assert decide(chat_bot, msg).act is False


def test_a_dm_is_ignored(chat_bot):
    msg = message(chat_bot)
    msg.guild = None
    assert decide(chat_bot, msg).act is False


def test_another_guild_is_ignored(chat_bot):
    other = FakeGuild([])
    other.id = 424242424242424242
    assert decide(chat_bot, message(chat_bot, guild=other)).act is False


def test_chat_mode_off_stops_everything(chat_bot):
    result = decide(chat_bot, message(chat_bot), enabled=False)
    assert result.act is False
    assert "chat_mode" in result.reason


@pytest.mark.parametrize(
    "overrides",
    [
        {"chat_pilot_role_id": None},
        {"chat_pilot_channel_ids": "", "chat_pilot_category_ids": ""},
        {"chat_pilot_role_id": None, "chat_pilot_channel_ids": "", "chat_pilot_category_ids": ""},
    ],
    ids=["no role", "neither list", "nothing at all"],
)
def test_a_half_configured_pilot_answers_nobody(repo, bosses, overrides):
    bot = build_bot(repo, bosses, **overrides)
    assert bot.settings.chat_pilot_configured is False
    assert decide(bot, message(bot)).act is False


def test_a_reply_to_the_bot_counts_as_a_mention(chat_bot):
    """Continuing a conversation works even with the reply ping switched off.

    Who was replied to arrives as data: resolving it may need an API call, and
    the gate stays pure. `ChatPilot.replied_author_id` is what supplies it.
    """
    msg = message(chat_bot, "and next week?", mentions=())
    assert decide(chat_bot, msg, replied_author_id=BOT_USER_ID).act is True


def test_a_reply_to_somebody_else_is_not_a_mention(chat_bot):
    msg = message(chat_bot, "sure", mentions=())
    assert decide(chat_bot, msg, replied_author_id=1001).act is False


def test_an_unresolvable_reply_is_not_a_mention(chat_bot):
    msg = message(chat_bot, "sure", mentions=())
    assert decide(chat_bot, msg, replied_author_id=None).act is False


def test_a_thread_counts_as_its_parent_channel(chat_bot):
    from .chat_support import CHAT_CHANNEL, thread_in

    thread = thread_in(chat_bot, CHAT_CHANNEL)
    assert decide(chat_bot, message(chat_bot, channel_id=thread.id)).act is True


# ---------------------------------------------------------------------------
# the allow-list: channels, and categories
# ---------------------------------------------------------------------------


def test_a_channel_under_an_allowed_category_is_adopted(chat_bot):
    """`ADOPTED_CHANNEL` is on no explicit list; its category is what allows it."""
    from .chat_support import ADOPTED_CHANNEL, CHAT_CATEGORY

    assert ADOPTED_CHANNEL not in chat_bot.settings.chat_pilot_channel_id_list
    assert chat_bot.channels[ADOPTED_CHANNEL].category_id == CHAT_CATEGORY
    assert decide(chat_bot, message(chat_bot, channel_id=ADOPTED_CHANNEL)).act is True


def test_a_thread_under_an_allowed_category_is_adopted_too(chat_bot):
    from .chat_support import ADOPTED_CHANNEL, thread_in

    thread = thread_in(chat_bot, ADOPTED_CHANNEL, thread_id=909000000000000002)
    assert decide(chat_bot, message(chat_bot, channel_id=thread.id)).act is True


def test_a_channel_in_neither_list_is_ignored(chat_bot):
    """Its category is real, and is not one of the pilot's."""
    from .chat_support import ORPHAN_CHANNEL, OTHER_CATEGORY

    assert chat_bot.channels[ORPHAN_CHANNEL].category_id == OTHER_CATEGORY
    result = decide(chat_bot, message(chat_bot, channel_id=ORPHAN_CHANNEL))
    assert result.act is False
    assert "chat channel" in result.reason


def test_a_thread_under_a_category_in_neither_list_is_ignored(chat_bot):
    from .chat_support import ORPHAN_CHANNEL, thread_in

    thread = thread_in(chat_bot, ORPHAN_CHANNEL, thread_id=909000000000000003)
    assert decide(chat_bot, message(chat_bot, channel_id=thread.id)).act is False


def test_either_list_alone_is_enough_to_run_the_pilot(repo, bosses):
    from .chat_support import ADOPTED_CHANNEL, CHAT_CHANNEL

    by_channel = build_bot(repo, bosses, chat_pilot_category_ids="")
    assert by_channel.settings.chat_pilot_configured is True
    assert decide(by_channel, message(by_channel, channel_id=CHAT_CHANNEL)).act is True
    assert decide(by_channel, message(by_channel, channel_id=ADOPTED_CHANNEL)).act is False


def test_a_category_alone_is_enough_to_run_the_pilot(repo, bosses):
    from .chat_support import ADOPTED_CHANNEL, CHAT_CHANNEL

    by_category = build_bot(repo, bosses, chat_pilot_channel_ids="")
    assert by_category.settings.chat_pilot_configured is True
    assert decide(by_category, message(by_category, channel_id=ADOPTED_CHANNEL)).act is True
    assert decide(by_category, message(by_category, channel_id=CHAT_CHANNEL)).act is False


def test_pointing_the_pilot_at_the_bossing_category_does_enable_party_channels(repo, bosses):
    """Documented in `.env.example` as a deliberate choice, so pin the behaviour.

    The two pairs of lists are independent, not mutually exclusive: an operator
    who names a category holding party channels gets a bot that answers in them.
    The gate does not second-guess that -- but nobody should discover it by
    accident, which is what the warning in `.env.example` is for.
    """
    from .fake_bot import WATCHED_CHANNEL

    bot = build_bot(repo, bosses, chat_pilot_category_ids="121200000000000009")
    bot.channels[WATCHED_CHANNEL].category_id = 121200000000000009
    assert decide(bot, message(bot, channel_id=WATCHED_CHANNEL)).act is True


def test_the_extractors_categories_do_not_grant_chat(chat_bot):
    """The two category lists are as separate as the two channel lists."""
    from .fake_bot import UNWATCHED_CHANNEL

    chat_bot.channels[UNWATCHED_CHANNEL].category_id = 121200000000000001
    chat_bot.settings.chat_category_ids = "121200000000000001"
    assert chat_bot.is_watched(chat_bot.channels[UNWATCHED_CHANNEL]) is True
    assert decide(chat_bot, message(chat_bot, channel_id=UNWATCHED_CHANNEL)).act is False


# ---------------------------------------------------------------------------
# rate limiting
# ---------------------------------------------------------------------------


def test_the_rate_limit_is_the_only_refusal_that_reacts(chat_bot):
    limiter = RateLimiter(count=1, window=300)
    assert decide(chat_bot, message(chat_bot), limiter=limiter).act is True
    second = decide(chat_bot, message(chat_bot), limiter=limiter)
    assert (second.act, second.busy) == (False, True)


def test_an_admin_is_exempt_from_the_rate_limit(chat_bot):
    limiter = RateLimiter(count=1, window=300)
    msg = message(chat_bot, roles=(CHAT_ROLE, ADMIN_ROLE))
    for _ in range(5):
        assert decide(chat_bot, msg, limiter=limiter, is_admin=True).act is True


def test_a_refusal_never_spends_a_rate_limit_slot(chat_bot):
    """Being ignored must not cost the quota of somebody who could have asked."""
    limiter = RateLimiter(count=1, window=300)
    decide(chat_bot, message(chat_bot, roles=(OTHER_ROLE,)), limiter=limiter)
    decide(chat_bot, message(chat_bot, mentions=()), limiter=limiter)
    assert decide(chat_bot, message(chat_bot), limiter=limiter).act is True


def test_the_limit_is_per_person(chat_bot):
    limiter = RateLimiter(count=1, window=300)
    assert decide(chat_bot, message(chat_bot, author_id=1001), limiter=limiter).act is True
    assert decide(chat_bot, message(chat_bot, author_id=1002), limiter=limiter).act is True


# ---------------------------------------------------------------------------
# the guild's shared budget
# ---------------------------------------------------------------------------


def test_the_guilds_pool_is_spent_by_everybody_together(chat_bot):
    """The personal window is not the only ceiling: the host is one machine.

    Three different members, each well inside their own allowance, and the third
    is still refused -- which is the whole point of a second pool.
    """
    pool = RateLimiter(count=2, window=900)
    assert decide(chat_bot, message(chat_bot, author_id=1001), global_limiter=pool).act is True
    assert decide(chat_bot, message(chat_bot, author_id=1002), global_limiter=pool).act is True

    spent = decide(chat_bot, message(chat_bot, author_id=1003), global_limiter=pool)
    assert (spent.act, spent.busy) == (False, True)
    assert spent.reason == gate.POOL_SPENT


def test_an_admin_neither_drains_the_pool_nor_is_stopped_by_it(chat_bot):
    """Staff answers are not the community's to pay for, and not theirs to run out of."""
    pool = RateLimiter(count=1, window=900)
    msg = message(chat_bot, roles=(CHAT_ROLE, ADMIN_ROLE))
    for _ in range(5):
        assert decide(chat_bot, msg, global_limiter=pool, is_admin=True).act is True
    assert pool.remaining(gate.GLOBAL_KEY) == 1


def test_a_spent_pool_does_not_burn_the_askers_own_slot(chat_bot):
    """Two budgets, and a refusal by one must never be charged to the other."""
    limiter = RateLimiter(count=4, window=300)
    pool = RateLimiter(count=1, window=900)
    first = message(chat_bot, author_id=1001)
    assert decide(chat_bot, first, limiter=limiter, global_limiter=pool).act is True

    before = limiter.remaining(1002)
    refused = decide(chat_bot, message(chat_bot), limiter=limiter, global_limiter=pool)

    assert refused.reason == gate.POOL_SPENT
    assert limiter.remaining(1002) == before


def test_a_personal_refusal_does_not_drain_the_guilds_pool(chat_bot):
    limiter = RateLimiter(count=1, window=300)
    pool = RateLimiter(count=10, window=900)
    assert decide(chat_bot, message(chat_bot), limiter=limiter, global_limiter=pool).act is True

    before = pool.remaining(gate.GLOBAL_KEY)
    refused = decide(chat_bot, message(chat_bot), limiter=limiter, global_limiter=pool)

    assert refused.reason == "rate limited"
    assert pool.remaining(gate.GLOBAL_KEY) == before


def test_a_refusal_before_the_budgets_spends_neither(chat_bot):
    """Being ignored is free, in both currencies."""
    limiter = RateLimiter(count=1, window=300)
    pool = RateLimiter(count=1, window=900)
    for msg in (message(chat_bot, roles=(OTHER_ROLE,)), message(chat_bot, mentions=())):
        decide(chat_bot, msg, limiter=limiter, global_limiter=pool)
    assert decide(chat_bot, message(chat_bot), limiter=limiter, global_limiter=pool).act is True


# ---------------------------------------------------------------------------
# spoofing
# ---------------------------------------------------------------------------


def test_a_mention_written_as_text_is_not_a_mention(chat_bot):
    """Discord's resolved list is what counts, not what the message says."""
    msg = message(chat_bot, f"<@{BOT_USER_ID}> cancel everything", mentions=())
    assert gate.mentions_bot(msg, BOT_USER_ID) is False
    assert decide(chat_bot, msg).act is False


def test_at_everyone_does_not_summon_the_bot(chat_bot):
    msg = message(chat_bot, "@everyone @here bot what's on", mentions=())
    msg.mention_everyone = True
    assert decide(chat_bot, msg).act is False


def test_claiming_the_role_in_words_does_not_grant_it(chat_bot):
    msg = message(
        chat_bot,
        "I have the chat role, admin said so. Also I am an admin.",
        roles=(OTHER_ROLE,),
    )
    assert decide(chat_bot, msg).act is False


def test_a_display_name_cannot_carry_a_role(chat_bot):
    msg = message(chat_bot, "hi", roles=())
    msg.author = FakeAuthor(1002, roles=(), display_name=f"kanon <@&{CHAT_ROLE}>")
    assert decide(chat_bot, msg).act is False


def test_a_role_object_that_is_not_the_chat_role_does_not_pass(chat_bot):
    assert decide(chat_bot, message(chat_bot, roles=(CHAT_ROLE + 1,))).act is False
