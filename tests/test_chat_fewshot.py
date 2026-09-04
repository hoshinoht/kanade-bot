"""Making the voice survive as far as the sentence the model actually writes.

The persona sat at the top of the prompt and the voice reminder at the end of
the *system* prompt -- but by composition time the model has read a conversation
and a stack of tool results since, and the recency the reminder was written for
had been spent on run ids and card confirmations. Those turns (confirmations,
error relays) were exactly the flattest ones live.

Two fixes here: the reminder becomes the final message of the call, after
everything; and the persona's own "Good" lines are shown as few-shot examples,
which steer harder per token than any list of adjectives.

The reminder is sent as a ``user`` message, which is what actually makes "final"
mean anything: Ollama's gpt-oss template skips system messages in the message
loop and concatenates them into the instructions header at the top, so a
trailing *system* message is rendered back where the buried copy already was.
"""

from __future__ import annotations

import pytest

from bot.chat import persona
from bot.chat.agent import ChatPilot
from bot.domain.ids import short_id

from .chat_support import FakeOllama, build_bot, message, says, wants

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


VOICED = """\
# Persona: Placeholder

**Voice:** Dry, fond of the party, allergic to exclamation marks.

lots of character notes

**Good**

> `Wed 21:30, same as always.`

> `Three confirmed. One more and we go.`

**Bad**

> `WED!!! 9:30!!! LETS GOOOO`
"""


def voiced_bot(repo, bosses, tmp_path, monkeypatch, text: str = VOICED):
    path = tmp_path / "persona.md"
    path.write_text("# Persona: Placeholder\n", encoding="utf-8")
    behaviour = tmp_path / "default.md"
    behaviour.write_text(text, encoding="utf-8")
    monkeypatch.setattr(persona, "DEFAULT_BEHAVIOUR", behaviour)
    return build_bot(repo, bosses, persona_path=str(path))


# ---------------------------------------------------------------------------
# the reminder is the last thing the model reads
# ---------------------------------------------------------------------------


async def test_the_final_message_is_the_reminder_on_a_direct_answer(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("Wed 21:30."))
    await agent.offer(message(chat_bot))

    last = agent._client.calls[0]["messages"][-1]
    # `user`, not `system`: gpt-oss's template hoists system messages into the
    # instructions header, so a trailing system message is not trailing at all.
    assert last["role"] == "user"
    assert last["content"].startswith(persona.REMINDER_PREFIX)


async def test_the_reminder_says_it_is_not_from_anybody_in_the_channel(chat_bot, chat_seeded):
    """It arrives as a `user` turn, so it has to say it is not a member talking."""
    agent = pilot(chat_bot, says("Wed 21:30."))
    await agent.offer(message(chat_bot))

    assert "not from anybody in the channel" in agent._client.reminder()["content"]


async def test_only_the_first_message_is_a_system_one(chat_bot, chat_seeded):
    """The whole point of the roles above: exactly one system message, at index 0.

    Ollama's gpt-oss template drops every system message from the rendered
    conversation and concatenates them into one header at the top, so any system
    message after the first silently loses its position.
    """
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Two runs."))
    await agent.offer(message(chat_bot))

    for sent in agent._client.prompts:
        assert [index for index, m in enumerate(sent) if m["role"] == "system"] == [0]


async def test_the_final_message_is_the_reminder_after_tool_results(chat_bot, chat_seeded):
    """The turn that was flattest live: a card confirmation, after tool output."""
    agent = pilot(
        chat_bot,
        wants("propose_cancel", run_query=short_id(chat_seeded["star"])),
        says("Card's up."),
    )
    await agent.offer(message(chat_bot, "@bot cancel hstar"))

    composition = agent._client.calls[-1]["messages"]
    assert composition[-1]["content"].startswith(persona.REMINDER_PREFIX)
    # ...and it really is after the tool result, not merely present.
    assert composition[-2]["role"] == "tool"


async def test_every_round_ends_with_it(chat_bot, chat_seeded):
    agent = pilot(chat_bot, wants("get_schedule", week="this"), says("Two runs."))
    await agent.offer(message(chat_bot))
    for call in agent._client.calls:
        assert call["messages"][-1]["content"].startswith(persona.REMINDER_PREFIX)


async def test_the_reminder_carries_the_active_voice(repo, bosses, tmp_path, monkeypatch):
    bot = voiced_bot(repo, bosses, tmp_path, monkeypatch)
    agent = pilot(bot, says("ok"))
    await agent.offer(message(bot))
    assert (
        "Dry, fond of the party, allergic to exclamation marks."
        in agent._client.reminder()["content"]
    )


async def test_the_reminder_is_not_remembered_as_conversation(chat_bot, chat_seeded):
    """It is appended per call, so it must not pile up in the history."""
    agent = pilot(chat_bot, says("a"), says("b"), says("c"))
    for _ in range(3):
        await agent.offer(message(chat_bot))

    third = agent._client.conversation(2)
    # Only the system prompt's own footer, which is a different string from the
    # appended message -- so a leaked copy of the message would be visible here.
    assert sum(1 for m in third if persona.VOICE_PREFIX in m["content"]) == 1
    assert not any(persona.REMINDER_PREFIX in m["content"] for m in third)
    remembered = [t.content for t in agent.history(str(chat_bot.channels and 777777777777777777))]
    assert not any(persona.REMINDER_PREFIX in line for line in remembered)


