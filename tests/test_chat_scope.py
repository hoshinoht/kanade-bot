"""Answering "what runs are in *this* channel?" honestly.

Live, that question got the whole group's week and the model relabelled it "in
this channel" -- `get_schedule` had no channel dimension, so the only thing it
could do was parrot the asker's framing back over guild-wide data.
"""

from __future__ import annotations

import pytest

from bot.chat import tools
from bot.domain.ids import short_id

from .chat_support import CHAT_CHANNEL
from .fake_bot import OTHER_CHANNEL, WATCHED_CHANNEL

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def context(bot, channel_id=WATCHED_CHANNEL):
    return tools.ToolContext(
        bot=bot,
        author_id="1002",
        channel_id=str(channel_id),
        message_id="950000000000000666",
    )


async def schedule(bot, channel_id=WATCHED_CHANNEL, **args) -> str:
    return await tools.dispatch(context(bot, channel_id), "get_schedule", {"week": "this", **args})


# ---------------------------------------------------------------------------
# scope="channel"
# ---------------------------------------------------------------------------


async def test_channel_scope_excludes_other_channels_runs(chat_bot, chat_seeded):
    """HMaleficStar lives in the watched channel, XKalos in the other one."""
    answer = await schedule(chat_bot, WATCHED_CHANNEL, scope="channel")
    assert "Hard MaleficStar + Hard FA" in answer
    assert "Extreme Kalos" not in answer
    assert short_id(chat_seeded["kalos"]) not in answer


async def test_channel_scope_from_the_other_channel_sees_the_other_run(chat_bot, chat_seeded):
    answer = await schedule(chat_bot, OTHER_CHANNEL, scope="channel")
    assert "Extreme Kalos" in answer
    assert "HMaleficStar" not in answer


async def test_channel_scope_says_it_is_channel_only(chat_bot, chat_seeded):
    assert "in this channel only" in await schedule(chat_bot, WATCHED_CHANNEL, scope="channel")


async def test_a_thread_asks_on_behalf_of_its_parent_channel(chat_bot, chat_seeded):
    """`ToolContext.channel_id` is already parent-resolved by `origin_ids`."""
    from bot.infrastructure.watch import origin_ids

    from .chat_support import thread_in

    thread = thread_in(chat_bot, WATCHED_CHANNEL, thread_id=909000000000000010)
    parent_id, thread_id = origin_ids(thread)
    assert (parent_id, thread_id) == (WATCHED_CHANNEL, thread.id)

    answer = await schedule(chat_bot, parent_id, scope="channel")
    assert "Hard MaleficStar + Hard FA" in answer
    assert "Extreme Kalos" not in answer


# ---------------------------------------------------------------------------
# an empty channel is not an empty guild
# ---------------------------------------------------------------------------


async def test_an_empty_channel_offers_the_whole_group_instead(chat_bot, chat_seeded):
    """The pilot's own channel has no runs; the group's week is not empty."""
    answer = await schedule(chat_bot, CHAT_CHANNEL, scope="channel")
    assert "No runs are scheduled in this channel" in answer
    assert "check all channels before answering" in answer
    assert "2 runs in other channels" in answer
    for internal in ("scope=", "get_schedule", "participant="):
        assert internal not in answer


async def test_an_empty_channel_in_an_empty_week_does_not_promise_runs(chat_bot, chat_seeded):
    answer = await schedule(chat_bot, CHAT_CHANNEL, week="next", scope="channel")
    assert "No runs are scheduled in this channel" in answer
    assert "check all channels" not in answer


# ---------------------------------------------------------------------------
# scope="all"
# ---------------------------------------------------------------------------


async def test_all_scope_is_the_default_and_unchanged(chat_bot, chat_seeded):
    answer = await schedule(chat_bot, WATCHED_CHANNEL)
    assert "Hard MaleficStar + Hard FA" in answer
    assert "Extreme Kalos" in answer


async def test_all_scope_links_the_channel_each_run_lives_in(chat_bot, chat_seeded):
    """So a cross-channel answer is both explicit and directly navigable."""
    answer = await schedule(chat_bot, WATCHED_CHANNEL, scope="all")
    assert f"<#{WATCHED_CHANNEL}>" in answer
    assert f"<#{OTHER_CHANNEL}>" in answer


async def test_all_scope_labels_itself_as_every_channel(chat_bot, chat_seeded):
    assert "ALL channels" in await schedule(chat_bot, WATCHED_CHANNEL, scope="all")


async def test_channel_scope_does_not_repeat_the_channel_on_every_line(chat_bot, chat_seeded):
    """They already know which channel they are in; it is noise there."""
    assert f"<#{WATCHED_CHANNEL}>" not in await schedule(chat_bot, WATCHED_CHANNEL, scope="channel")


async def test_a_run_in_a_channel_the_bot_cannot_see_still_lists(chat_bot, chat_seeded):
    """`channel_name` returns None off the gateway; "#None" would be worse."""
    chat_bot.channels.clear()
    answer = await schedule(chat_bot, WATCHED_CHANNEL, scope="all")
    assert "Hard MaleficStar + Hard FA" in answer
    assert "<#" not in answer


# ---------------------------------------------------------------------------
# the argument itself
# ---------------------------------------------------------------------------


async def test_an_unknown_scope_is_refused(chat_bot, chat_seeded):
    answer = await schedule(chat_bot, WATCHED_CHANNEL, scope="mine")
    assert "this channel or all channels" in answer
    assert "'all'" in answer and "'channel'" in answer


async def test_the_description_steers_the_model(chat_bot):
    schema = next(t for t in tools.TOOLS if t["function"]["name"] == "get_schedule")
    properties = schema["function"]["parameters"]["properties"]
    scope_description = properties["scope"]["description"]
    for phrase in ("only", "this channel", "here", "our runs", "Bare dates"):
        assert phrase in scope_description
    assert set(properties["scope"]["enum"]) == {
        "all",
        "channel",
    }
    participant = properties["participant"]
    assert "enum" not in participant
    for phrase in ("'for me'", "'my runs'", "named member", "omit it"):
        assert phrase in participant["description"]
    day = properties["day"]
    for phrase in ("'today'", "'tonight'", "'tomorrow'", "weekday"):
        assert phrase in day["description"]
    # `week` stays required; `scope` is optional and defaults to today's behaviour.
    assert schema["function"]["parameters"]["required"] == ["week"]
