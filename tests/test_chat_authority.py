"""Whose runs the chatbot may draft a card about.

``resolve_run`` searches every channel, and a card is posted in the channel the
question came from -- so before this rule, anybody holding the chat role could
sit in their own channel and raise cards about another party's evenings: pinging
them, and retiring the live cards they were about to press.

Two conditions, one helper (:func:`bot.chat.tools._require_authority`): you are
on the thing you want changed (or own the weekly timing behind it), and you are
asking from the channel it lives in. An admin is exempt from both, which is the
same exemption the gate already makes for the chat role and the rate limit.
"""

from __future__ import annotations

from datetime import timedelta

import pytest

from bot.chat import tools
from bot.domain.ids import short_id
from bot.domain.timeutil import utcnow

from .chat_support import ADOPTED_CHANNEL, CHAT_CHANNEL
from .fake_bot import WATCHED_CHANNEL

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def context(bot, author_id: int | str = 1002, is_admin: bool = False, channel_id=CHAT_CHANNEL):
    return tools.ToolContext(
        bot=bot,
        author_id=str(author_id),
        channel_id=str(channel_id),
        message_id="950000000000000777",
        is_admin=is_admin,
    )


def proposals(bot):
    return bot.repo.list_amendments(status="proposed")


def run_in(bot, chat_seeded, channel_id, participants=("1002",), bosses=("HBellona",)) -> dict:
    """A run of this week homed in ``channel_id``."""
    run_id = bot.repo.create_run(
        week_start=chat_seeded["week_start"],
        bosses=list(bosses),
        run_at=chat_seeded["week_start"] + timedelta(days=3, hours=21),
        participants=list(participants),
        status="planned",
        source="amend",
        channel_id=channel_id,
    )
    return bot.repo.get_run(run_id)


def live_card(bot, run: dict) -> str:
    """A proposal card of the kind a refusal must never retire."""
    amendment_id = bot.repo.create_amendment(
        run["week_start"],
        "move",
        bosses=list(run["bosses"]),
        run_id=run["id"],
        new_datetime=run["datetime"],
        channel_id=run["channel_id"],
    )
    bot.repo.set_amendment_proposal_message(amendment_id, 8001)
    return amendment_id


# ---------------------------------------------------------------------------
# (a) it has to be yours
# ---------------------------------------------------------------------------


