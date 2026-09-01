"""What the pilot remembers between messages, and for how long.

Three mechanisms with one clock behind them, and none of them tries to guess
where one conversation ends and the next begins: turns age out
(``CHAT_PILOT_HISTORY_TTL_S``), the last card a channel saw is carried as one
line of the system prompt on the same clock, and an answer somebody *replies to*
comes back whatever its age.

Driven by a fake clock rather than by sleeping, exactly as
`tests/test_chat_ratelimit.py` drives the sliding window: the TTL is 45 minutes
in production, and a test that waits it out is a test nobody runs.
"""

from __future__ import annotations

from datetime import timedelta

import pytest

from bot.chat import persona
from bot.chat.agent import ANCHOR_CACHE, ChatPilot, ChatTurn
from bot.timeutil import utcnow

from .chat_support import (
    BOT_USER_ID,
    CHAT_CHANNEL,
    FakeOllama,
    FakeReference,
    chat_settings,
    message,
    says,
    wants,
)
from .conftest import TZ
from .fake_bot import FakeMessage

pytestmark = pytest.mark.anyio

#: Long enough to be obviously deliberate in an assertion, short enough that
#: winding a fake clock past it reads as a jump rather than an era.
TTL = 600.0


@pytest.fixture
def anyio_backend():
    return "asyncio"


class Clock:
    """A monotonic clock a test can wind forward, as the rate limiter's tests use."""

    def __init__(self, now: float = 1000.0):
        self.now = now

    def __call__(self) -> float:
        return self.now

    def advance(self, seconds: float) -> None:
        self.now += seconds


def pilot(bot, *responses, clock: Clock | None = None) -> tuple[ChatPilot, Clock]:
    """A pilot on a fake clock, with the TTL wound down to something testable."""
    clock = clock or Clock()
    bot.settings.chat_pilot_history_ttl_s = TTL
    return ChatPilot(bot, client=FakeOllama(*responses), clock=clock), clock


def channel() -> str:
    return str(CHAT_CHANNEL)


def system_of(agent: ChatPilot, index: int = -1) -> str:
    return agent._client.calls[index]["messages"][0]["content"]


def tomorrow_at(hhmm: str = "23:30") -> str:
    return (utcnow().astimezone(TZ) + timedelta(days=1)).strftime(f"%Y-%m-%d {hhmm}")


def weekly_card(**overrides):
    """The model asking for a recurring run, which is the richest card there is."""
    args = {
        "boss": "HBellona",
        "when": tomorrow_at(),
        "weekly": True,
        "participants": "kanon, Priya",
    }
    args.update(overrides)
    return wants("propose_add", **args)


# ---------------------------------------------------------------------------
# the history TTL
# ---------------------------------------------------------------------------


async def test_a_stale_conversation_is_not_in_the_prompt(chat_bot, chat_seeded):
    """The live hazard: "move it to 22:00" an hour later binding to a dead topic."""
    agent, clock = pilot(chat_bot, says("ok"))
    agent.remember(channel(), ChatTurn("user", "kanon: when is hstar?"))
    agent.remember(channel(), ChatTurn("assistant", "Monday 21:30."))

    clock.advance(TTL + 1)
    built = agent.build_conversation(message(chat_bot, "@bot move it to 22:00"), channel())

    assert [m["role"] for m in built] == ["system", "user"]
    assert "hstar" not in built[-1]["content"]


async def test_an_active_back_and_forth_never_loses_a_live_turn(chat_bot, chat_seeded):
    """Age is per turn, so only the genuinely old end of the deque falls off."""
    agent, clock = pilot(chat_bot, says("ok"))
    agent.remember(channel(), ChatTurn("user", "kanon: the old thing"))
    clock.advance(TTL - 10)
    agent.remember(channel(), ChatTurn("user", "kanon: the new thing"))
    clock.advance(20)  # the first turn is now past the TTL; the second is not

    contents = [turn.content for turn in agent.history(channel())]
    assert contents == ["kanon: the new thing"]


async def test_a_conversation_inside_the_ttl_is_kept_whole(chat_bot, chat_seeded):
    agent, clock = pilot(chat_bot, says("ok"))
    agent.remember(channel(), ChatTurn("user", "kanon: when is hstar?"))
    agent.remember(channel(), ChatTurn("assistant", "Monday 21:30."))

    clock.advance(TTL - 1)
    built = agent.build_conversation(message(chat_bot, "@bot who's on it?"), channel())

    assert [m["role"] for m in built] == ["system", "user", "assistant", "user"]


