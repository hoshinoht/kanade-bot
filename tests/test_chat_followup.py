"""The question the bot asks when its own card is ❌'d.

Two halves, and the second is the one that matters. The first is that the
question is asked at all, in persona, with the card's facts in it and the
answer's context left behind for the reply. The second is that it is asked
**once**: a reaction is free to press, and a bot that starts a 30-second
generation every time somebody presses one is a bot that can be made to talk
over a channel by clicking.

So most of what is below is about not speaking -- somebody else's card, somebody
else's ❌, a channel the pilot was never given, quiet mode, the same card twice,
four cards in a row -- and about the one turn in the whole feature where the
write tools do not exist.

Every id here is synthetic; see `tests/chat_support.py`.
"""

from __future__ import annotations

import asyncio
import time
from types import SimpleNamespace

import pytest

from bot.agent.client import BossBot
from bot.agent.rsvp import EMOJI_NO, EMOJI_YES
from bot.chat import followup, tools
from bot.chat.agent import ChatPilot
from bot.domain.ids import short_id
from bot.extract.commit import CommitResult

from .chat_support import (
    BOT_USER_ID,
    CHAT_CHANNEL,
    OFF_LIMITS_CHANNEL,
    FakeOllama,
    message,
    says,
    wants,
)

pytestmark = pytest.mark.anyio

#: The card's own message in the channel -- what a ❌ actually lands on.
CARD_MESSAGE = 960000000000000123


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


def posts(bot) -> list:
    return [post for post in bot.posts if post.kind == "plain"]


def proposals(bot) -> list[dict]:
    return bot.repo.list_amendments(status="proposed")


async def raise_card(bot, run_id: str, author_id: int = 1002) -> list[dict]:
    """A card the chat pilot really posted, through the real write tool.

    Built by asking the pilot for it rather than by writing rows, because the
    provenance the follow-up runs on *is* the interaction that
    :meth:`bot.chat.agent.ChatPilot.offer` records -- a card assembled by hand
    would prove the gate works against a fixture rather than against the feature.
    """
    asker = pilot(
        bot,
        wants("propose_move", run_query=short_id(run_id), to_when="sunday 22:00"),
        says("Card's up — it needs a ✅."),
    )
    await asker.offer(message(bot, "@bot move hstar to sunday 22:00", author_id=author_id))
    return proposals(bot)


def synthetic_card(
    bot,
    *,
    author_id: str = "1002",
    channel_id: int = CHAT_CHANNEL,
    created_by_chat: bool = True,
    summary: str = "move Hard Will to Sun 07 Sep 22:00",
) -> dict:
    """A proposal row, with or without a chat interaction claiming to have made it.

    The gate tests use this rather than :func:`raise_card` because they are
    about the row's *provenance*, and this is the only way to write the one
    thing the pilot cannot produce for itself: a card in a channel it does not
    answer in, and a card nobody asked it for.
    """
    from .conftest import TZ, kl

    amendment_id = bot.repo.create_amendment(
        kl(2026, 8, 27),
        "move",
        bosses=["HStar"],
        participants=["1002"],
        channel_id=channel_id,
        summary=summary,
        new_datetime=kl(2026, 9, 6, 22, 0).astimezone(TZ),
    )
    bot.repo.set_amendment_proposal_message(amendment_id, CARD_MESSAGE)
    if created_by_chat:
        bot.repo.log_chat_interaction(
            model="test",
            question="move hstar to sunday",
            reply="Card's up.",
            outcome="answered",
            channel_id=channel_id,
            author_id=author_id,
            tool_calls=[{"name": "propose_move", "outcome": "ok", "created": [amendment_id]}],
        )
    return bot.repo.get_amendment(amendment_id)


async def reject(agent, bot, rows: list[dict], reactor_id: int = 1002):
    """A ❌ on the card, as ``_handle_proposal_reaction`` delivers one."""
    return await agent.on_rejection(rows, reactor_id=reactor_id, card_message_id=CARD_MESSAGE)


# ---------------------------------------------------------------------------
# the question itself
# ---------------------------------------------------------------------------


async def test_a_rejected_chat_card_asks_what_it_should_have_been(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("No worries — what should it have been instead?"))
    card = synthetic_card(chat_bot)

    handling = await reject(agent, chat_bot, [card])

    assert handling.handled is True
    (posted,) = posts(chat_bot)
    assert posted.content == "No worries — what should it have been instead?"
    assert posted.channel_id == CHAT_CHANNEL


