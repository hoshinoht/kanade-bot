"""Asking one question instead of guessing.

A write that is missing a piece -- no time, a boss with no difficulty, a name
that could be two people -- must produce a question, not a guess and not a dead
end. The mechanism is three things that have to agree: the refusal text tells
the model what to ask, a hard rule tells it to ask rather than retry, and the
gate already accepts the reply so the answer can arrive in the next turn.
"""

from __future__ import annotations

import pytest

from bot.chat import persona, tools
from bot.chat.agent import ChatPilot

from .chat_support import (
    BOT_USER_ID,
    CHAT_CHANNEL,
    FakeOllama,
    FakeReference,
    message,
    says,
    wants,
)

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


def context(bot, author_id: int | str = 1002):
    return tools.ToolContext(
        bot=bot,
        author_id=str(author_id),
        channel_id=str(CHAT_CHANNEL),
        message_id="950000000000000888",
    )


def tomorrow_at(hhmm: str = "21:30") -> str:
    from datetime import timedelta

    from bot.timeutil import utcnow

    from .conftest import TZ

    return (utcnow().astimezone(TZ) + timedelta(days=1)).strftime(f"%Y-%m-%d {hhmm}")


# ---------------------------------------------------------------------------
# the rule
# ---------------------------------------------------------------------------


def test_the_prompt_tells_the_model_to_ask_rather_than_guess():
    rules = persona.HARD_RULES
    assert "ask" in rules.lower()
    assert "ONE short, specific question" in rules
    assert "Do not call the tool again with a guess" in rules
    for invented in ("difficulty", "time", "boss", "member"):
        assert invented in rules.lower()


def test_the_rules_are_numbered_once_and_in_order():
    """A renumbering slip would leave two rules sharing a number."""
    import re

    numbers = [int(n) for n in re.findall(r"^(\d+)\. ", persona.HARD_RULES, re.MULTILINE)]
    assert numbers == list(range(1, len(numbers) + 1))


async def test_the_rule_reaches_the_model_in_the_system_prompt(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("hi"))
    await agent.offer(message(chat_bot))
    assert "ONE short, specific question" in agent._client.system


# ---------------------------------------------------------------------------
# every write refusal has to say what to ask
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("name", "args"),
    [
        ("propose_add", {"boss": "bellona", "when": tomorrow_at()}),
        ("propose_add", {"boss": "HBellona"}),
        ("propose_add", {"boss": "HBellona", "when": "whenever lah"}),
        ("propose_add", {"boss": "", "when": tomorrow_at()}),
        ("propose_move", {"run_query": "nonsense", "to_when": tomorrow_at()}),
        ("propose_move", {"run_query": "hstar"}),
        ("propose_cancel", {"run_query": "the thing"}),
        ("propose_rsvp", {"run_query": "hstar", "answer": "dunno"}),
    ],
)
async def test_a_write_refusal_names_the_question_to_ask(chat_bot, chat_seeded, name, args):
    answer = await tools.dispatch(context(chat_bot), name, args)
    assert "sk them" in answer or "sk which" in answer, answer
    assert chat_bot.repo.list_amendments(status="proposed") == []


async def test_an_ambiguous_run_is_refused_with_the_candidates(chat_bot, chat_seeded):
    from datetime import timedelta

    kalos = chat_bot.repo.get_run(chat_seeded["kalos"])
    chat_bot.repo.create_run(
        week_start=kalos["week_start"],
        bosses=["XKalos"],
        run_at=kalos["datetime"] + timedelta(days=3),
        participants=["1002"],
        status="planned",
        source="amend",
        channel_id=kalos["channel_id"],
    )
    answer = await tools.dispatch(context(chat_bot), "propose_cancel", {"run_query": "kalos"})
    assert "Ask which one" in answer
    assert answer.count("XKalos") == 2  # both nights are named so it can ask


async def test_an_ambiguous_participant_is_refused_with_the_candidates(chat_bot, chat_seeded):
    # "kano" is nobody's exact name, and prefixes both kanon and kanonn.
    chat_bot.repo.upsert_member(1004, "kanonn", "kanonn", True)
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_add",
        {"boss": "HBellona", "when": tomorrow_at(), "participants": "kano"},
    )
    assert "Ask them which they mean" in answer
    assert chat_bot.repo.list_amendments(status="proposed") == []


# ---------------------------------------------------------------------------
# the whole two-turn conversation
# ---------------------------------------------------------------------------


async def test_a_missing_difficulty_becomes_a_question_then_a_card(chat_bot, chat_seeded):
    """The live flow, end to end, with the model scripted to behave.

    Turn one: the user names a boss with no difficulty, the tool refuses with
    the valid forms, and the model asks. Turn two: they answer as a reply, which
    the gate accepts as a mention, and the card goes up.
    """
    agent = pilot(
        chat_bot,
        # turn one
        wants("propose_add", boss="bellona", when="tomorrow 21:30"),
        says("Which Bellona — Easy, Normal or Hard?"),
        # turn two
        wants("propose_add", boss="HBellona", when="tomorrow 21:30"),
        says("Card's up for HBellona — someone ✅ it."),
    )

    asked = message(chat_bot, "@bot schedule a new run for bellona tomorrow at 2130")
    first = (await agent.offer(asked)).answered

    assert first.tool_calls == ["propose_add"]
    assert first.outcomes[0].outcome == tools.REFUSED
    assert "missing a difficulty prefix" in first.outcomes[0].output
    assert first.reply == "Which Bellona — Easy, Normal or Hard?"
    assert chat_bot.repo.list_amendments(status="proposed") == []

    # They reply to the bot's question; the gate takes a reply to the bot as a
    # mention, so a clarification does not need to be re-@-ed.
    question = message(chat_bot, first.reply, author_id=BOT_USER_ID, mentions=())
    replied = message(chat_bot, "hard one", mentions=(), reference=FakeReference(question))
    second = (await agent.offer(replied)).answered

    assert second.error is None
    row = chat_bot.repo.list_amendments(status="proposed")[0]
    assert (row["kind"], row["bosses"]) == ("add", ["HBellona"])
    assert row["proposal_message_id"]
    assert len(second.created) == 1


async def test_the_question_and_its_answer_are_both_in_the_second_prompt(chat_bot, chat_seeded):
    """The clarification only works because the channel remembers the exchange."""
    agent = pilot(
        chat_bot,
        wants("propose_add", boss="bellona", when="tomorrow 21:30"),
        says("Which Bellona — Easy, Normal or Hard?"),
        says("Right, Hard."),
    )
    await agent.offer(message(chat_bot, "@bot new run for bellona tomorrow 2130"))
    await agent.offer(message(chat_bot, "@bot hard"))

    final = agent._client.prompts[-1]
    contents = [m["content"] for m in final]
    assert any("bellona tomorrow 2130" in c for c in contents)
    assert "Which Bellona — Easy, Normal or Hard?" in contents
    assert any(c.endswith("hard") for c in contents)


async def test_a_model_that_guesses_anyway_still_only_makes_a_card(chat_bot, chat_seeded):
    """The rule is guidance; the card requirement is the actual guarantee."""
    agent = pilot(
        chat_bot,
        wants("propose_add", boss="bellona", when="tomorrow 21:30"),
        wants("propose_add", boss="HBellona", when="tomorrow 21:30"),
        says("Went with Hard."),
    )
    before = len(chat_bot.repo.list_runs())
    await agent.offer(message(chat_bot, "@bot new run for bellona tomorrow 2130"))

    assert len(chat_bot.repo.list_runs()) == before
    assert chat_bot.repo.list_amendments(status="proposed")[0]["status"] == "proposed"