async def test_answering_stamps_the_exchange_with_the_pilots_clock(chat_bot, chat_seeded):
    """Nothing has to remember to stamp a turn: `remember` is the only door in."""
    agent, clock = pilot(chat_bot, says("Monday 21:30."))
    await agent.offer(message(chat_bot, "@bot when is hstar?"))

    assert [turn.at for turn in agent.history(channel())] == [clock.now, clock.now]

    clock.advance(TTL + 1)
    assert list(agent.history(channel())) == []


async def test_the_ttl_is_the_setting_and_not_a_constant(chat_bot, chat_seeded):
    agent, clock = pilot(chat_bot, says("ok"))
    chat_bot.settings.chat_pilot_history_ttl_s = 30.0
    agent.remember(channel(), ChatTurn("user", "kanon: hello"))

    clock.advance(31)
    assert list(agent.history(channel())) == []


async def test_a_turn_nobody_stamped_is_never_too_old(chat_bot, chat_seeded):
    """A turn with no age has none to be old for -- `assemble` builds several."""
    agent, clock = pilot(chat_bot, says("ok"))
    agent.history(channel()).append(ChatTurn("user", "kanon: unstamped"))

    clock.advance(TTL * 10)
    assert [turn.content for turn in agent.history(channel())] == ["kanon: unstamped"]


# ---------------------------------------------------------------------------
# the current-focus line
# ---------------------------------------------------------------------------


async def test_a_channel_with_no_card_carries_no_focus_line(chat_bot, chat_seeded):
    """Absent, not an empty placeholder: a sentence about nothing is a topic."""
    agent, _clock = pilot(chat_bot, says("Monday 21:30."))
    await agent.offer(message(chat_bot, "@bot when is hstar?"))

    assert persona.FOCUS_PREFIX not in system_of(agent)
    assert agent.focus(channel()) == ""


async def test_a_posted_card_becomes_the_next_prompts_focus_line(chat_bot, chat_seeded):
    """The three-step job: create the run, then say "it" and be understood."""
    agent, _clock = pilot(chat_bot, weekly_card(), says("Card's up — ✅ it."), says("Done."))
    await agent.offer(message(chat_bot, "@bot set up a weekly hbellona at 23:30"))
    await agent.offer(message(chat_bot, "@bot move it to 22:00"))

    card = agent.focus(channel())
    assert card.startswith("new weekly: Hard Bellona every ")
    assert card.endswith("23:30 — kanon, Priya")

    system = system_of(agent)
    assert persona.FOCUS_PREFIX + card in system
    # ...and it was not there before the card existed.
    assert persona.FOCUS_PREFIX not in system_of(agent, 0)


async def test_only_the_last_card_is_kept(chat_bot, chat_seeded):
    """One slot per channel: "it" has one answer, so only one is offered."""
    agent, _clock = pilot(
        chat_bot,
        weekly_card(),
        says("Card's up."),
        weekly_card(boss="HLimbo", participants="Priya"),
        says("That one too."),
        says("ok"),
    )
    await agent.offer(message(chat_bot, "@bot weekly hbellona"))
    await agent.offer(message(chat_bot, "@bot weekly hlimbo"))
    await agent.offer(message(chat_bot, "@bot who's on it?"))

    system = system_of(agent)
    assert "Hard Limbo" in system
    assert "Hard Bellona" not in system


async def test_the_focus_line_expires_on_the_history_ttl(chat_bot, chat_seeded):
    """An hour-old card makes "move it" mean something nobody is discussing."""
    agent, clock = pilot(chat_bot, weekly_card(), says("Card's up."), says("ok"))
    await agent.offer(message(chat_bot, "@bot weekly hbellona"))
    assert agent.focus(channel())

    clock.advance(TTL)
    await agent.offer(message(chat_bot, "@bot what's on?"))

    assert agent.focus(channel()) == ""
    assert persona.FOCUS_PREFIX not in system_of(agent)


