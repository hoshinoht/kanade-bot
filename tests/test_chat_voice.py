"""Why replies sound like the persona rather than like anyone.

Three code-side levers, none of which is the persona document itself:

* sampling -- the chatbot sets its own temperature instead of inheriting the
  extractor's greedy decode or the model's Modelfile default;
* recency -- one line of voice is repeated as the LAST thing in the system
  prompt, because the persona document itself is thousands of tokens away from
  where the model actually composes;
* the persona is never trimmed, however long the conversation gets.
"""

from __future__ import annotations

import pytest

from bot.chat import persona
from bot.chat.agent import ChatPilot, ChatTurn

from .chat_support import CHAT_CHANNEL, FakeOllama, build_bot, chat_settings, message, says

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def pilot(bot, *responses) -> ChatPilot:
    return ChatPilot(bot, client=FakeOllama(*responses))


# ---------------------------------------------------------------------------
# (a) sampling
# ---------------------------------------------------------------------------


async def test_the_chatbot_sets_its_own_temperature(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("ok"))
    await agent.offer(message(chat_bot))
    options = agent._client.calls[0]["options"]
    assert options["temperature"] == chat_bot.settings.chat_pilot_temperature
    assert options["num_ctx"] == chat_bot.settings.ollama_num_ctx


async def test_it_does_not_inherit_the_extractors_greedy_decode(chat_bot, chat_seeded):
    """The extractor pins 0 next door; a conversation at 0 reads like a form letter."""
    import inspect

    from bot.extract import llm

    assert '"temperature": 0' in inspect.getsource(llm.Extractor._chat)
    agent = pilot(chat_bot, says("ok"))
    await agent.offer(message(chat_bot))
    assert agent._client.calls[0]["options"]["temperature"] > 0


async def test_the_temperature_is_configurable(repo, bosses):
    bot = build_bot(repo, bosses, chat_pilot_temperature=1.2)
    agent = pilot(bot, says("ok"))
    await agent.offer(message(bot))
    assert agent._client.calls[0]["options"]["temperature"] == 1.2


def test_the_temperature_default_and_bounds():
    assert chat_settings().chat_pilot_temperature == 0.7
    for bad in (-0.1, 2.1):
        with pytest.raises(ValueError):
            chat_settings(chat_pilot_temperature=bad)


def test_the_env_example_documents_the_temperature():
    from .conftest import REPO_ROOT

    text = (REPO_ROOT / ".env.example").read_text(encoding="utf-8")
    assert "\nCHAT_PILOT_TEMPERATURE=0.7" in text


# ---------------------------------------------------------------------------
# (b) the voice reminder
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("line", "expected"),
    [
        (
            "**Voice:** Deadpan idol who takes the schedule seriously.",
            "Deadpan idol who takes the schedule seriously.",
        ),
        ("**Voice**: colon outside the emphasis", "colon outside the emphasis"),
        ("Voice: plain form", "plain form"),
        ("  voice:   extra space   ", "extra space"),
        ("<!-- voice: written as a comment -->", "written as a comment"),
        ("_Voice:_ underscores", "underscores"),
    ],
)
def test_the_loader_reads_every_reasonable_spelling(line, expected):
    assert persona.voice_line(line) == expected


@pytest.mark.parametrize(
    "text",
    [
        "",
        "nothing about voice here",
        "**Voice:** <one sentence: archetype, register>",  # unfilled slot
        "Voice:   ",
    ],
)
def test_an_absent_or_unfilled_voice_falls_back(text):
    assert persona.voice_line(text) == persona.DEFAULT_VOICE


def test_the_first_voice_line_wins():
    """A persona also contains prose *about* voice; the slot must not lose to it."""
    text = "**Voice:** The real one.\n\n## Later\n\nVoice: an example inside a code fence"
    assert persona.voice_line(text) == "The real one."


def test_the_tracked_template_has_a_slot_that_reads_as_unfilled():
    text = persona.EXAMPLE_PERSONA.read_text(encoding="utf-8")
    assert "**Voice:**" in text
    assert persona.voice_line(text) == persona.DEFAULT_VOICE