async def test_a_move_for_a_run_you_are_not_on_is_refused(chat_bot, chat_seeded):
    run = run_in(chat_bot, chat_seeded, CHAT_CHANNEL, participants=("1001",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_move",
        {"run_query": short_id(run["id"]), "to_when": "sunday 22:00"},
    )
    assert "not theirs to change" in answer
    assert proposals(chat_bot) == []


async def test_a_cancel_for_a_run_you_are_not_on_is_refused(chat_bot, chat_seeded):
    run = run_in(chat_bot, chat_seeded, CHAT_CHANNEL, participants=("1001",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "propose_cancel", {"run_query": short_id(run["id"])}
    )
    assert "not theirs to change" in answer
    assert chat_bot.repo.get_run(run["id"])["status"] == "planned"
    assert proposals(chat_bot) == []


async def test_a_refusal_names_nobody_on_the_run(chat_bot, chat_seeded):
    """The refusal says it is not theirs, not who it belongs to."""
    run = run_in(chat_bot, chat_seeded, CHAT_CHANNEL, participants=("1001",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "propose_cancel", {"run_query": short_id(run["id"])}
    )
    assert "Alvin" not in answer
    assert "Do not name anybody" in answer


async def test_the_owner_of_the_weekly_timing_may_still_ask(chat_bot, chat_seeded):
    """1001 owns the HMaleficStar baseline; the run it produced is theirs to change."""
    star = chat_bot.repo.get_run(chat_seeded["star"])
    chat_bot.repo.set_run_participants(star["id"], ["1002"])
    answer = await tools.dispatch(
        context(chat_bot, author_id=1001), "propose_cancel", {"run_query": short_id(star["id"])}
    )
    assert "✅" in answer


async def test_being_on_the_run_is_enough(chat_bot, chat_seeded):
    run = run_in(chat_bot, chat_seeded, CHAT_CHANNEL, participants=("1002",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "propose_cancel", {"run_query": short_id(run["id"])}
    )
    assert "✅" in answer
    assert len(proposals(chat_bot)) == 1


async def test_an_rsvp_for_a_run_you_are_not_on_is_refused(chat_bot, chat_seeded):
    run = run_in(chat_bot, chat_seeded, CHAT_CHANNEL, participants=("1001",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_rsvp",
        {"run_query": short_id(run["id"]), "answer": "yes"},
    )
    assert "not on run" in answer
    assert proposals(chat_bot) == []


async def test_removing_a_weekly_timing_that_is_not_yours_is_refused(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(1001, ["HBellona"], 3, "21:00", ["1001"], channel_id=CHAT_CHANNEL)
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "propose_remove_fixed", {"query": "hbellona"}
    )
    assert "not theirs to remove" in answer
    assert proposals(chat_bot) == []
    assert [f for f in chat_bot.repo.list_fixed_runs() if "HBellona" in f["bosses"]]


async def test_the_owner_may_remove_a_weekly_timing_they_are_not_on(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(1002, ["HBellona"], 3, "21:00", ["1001"], channel_id=CHAT_CHANNEL)
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "propose_remove_fixed", {"query": "hbellona"}
    )
    assert "✅" in answer


# ---------------------------------------------------------------------------
# (b) and it has to live here
# ---------------------------------------------------------------------------


async def test_a_run_in_another_chat_channel_is_refused_even_to_its_party(chat_bot, chat_seeded):
    """The whole attack: a card about somebody else's channel, posted in yours."""
    run = run_in(chat_bot, chat_seeded, ADOPTED_CHANNEL, participants=("1002",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_move",
        {"run_query": short_id(run["id"]), "to_when": "sunday 22:00"},
    )
    assert f"<#{ADOPTED_CHANNEL}>" in answer
    assert "its own channel" in answer
    assert proposals(chat_bot) == []


@pytest.mark.parametrize(
    ("name", "arguments"),
    [
        ("propose_move", {"to_when": "sunday 22:00"}),
        ("propose_cancel", {}),
        ("propose_rsvp", {"answer": "yes"}),
    ],
)
async def test_every_write_that_targets_a_run_applies_the_rule(
    chat_bot, chat_seeded, name, arguments
):
    run = run_in(chat_bot, chat_seeded, ADOPTED_CHANNEL, participants=("1002",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), name, {"run_query": short_id(run["id"]), **arguments}
    )
    assert f"<#{ADOPTED_CHANNEL}>" in answer
    assert proposals(chat_bot) == []


async def test_a_weekly_timing_in_another_chat_channel_is_refused(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(
        1002, ["HBellona"], 3, "21:00", ["1002"], channel_id=ADOPTED_CHANNEL
    )
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "propose_remove_fixed", {"query": "hbellona"}
    )
    assert f"<#{ADOPTED_CHANNEL}>" in answer
    assert proposals(chat_bot) == []


async def test_asking_from_the_runs_own_channel_is_fine(chat_bot, chat_seeded):
    run = run_in(chat_bot, chat_seeded, ADOPTED_CHANNEL, participants=("1002",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002, channel_id=ADOPTED_CHANNEL),
        "propose_move",
        {"run_query": short_id(run["id"]), "to_when": "sunday 22:00"},
    )
    assert "✅" in answer


async def test_a_run_outside_the_pilots_own_channels_is_left_alone(chat_bot, chat_seeded):
    """A deployment with one dedicated chat channel proposes for other channels' runs.

    "Ask in its own channel" is only advice worth giving where the bot listens,
    so the rule is applied to the pilot's own channels. Everywhere else the
    party test above is the whole gate -- which is why it is the strict one.
    """
    run = run_in(chat_bot, chat_seeded, WATCHED_CHANNEL, participants=("1002",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "propose_cancel", {"run_query": short_id(run["id"])}
    )
    assert "✅" in answer


# ---------------------------------------------------------------------------
# the admin exemption, and what a refusal must not do
# ---------------------------------------------------------------------------


async def test_an_admin_may_ask_about_any_run_from_anywhere(chat_bot, chat_seeded):
    run = run_in(chat_bot, chat_seeded, ADOPTED_CHANNEL, participants=("1001",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1009, is_admin=True),
        "propose_move",
        {"run_query": short_id(run["id"]), "to_when": "sunday 22:00"},
    )
    assert "✅" in answer
    assert len(proposals(chat_bot)) == 1


async def test_a_context_that_says_nothing_about_admin_is_not_one(chat_bot, chat_seeded):
    """The default is the least privilege, so a context built anywhere is safe."""
    assert (
        tools.ToolContext(bot=chat_bot, author_id="1", channel_id="2", message_id="3").is_admin
        is False
    )


async def test_a_refusal_writes_nothing_and_retires_nothing(chat_bot, chat_seeded):
    """The card the attacker would have superseded is still there to press."""
    run = run_in(chat_bot, chat_seeded, ADOPTED_CHANNEL, participants=("1001",))
    theirs = live_card(chat_bot, run)
    before = chat_bot.repo.get_run(run["id"])["datetime"]

    answer = await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_move",
        {"run_query": short_id(run["id"]), "to_when": "sunday 22:00"},
    )

    assert "not theirs to change" in answer
    assert chat_bot.repo.get_amendment(theirs)["status"] == "proposed"
    assert [a["id"] for a in proposals(chat_bot)] == [theirs]
    assert chat_bot.repo.get_run(run["id"])["datetime"] == before
    assert [post for post in chat_bot.posts if post.kind == "card"] == []


# ---------------------------------------------------------------------------
# reads and creations are not affected
# ---------------------------------------------------------------------------


async def test_reading_another_channels_run_is_still_allowed(chat_bot, chat_seeded):
    """The rule is about drafting cards, not about what may be looked up."""
    run = run_in(chat_bot, chat_seeded, ADOPTED_CHANNEL, participants=("1001",))
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "get_run", {"query": short_id(run["id"])}
    )
    assert "Hard Bellona" in answer


async def test_proposing_a_new_run_needs_no_authority(chat_bot, chat_seeded):
    """`propose_add` creates in the channel it was asked in, so there is nobody to wrong."""
    when = (utcnow() + timedelta(days=1)).astimezone(chat_bot.tz).strftime("%Y-%m-%d 21:00")
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "propose_add", {"boss": "HBellona", "when": when}
    )
    assert "✅" in answer