async def test_a_refused_write_is_not_a_card(chat_bot, chat_seeded):
    """Only a tool that actually posted one; a refusal put nothing on screen."""
    agent, _clock = pilot(
        chat_bot,
        wants("propose_move", run_query="nothing-like-this", to_when="sunday 22:00"),
        says("Which run did you mean?"),
    )
    await agent.offer(message(chat_bot, "@bot move it"))

    assert agent.focus(channel()) == ""


async def test_a_card_belongs_to_the_channel_it_was_posted_in(chat_bot, chat_seeded):
    agent, _clock = pilot(chat_bot, weekly_card(), says("Card's up."))
    await agent.offer(message(chat_bot, "@bot weekly hbellona"))

    assert agent.focus(channel())
    assert agent.focus("123456789") == ""


async def test_a_card_that_cannot_be_read_is_no_line_and_no_crash(chat_bot, chat_seeded):
    """Context for a nicer answer is never allowed to become part of one."""
    agent, _clock = pilot(chat_bot, says("ok"))
    agent.note_card(channel(), "no-such-amendment")

    assert agent.focus(channel()) == ""
    assert (await agent.offer(message(chat_bot))).answered.reply == "ok"


async def test_forgetting_a_channel_forgets_its_card(chat_bot, chat_seeded):
    agent, _clock = pilot(chat_bot, weekly_card(), says("Card's up."))
    await agent.offer(message(chat_bot, "@bot weekly hbellona"))

    agent.forget(channel())
    assert agent.focus(channel()) == ""


def test_the_focus_line_says_what_it_is_and_says_nothing_when_empty():
    line = persona.focus_line("new weekly: Hard Bellona every Tue 23:30 — kanon, Priya")
    assert line.startswith(persona.FOCUS_PREFIX)
    assert "new weekly: Hard Bellona every Tue 23:30 — kanon, Priya." in line
    assert persona.focus_line("") == ""
    assert persona.focus_line("   ") == ""


def test_the_focus_line_sits_with_the_clock_and_the_runtime_line():
    prompt = persona.system_prompt("A persona.", "CLOCK", "RUNTIME", persona.focus_line("a card"))
    assert prompt.index("RUNTIME") < prompt.index(persona.FOCUS_PREFIX)
    assert prompt.index(persona.FOCUS_PREFIX) < prompt.index(persona.VOICE_PREFIX)


# ---------------------------------------------------------------------------
# re-anchoring a reply
# ---------------------------------------------------------------------------


def reply_to(bot, answer_id: str, content: str = "@bot and who's on it?"):
    """A message replying to one the bot posted, as Discord resolves one."""
    parent = FakeMessage(int(answer_id), bot.channels[CHAT_CHANNEL])
    return message(bot, content, reference=FakeReference(parent))


def last_answer_id(agent: ChatPilot) -> str:
    return agent.history(channel())[-1].message_id


async def test_an_answer_is_remembered_by_the_id_it_went_out_as(chat_bot, chat_seeded):
    agent, _clock = pilot(chat_bot, says("Monday 21:30."))
    await agent.offer(message(chat_bot, "@bot when is hstar?"))

    answer_id = last_answer_id(agent)
    assert answer_id
    assert agent._anchors[answer_id].answer.content == "Monday 21:30."


async def test_a_reply_to_an_aged_out_answer_puts_the_exchange_back(chat_bot, chat_seeded):
    """Somebody replying to a message has said which exchange they mean."""
    agent, clock = pilot(chat_bot, says("Monday 21:30, HStar."), says("You and Alvin."))
    await agent.offer(message(chat_bot, "@bot when is hstar?"))
    answer_id = last_answer_id(agent)

    clock.advance(TTL + 1)
    assert list(agent.history(channel())) == []  # the TTL has taken it

    await agent.offer(reply_to(chat_bot, answer_id))

    prompt = agent._client.conversation(1)
    assert [m["role"] for m in prompt] == ["system", "user", "assistant", "user"]
    assert "when is hstar?" in prompt[1]["content"]
    assert prompt[2]["content"] == "Monday 21:30, HStar."
    assert "who's on it?" in prompt[3]["content"]


async def test_a_reply_to_a_live_exchange_does_not_repeat_it(chat_bot, chat_seeded):
    agent, _clock = pilot(chat_bot, says("Monday 21:30, HStar."), says("You and Alvin."))
    await agent.offer(message(chat_bot, "@bot when is hstar?"))
    await agent.offer(reply_to(chat_bot, last_answer_id(agent)))

    contents = [m["content"] for m in agent._client.prompts[1]]
    assert sum(1 for c in contents if "when is hstar?" in c) == 1
    assert sum(1 for c in contents if c == "Monday 21:30, HStar.") == 1