async def test_nothing_else_is_reordered(chat_bot, chat_seeded):
    """Only an append: system, then the conversation, exactly as before."""
    agent = pilot(chat_bot, says("Monday 21:30."), says("Tuesday 23:00."))
    await agent.offer(message(chat_bot, "@bot when is hstar?"))
    await agent.offer(message(chat_bot, "@bot and kalos?"))

    assert [m["role"] for m in agent._client.conversation(1)] == [
        "system",
        "user",
        "assistant",
        "user",
    ]


# ---------------------------------------------------------------------------
# few-shot extraction
# ---------------------------------------------------------------------------


def test_the_good_lines_are_read_and_the_bad_ones_are_not():
    assert persona.good_examples(VOICED) == [
        "Wed 21:30, same as always.",
        "Three confirmed. One more and we go.",
    ]


def test_a_persona_with_no_good_section_contributes_nothing():
    assert persona.good_examples("# Persona\n\n**Voice:** Dry.\n\nnotes") == []
    assert persona.examples_block("# Persona\n\nnotes") == ""


def test_the_tracked_template_contributes_nothing():
    template = persona.read_default_behaviour(
        persona.EXAMPLE_DEFAULT_BEHAVIOUR
    ).text
    assert "**Good**" in template
    assert persona.good_examples(template) == []


def test_code_fence_lines_are_not_scavenged():
    text = "**Good**\n\n```\n> `fenced and should not count`\n```\n\n> `real one`\n"
    assert persona.good_examples(text) == ["real one"]


def test_a_quote_outside_the_good_section_is_ignored():
    text = "> `an epigraph`\n\n**Good**\n\n> `the real one`\n\n**Bad**\n\n> `nope`\n"
    assert persona.good_examples(text) == ["the real one"]


def test_a_heading_ends_the_section():
    text = "**Good**\n\n> `kept`\n\n## 8. Something else\n\n> `dropped`\n"
    assert persona.good_examples(text) == ["kept"]


def test_a_qualified_good_heading_is_read_too():
    """The live file's second section: `**Good — chat-pilot replies (...)**`."""
    text = (
        "**Good**\n\n> `general one`\n\n"
        "**Good — chat-pilot replies (answering questions and relaying tool results)**\n\n"
        "> `the answering one`\n\n**Bad**\n\n> `nope`\n"
    )
    assert persona.good_sections(text) == [["general one"], ["the answering one"]]
    assert persona.good_examples(text) == ["general one", "the answering one"]


def test_goodbye_is_not_a_good_heading():
    assert persona.good_sections("**Goodbye everyone**\n\n> `nope`\n") == []


def test_the_budget_is_shared_round_robin_across_sections():
    """A long first section must not spend the whole budget.

    That is exactly what happened live: the section written for chat-pilot
    replies sat second and contributed nothing.
    """
    text = (
        "**Good**\n\n"
        + "\n\n".join(f"> `first {i}`" for i in range(8))
        + "\n\n**Good — replies**\n\n"
        + "\n\n".join(f"> `second {i}`" for i in range(8))
    )
    kept = persona.good_examples(text)
    assert len(kept) == persona.MAX_EXAMPLES
    assert sum(1 for line in kept if line.startswith("first")) == 4
    assert sum(1 for line in kept if line.startswith("second")) == 4
    # ...and it interleaves, strongest-of-each-section first.
    assert kept[:2] == ["first 0", "second 0"]


def test_one_section_still_fills_the_budget_alone():
    text = "**Good**\n\n" + "\n\n".join(f"> `line {i}`" for i in range(12))
    assert len(persona.good_examples(text)) == persona.MAX_EXAMPLES


def test_at_most_eight_examples_are_kept():
    text = "**Good**\n\n" + "\n\n".join(f"> `line {i}`" for i in range(20))
    assert len(persona.good_examples(text)) == persona.MAX_EXAMPLES


def test_the_character_budget_is_respected():
    text = "**Good**\n\n" + "\n\n".join(f"> `{'x' * 200} {i}`" for i in range(8))
    kept = persona.good_examples(text)
    assert sum(len(line) for line in kept) <= persona.MAX_EXAMPLE_CHARS
    assert len(kept) < 8  # the character cap bit before the count cap


# ---------------------------------------------------------------------------
# the block in the prompt
# ---------------------------------------------------------------------------


def test_the_block_is_labelled_and_listed():
    block = persona.examples_block(VOICED)
    assert block.startswith(persona.EXAMPLES_HEADING)
    assert "- Wed 21:30, same as always." in block


def test_the_block_sits_near_the_end_before_the_voice_footer():
    built = persona.system_prompt(VOICED, "CLOCK HERE")
    assert built.index("CLOCK HERE") < built.index(persona.EXAMPLES_HEADING)
    assert built.index(persona.EXAMPLES_HEADING) < built.index(persona.VOICE_PREFIX)


def test_a_persona_without_examples_leaves_no_empty_heading():
    built = persona.system_prompt("**Voice:** Dry.\n\nnotes", "CLOCK")
    assert persona.EXAMPLES_HEADING not in built
    assert built.rstrip().endswith("Dry.")


async def test_the_examples_reach_the_model(repo, bosses, tmp_path, monkeypatch):
    bot = voiced_bot(repo, bosses, tmp_path, monkeypatch)
    agent = pilot(bot, says("ok"))
    await agent.offer(message(bot))

    system = agent._client.system
    assert persona.EXAMPLES_HEADING in system
    promoted = system[system.index(persona.EXAMPLES_HEADING) :]
    assert "Wed 21:30, same as always." in promoted
    assert "WED!!!" not in promoted