def test_the_voice_footer_is_the_last_thing_in_the_prompt():
    text = "**Voice:** Deadpan.\n\nlots of persona"
    built = persona.system_prompt(text, "CLOCK")
    assert built.rstrip().endswith(persona.voice_footer(text))
    # ...and it really is after the rules and the clock, not merely present.
    assert built.index("CLOCK") < built.index(persona.VOICE_PREFIX)
    assert built.index("Operating rules") < built.index(persona.VOICE_PREFIX)


def test_the_order_is_persona_rules_clock_voice():
    built = persona.system_prompt("**Voice:** Deadpan.\n\nPERSONA BODY", "CLOCK HERE")
    positions = [
        built.index("PERSONA BODY"),
        built.index("Operating rules"),
        built.index("CLOCK HERE"),
        built.index(persona.VOICE_PREFIX),
    ]
    assert positions == sorted(positions)


async def test_the_footer_reaches_the_model_as_the_last_system_line(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("ok"))
    await agent.offer(message(chat_bot))
    system = agent._client.system
    assert system.rstrip().endswith(persona.DEFAULT_VOICE)
    assert [m["role"] for m in agent._client.prompts[0]][0] == "system"


async def test_a_real_voice_line_reaches_the_model(tmp_path, repo, bosses):
    path = tmp_path / "persona.md"
    path.write_text(
        "# Persona: Placeholder\n\n**Voice:** Dry, fond of the party, allergic to exclamation "
        "marks.\n\nlots more text\n",
        encoding="utf-8",
    )
    bot = build_bot(repo, bosses, persona_path=str(path))
    agent = pilot(bot, says("ok"))
    await agent.offer(message(bot))
    assert agent._client.system.rstrip().endswith(
        "Dry, fond of the party, allergic to exclamation marks."
    )


def test_neither_form_carries_ids_or_secrets(repo, bosses):
    bot = build_bot(repo, bosses)
    text = persona.load_persona(None)
    for form in (persona.voice_footer(text), persona.voice_reminder(text)):
        for secret in (str(bot.settings.guild_id), bot.settings.admin_token, "CHAT_PILOT"):
            assert secret not in form


def test_the_two_forms_differ_only_where_position_demands_it():
    """The footer is instructions; the reminder is a message and must say so.

    "answer the conversation above it" is true of the trailing message and false
    of the system prompt, which has no conversation above it -- so the bracketed
    opener must never leak into the footer.
    """
    text = "**Voice:** Deadpan.\n\nlots of persona"
    footer, reminder = persona.voice_footer(text), persona.voice_reminder(text)

    assert "not from anybody in the channel" in reminder
    assert "not from anybody in the channel" not in footer
    assert "answer the conversation above it" not in footer
    # ...and both still carry the persona's own sentence, which is the point.
    assert "Deadpan." in footer
    assert "Deadpan." in reminder


def test_the_bracketed_opener_never_reaches_the_system_prompt():
    built = persona.system_prompt("**Voice:** Deadpan.\n\nlots of persona", "CLOCK")
    assert persona.REMINDER_PREFIX not in built
    assert persona.REMINDER_SUFFIX not in built


# ---------------------------------------------------------------------------
# (c) the persona is never trimmed
# ---------------------------------------------------------------------------


async def test_the_persona_survives_a_conversation_that_blows_the_budget(chat_bot, chat_seeded):
    """The budget trims remembered turns; the system prompt is not a turn."""
    agent = pilot(chat_bot, says("ok"))
    for index in range(60):
        agent.remember(str(CHAT_CHANNEL), ChatTurn("user", f"kanon: {'chatter ' * 300}{index}"))
        agent.remember(str(CHAT_CHANNEL), ChatTurn("assistant", "sure " * 300))

    built = agent.build_conversation(message(chat_bot, "@bot what's on?"), str(CHAT_CHANNEL))
    system = built[0]["content"]
    whole = persona.load_persona(chat_bot.settings.persona_path)

    assert built[0]["role"] == "system"
    assert whole in system, "the persona document was truncated"
    assert system.rstrip().endswith(persona.voice_footer(whole))
    assert len(built) < 122  # turns really were dropped


async def test_the_persona_survives_a_single_enormous_question(chat_bot, chat_seeded):
    agent = pilot(chat_bot, says("ok"))
    built = agent.build_conversation(
        message(chat_bot, "@bot " + "please " * 5000), str(CHAT_CHANNEL)
    )
    assert persona.load_persona(chat_bot.settings.persona_path) in built[0]["content"]
    assert [m["role"] for m in built] == ["system", "user"]