async def test_a_resolved_parent_and_its_anchor_are_not_both_said(chat_bot, chat_seeded):
    """The two can reach the same message from opposite ends; it is said once."""
    agent, clock = pilot(chat_bot, says("Monday 21:30, HStar."), says("Alvin and you."))
    await agent.offer(message(chat_bot, "@bot when is hstar?"))
    answer_id = last_answer_id(agent)

    clock.advance(TTL + 1)
    # Discord resolved the parent for us, so the reply chain has it too.
    parent = message(chat_bot, "Monday 21:30, HStar.", author_id=BOT_USER_ID, mentions=())
    parent.id = int(answer_id)
    await agent.offer(message(chat_bot, "@bot who's on it?", reference=FakeReference(parent)))

    contents = [m["content"] for m in agent._client.conversation(1)]
    assert contents.count("Monday 21:30, HStar.") == 1


async def test_a_reply_to_a_message_the_bot_has_forgotten_just_proceeds(chat_bot, chat_seeded):
    """No error, no apology: the question is answered without the context."""
    agent, _clock = pilot(chat_bot, says("Sure."))
    result = (await agent.offer(reply_to(chat_bot, "700000000000009999"))).answered

    assert result.reply == "Sure."
    assert [m["role"] for m in agent._client.conversation(0)] == ["system", "user"]


async def test_the_re_anchored_exchange_goes_in_front_of_the_live_history(chat_bot, chat_seeded):
    """It is older than everything else in the prompt, so it sits where it belongs."""
    answers = (says("Monday 21:30, HStar."), says("Kalos is Tuesday."), says("Alvin and you."))
    agent, clock = pilot(chat_bot, *answers)
    await agent.offer(message(chat_bot, "@bot when is hstar?"))
    answer_id = last_answer_id(agent)

    clock.advance(TTL + 1)
    await agent.offer(message(chat_bot, "@bot and kalos?"))
    await agent.offer(reply_to(chat_bot, answer_id))

    contents = [m["content"] for m in agent._client.conversation(2)]
    assert contents.index("Monday 21:30, HStar.") < contents.index("Kalos is Tuesday.")


async def test_an_answer_that_never_posted_is_anchored_to_nothing(chat_bot, chat_seeded):
    """No id to reply to, so no anchor -- and the answer still happened."""

    async def boom(*_args, **_kwargs):
        raise RuntimeError("discord said no")

    chat_bot.post_plain = boom
    agent, _clock = pilot(chat_bot, says("Monday 21:30."))
    assert (await agent.offer(message(chat_bot))).answered.reply == "Monday 21:30."
    assert agent._anchors == {}


async def test_the_anchors_are_bounded(chat_bot, chat_seeded):
    """A dict of every answer the process ever gave is a leak with a good excuse."""
    agent, _clock = pilot(chat_bot, *[says("ok")] * (ANCHOR_CACHE + 5))
    agent.limiter.count = ANCHOR_CACHE + 10
    # The guild's shared pool refuses at twelve however much room the asker has
    # left, and a refused question never reaches `_answer` to be anchored -- so
    # without this the loop below anchors twelve exchanges and stops.
    agent.global_limiter.count = ANCHOR_CACHE + 10
    for _ in range(ANCHOR_CACHE + 5):
        await agent.offer(message(chat_bot))

    assert len(agent._anchors) == ANCHOR_CACHE


# ---------------------------------------------------------------------------
# the setting
# ---------------------------------------------------------------------------


def test_the_ttl_setting_defaults_to_forty_five_minutes():
    assert chat_settings().chat_pilot_history_ttl_s == 2700.0


def test_the_ttl_setting_must_be_positive():
    from pydantic import ValidationError

    with pytest.raises(ValidationError):
        chat_settings(chat_pilot_history_ttl_s=0)


def test_the_env_example_documents_the_ttl():
    from .conftest import REPO_ROOT

    text = (REPO_ROOT / ".env.example").read_text(encoding="utf-8")
    assert "\nCHAT_PILOT_HISTORY_TTL_S=2700" in text