async def test_the_question_notifies_the_asker_and_nobody_else(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("What should it be?"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    (posted,) = posts(chat_bot)
    assert posted.allowed_mentions == ["1002"]
    assert posted.roles == []


async def test_the_model_is_told_what_the_card_said_and_never_a_members_words(
    chat_bot, chat_seeded
):
    """The prompt is built from the row, so there is nothing in it to steer."""
    agent = pilot(chat_bot, says("What should it be?"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    note = agent._client.conversation()[-1]
    # A `user` turn although nobody said it: gpt-oss's template lifts system
    # messages into the header at the top, and a note about a reaction that just
    # happened has to arrive *after* the conversation it reacted to. The
    # bracketed opener carries the provenance the role no longer can.
    assert note["role"] == "user"
    assert note["content"].startswith("[Note from the scheduler, not from anybody in the channel.]")
    assert "move Hard Will to Sun 07 Sep 22:00" in note["content"]
    assert "kanon" in note["content"]  # the asker, by roster name


async def test_the_follow_up_sends_exactly_one_system_message(chat_bot, chat_seeded):
    """The same guard the answering path has: system at index 0 and nowhere else."""
    agent = pilot(chat_bot, says("What should it be?"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    for sent in agent._client.prompts:
        assert [index for index, m in enumerate(sent) if m["role"] == "system"] == [0]


async def test_the_question_is_generated_in_persona_with_the_voice_reminder(chat_bot, chat_seeded):
    """The same composition path an answer takes -- persona, rules, voice, last."""
    agent = pilot(chat_bot, says("What should it be?"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    from bot.chat import persona

    assert persona.HARD_RULES.strip() in agent._client.system
    assert agent._client.reminder()["content"].startswith(persona.REMINDER_PREFIX)


async def test_a_model_that_fails_says_nothing_at_all(chat_bot, chat_seeded):
    """`FAILURE_REPLY` answers a question. Nobody asked one here."""
    agent = pilot(chat_bot, ConnectionError("ollama is down"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    assert posts(chat_bot) == []


# ---------------------------------------------------------------------------
# the scope gates -- every one of them silent
# ---------------------------------------------------------------------------


async def test_an_extractor_card_is_never_followed_up(chat_bot, chat_seeded):
    """No interaction created it, so nobody asked the bot for it."""
    agent = pilot(chat_bot, says("never said"))
    card = synthetic_card(chat_bot, created_by_chat=False)

    handling = await reject(agent, chat_bot, [card])

    assert handling.handled is False
    assert posts(chat_bot) == []


async def test_only_the_member_who_asked_is_asked_back(chat_bot, chat_seeded):
    """Anyone on the run may ❌ a card; only the asker wanted something."""
    agent = pilot(chat_bot, says("never said"))
    card = synthetic_card(chat_bot, author_id="1002")

    handling = await reject(agent, chat_bot, [card], reactor_id=1001)

    assert handling.handled is False
    assert posts(chat_bot) == []


async def test_a_card_outside_the_pilots_channels_is_left_alone(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("never said"))
    card = synthetic_card(chat_bot, channel_id=OFF_LIMITS_CHANNEL)

    handling = await reject(agent, chat_bot, [card])

    assert handling.handled is False
    assert posts(chat_bot) == []


async def test_quiet_mode_asks_nothing(chat_bot, chat_seeded):
    chat_bot.repo.set_config("quiet_mode", "1")
    agent = pilot(chat_bot, says("never said"))

    handling = await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    assert handling.handled is False
    assert chat_bot.posts == []


async def test_chat_turned_off_asks_nothing(chat_bot, chat_seeded):
    chat_bot.repo.set_config("chat_mode", "0")
    agent = pilot(chat_bot, says("never said"))

    handling = await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    assert handling.handled is False
    assert posts(chat_bot) == []


# ---------------------------------------------------------------------------
# one per card, for ever
# ---------------------------------------------------------------------------


async def test_reacting_unreacting_and_reacting_again_asks_once(chat_bot, chat_seeded):
    """The marker does this, not the cooldown -- so the cooldown is cleared."""
    agent = pilot(chat_bot, says("first"), says("second"), says("third"))
    card = synthetic_card(chat_bot)

    for _ in range(3):
        agent._followed_up_at.clear()
        await reject(agent, chat_bot, [card])

    assert [post.content for post in posts(chat_bot)] == ["first"]


async def test_the_marker_outlives_the_process(chat_bot, chat_seeded):
    """A restart forgets the cooldown; the card must still remember."""
    card = synthetic_card(chat_bot)
    await reject(pilot(chat_bot, says("first")), chat_bot, [card])

    restarted = pilot(chat_bot, says("second"))
    handling = await reject(restarted, chat_bot, [card])

    assert handling.handled is False
    assert [post.content for post in posts(chat_bot)] == ["first"]


async def test_a_second_cross_mid_generation_starts_nothing(chat_bot, chat_seeded):
    released = asyncio.Event()
    agent = pilot(chat_bot)
    card = synthetic_card(chat_bot)

    async def slow(**_kwargs):
        await released.wait()
        return says("first")

    agent._client.chat = slow
    first = asyncio.create_task(reject(agent, chat_bot, [card]))
    await asyncio.sleep(0)
    second = await reject(agent, chat_bot, [card])
    released.set()
    await first

    assert second.handled is False
    assert len(posts(chat_bot)) == 1


async def test_a_card_carrying_two_amendments_is_followed_up_once(chat_bot, chat_seeded):
    """One card, one question -- not one per row on it."""
    agent = pilot(chat_bot, says("first"), says("second"))
    both = [synthetic_card(chat_bot), synthetic_card(chat_bot, summary="cancel Hard Will")]

    await reject(agent, chat_bot, both)
    agent._followed_up_at.clear()
    handling = await reject(agent, chat_bot, both)

    assert handling.handled is False
    assert [post.content for post in posts(chat_bot)] == ["first"]


# ---------------------------------------------------------------------------
# and one per channel, for a while
# ---------------------------------------------------------------------------


async def test_rejecting_several_cards_at_once_asks_once(chat_bot, chat_seeded):
    """Somebody clearing out a channel is one action, not four questions."""
    agent = pilot(chat_bot, *[says(f"question {i}") for i in range(4)])
    cards = [synthetic_card(chat_bot) for _ in range(4)]

    for card in cards:
        await reject(agent, chat_bot, [card])

    assert [post.content for post in posts(chat_bot)] == ["question 0"]


async def test_a_rejection_after_the_cooldown_is_asked_about(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("first"), says("second"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    agent._followed_up_at[str(CHAT_CHANNEL)] = time.monotonic() - followup.COOLDOWN_S - 1
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    assert [post.content for post in posts(chat_bot)] == ["first", "second"]


async def test_a_channel_already_answering_drops_the_question(chat_bot, chat_seeded):
    """A question about a dead card must not queue up behind a live answer."""
    agent = pilot(chat_bot, says("never said"))
    agent._busy.add(str(CHAT_CHANNEL))

    handling = await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    assert handling.handled is False
    assert posts(chat_bot) == []


async def test_a_busy_model_anywhere_on_the_host_drops_the_question(
    chat_bot, chat_seeded, model_lock
):
    """Nobody is waiting for this one, so it is the first thing to give up the model.

    And it gives it up *without* claiming the card: a claim is for ever, so
    burning one on a generation that never runs would silence the card for good.
    """
    agent = pilot(chat_bot, says("What should it be instead?"))
    card = synthetic_card(chat_bot)

    await model_lock.acquire()
    try:
        skipped = await reject(agent, chat_bot, [card])
    finally:
        model_lock.release()

    assert skipped.handled is False
    assert posts(chat_bot) == []
    # The same ❌ once the model is free: still asked about, and asked once.
    assert (await reject(agent, chat_bot, [card])).handled is True
    assert [post.content for post in posts(chat_bot)] == ["What should it be instead?"]


async def test_the_question_holds_the_model_while_it_generates(chat_bot, chat_seeded, model_lock):
    """An extraction starting underneath it would stretch it past its timeout."""
    agent = pilot(chat_bot, says("What should it be instead?"))
    held: list[bool] = []
    real_chat = agent._client.chat

    async def watched(**kwargs):
        held.append(model_lock.locked())
        return await real_chat(**kwargs)

    agent._client.chat = watched
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    assert held == [True]
    assert model_lock.locked() is False


# ---------------------------------------------------------------------------
# the turn that cannot post a card
# ---------------------------------------------------------------------------


async def test_the_write_tools_are_not_offered_to_the_follow_up(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("What should it be?"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    offered = [t["function"]["name"] for t in agent._client.calls[0]["tools"]]
    assert offered == ["get_schedule", "get_run", "list_bosses", "get_pending"]
    assert not any(name.startswith("propose_") for name in offered)


async def test_a_write_tool_asked_for_anyway_is_inert(chat_bot, chat_seeded):
    """Withholding the schema is the polite half; the dispatcher is the real one."""
    before = len(proposals(chat_bot))
    agent = pilot(
        chat_bot,
        wants("propose_cancel", run_query=short_id(chat_seeded["star"])),
        says("What should it be instead?"),
    )
    card = synthetic_card(chat_bot)

    handling = await reject(agent, chat_bot, [card])

    (outcome,) = handling.answered.outcomes
    assert outcome.name == "propose_cancel"
    assert outcome.ok is False
    assert outcome.output == tools.READ_ONLY_TURN
    assert outcome.created == []
    assert len(proposals(chat_bot)) == before + 1  # the rejected card itself, and nothing new


async def test_the_read_tools_still_work_in_the_follow_up(chat_bot, chat_seeded):
    """It may look the run up before asking -- it just cannot change one."""
    agent = pilot(
        chat_bot,
        wants("get_run", query=short_id(chat_seeded["star"])),
        says("What should it be instead?"),
    )

    handling = await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    (outcome,) = handling.answered.outcomes
    assert outcome.name == "get_run"
    assert outcome.ok is True


# ---------------------------------------------------------------------------
# what the answer arrives into
# ---------------------------------------------------------------------------


async def test_the_channel_remembers_the_card_and_the_question(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("What should it be instead?"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    remembered = list(agent.history(str(CHAT_CHANNEL)))
    # The note is `user` for the same reason the question was: a system turn
    # would be hoisted out of the history and stop sitting where it happened.
    assert [turn.role for turn in remembered] == ["user", "assistant"]
    assert remembered[0].content.startswith("[Note]")
    assert "move Hard Will to Sun 07 Sep 22:00" in remembered[0].content
    assert remembered[1].content == "What should it be instead?"


async def test_a_failed_question_is_not_remembered_as_having_been_asked(chat_bot, chat_seeded):
    agent = pilot(chat_bot, ConnectionError("ollama is down"))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    assert list(agent.history(str(CHAT_CHANNEL))) == []


async def test_the_answer_corrects_the_card_through_the_ordinary_loop(chat_bot, chat_seeded):
    """End to end: ❌, the question, their reply, a new card -- and no new path.

    The reply is an ordinary mention answered by :meth:`ChatPilot.offer`, so the
    only thing this feature contributes to it is the *context*: the model is
    shown the rejected card's facts and its own question before it reads
    "friday 21:00 instead".
    """
    agent = pilot(
        chat_bot,
        says("What should it be instead?"),
        wants("propose_move", run_query=short_id(chat_seeded["star"]), to_when="friday 21:00"),
        says("Card's up for Friday 21:00 — it needs a ✅."),
    )
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])
    before = len(proposals(chat_bot))

    await agent.offer(message(chat_bot, "@bot friday 21:00 instead", author_id=1002))

    seen = agent._client.conversation(1)
    assert any("move Hard Will to Sun 07 Sep 22:00" in turn["content"] for turn in seen)
    assert any(turn["content"] == "What should it be instead?" for turn in seen)
    assert len(proposals(chat_bot)) == before + 1


# ---------------------------------------------------------------------------
# the books
# ---------------------------------------------------------------------------


async def test_the_question_is_recorded_as_an_interaction(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("What should it be instead?"))
    card = synthetic_card(chat_bot)
    before = chat_bot.repo.count_chat_interactions()

    await reject(agent, chat_bot, [card])

    row = chat_bot.repo.recent_chat_interactions()[0]
    assert chat_bot.repo.count_chat_interactions() == before + 1
    assert row["reply"] == "What should it be instead?"
    assert row["outcome"] == "answered"
    assert row["author_id"] == "1002"
    assert row["channel_id"] == str(CHAT_CHANNEL)
    assert row["message_id"] == str(CARD_MESSAGE)
    assert "reacted ❌" in row["question"]
    assert isinstance(row["latency_ms"], int)


async def test_the_question_costs_the_member_none_of_their_allowance(chat_bot, chat_seeded):
    """The bot started this conversation, so the member does not pay for it."""
    agent = pilot(chat_bot, says("What should it be instead?"))
    before = agent.limiter.remaining(1002)

    await reject(agent, chat_bot, [synthetic_card(chat_bot)])

    assert agent.limiter.remaining(1002) == before


async def test_their_answer_does_cost_them_normally(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("What should it be instead?"), says("Right you are."))
    await reject(agent, chat_bot, [synthetic_card(chat_bot)])
    before = agent.limiter.remaining(1002)

    await agent.offer(message(chat_bot, "@bot friday instead", author_id=1002))

    assert agent.limiter.remaining(1002) == before - 1


# ---------------------------------------------------------------------------
# provenance, over a card the pilot really posted
# ---------------------------------------------------------------------------


async def test_a_card_the_pilot_really_posted_is_traced_back_to_its_asker(chat_bot, chat_seeded):
    """The provenance query, over the rows `offer` actually writes."""
    (card,) = await raise_card(chat_bot, chat_seeded["star"])

    interaction = chat_bot.repo.chat_interaction_for_amendment(card["id"])
    assert interaction is not None
    assert interaction["author_id"] == "1002"


async def test_a_real_chat_card_rejected_by_its_asker_is_followed_up(chat_bot, chat_seeded):
    """The whole path over a card nothing in the test wrote by hand."""
    (card,) = await raise_card(chat_bot, chat_seeded["star"])
    agent = pilot(chat_bot, says("Not Sunday then — when suits?"))

    handling = await agent.on_rejection(
        [card], reactor_id=1002, card_message_id=card["proposal_message_id"]
    )

    assert handling.handled is True
    assert posts(chat_bot)[-1].content == "Not Sunday then — when suits?"
    # The facts in the prompt are the write tool's own summary of the card.
    assert card["summary"] in agent._client.conversation()[-1]["content"]


async def test_provenance_ignores_an_id_that_merely_looks_similar(chat_bot, chat_seeded):
    card = synthetic_card(chat_bot)
    assert chat_bot.repo.chat_interaction_for_amendment(card["id"][:8]) is None


# ---------------------------------------------------------------------------
# the seam: which reactions reach the pilot at all
# ---------------------------------------------------------------------------


class Recorder:
    """A pilot that records the rejection it was handed and generates nothing."""

    def __init__(self) -> None:
        self.calls: list[tuple[list[str], int, int]] = []

    async def on_rejection(self, amendments, *, reactor_id, card_message_id):
        self.calls.append(([a["id"] for a in amendments], reactor_id, card_message_id))


def reaction_bot(chat_bot, chat_pilot) -> BossBot:
    """A client wired to the real reaction path and nothing else.

    ``Client.user`` is a read-only property fed by the gateway, so the bot is
    built the way `tests/test_at_risk.py` builds one: no ``__init__``, and only
    the attributes the path under test reads.
    """

    class Wired(BossBot):
        user = SimpleNamespace(id=BOT_USER_ID)

    client = Wired.__new__(Wired)
    client.repo = chat_bot.repo
    client.settings = chat_bot.settings
    client.chat = chat_pilot
    client.get_guild = lambda _guild_id: chat_bot.guild
    client.annotated: list[str] = []

    async def annotate(_payload, notice):
        client.annotated.append(notice)

    client._annotate_card = annotate
    return client


def card_reaction(emoji: str, user_id: int = 1002) -> SimpleNamespace:
    return SimpleNamespace(
        user_id=user_id,
        emoji=emoji,
        message_id=CARD_MESSAGE,
        channel_id=CHAT_CHANNEL,
        member=None,
    )


async def test_a_cross_on_a_card_reaches_the_pilot(chat_bot, chat_seeded):
    recorder = Recorder()
    client = reaction_bot(chat_bot, recorder)
    card = synthetic_card(chat_bot)

    await client._handle_proposal_reaction(card_reaction(EMOJI_NO), [card], EMOJI_NO)

    assert recorder.calls == [([card["id"]], 1002, CARD_MESSAGE)]
    assert chat_bot.repo.get_amendment(card["id"])["status"] == "rejected"


async def test_a_tick_on_a_card_reaches_nobody(chat_bot, chat_seeded):
    """A follow-up is a question about a card that is *off*."""
    recorder = Recorder()
    client = reaction_bot(chat_bot, recorder)
    card = synthetic_card(chat_bot)

    async def committed(amendment, _actor, _payload):
        return CommitResult(amendment_id=amendment["id"], kind=amendment["kind"])

    client._commit_one = committed
    await client._handle_proposal_reaction(card_reaction(EMOJI_YES), [card], EMOJI_YES)

    assert recorder.calls == []


async def test_a_pilot_that_throws_does_not_cost_the_card_its_rejection(chat_bot, chat_seeded):
    class Broken:
        async def on_rejection(self, *_args, **_kwargs):
            raise RuntimeError("the model is on fire")

    client = reaction_bot(chat_bot, Broken())
    card = synthetic_card(chat_bot)

    await client._handle_proposal_reaction(card_reaction(EMOJI_NO), [card], EMOJI_NO)

    assert chat_bot.repo.get_amendment(card["id"])["status"] == "rejected"
    assert client.annotated  # the card still says who turned it down
