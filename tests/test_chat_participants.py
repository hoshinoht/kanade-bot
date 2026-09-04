"""Who ends up on a run the chatbot drafts.

Two names a roster lookup can never resolve, and both bit live:

* **the bot itself.** The trigger mention is part of the message the model is
  reading, and it passed it back as a participant. `validate_participants`
  refused with "not in the bossing role: user 5555 (…)" and the model relayed
  that in the first person -- "I'm not in the bossing role" -- which is untrue
  and baffling. The bot is the thing being spoken to, not a member.
* **"me".** The commonest way anybody says who a run is for, and the one the
  model must never resolve itself.
"""

from __future__ import annotations

import pytest

from bot.chat import tools

from .chat_support import BOT_USER_ID, CHAT_CHANNEL

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def context(bot, author_id: int | str = 1002):
    return tools.ToolContext(
        bot=bot,
        author_id=str(author_id),
        channel_id=str(CHAT_CHANNEL),
        message_id="950000000000000444",
    )


def tomorrow_at(hhmm: str = "21:30") -> str:
    from datetime import timedelta

    from bot.domain.timeutil import utcnow

    from .conftest import TZ

    return (utcnow().astimezone(TZ) + timedelta(days=1)).strftime(f"%Y-%m-%d {hhmm}")


async def add(bot, participants, author_id: int | str = 1002) -> str:
    return await tools.dispatch(
        context(bot, author_id),
        "propose_add",
        {"boss": "HBellona", "when": tomorrow_at(), "participants": participants},
    )


def party(bot) -> list[str]:
    return bot.repo.list_amendments(status="proposed")[0]["participants"]


# ---------------------------------------------------------------------------
# the bot is never a participant
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "text",
    [
        f"<@{BOT_USER_ID}>",
        f"<@!{BOT_USER_ID}>",
        str(BOT_USER_ID),
        "YuukiSakuna",
        "yuukisakuna",
        f"  <@{BOT_USER_ID}>  ",
    ],
)
async def test_the_bot_alone_falls_back_to_the_asker(chat_bot, chat_seeded, text):
    """The live failure: the trigger mention copied into the participants field.

    It must not become "I'm not in the bossing role" -- there is nobody left
    once the bot is removed, so the run is for the person who asked.
    """
    answer = await add(chat_bot, text)
    assert "not in the bossing role" not in answer
    assert "✅" in answer
    assert party(chat_bot) == ["1002"]


async def test_the_bot_is_dropped_from_a_list_that_also_names_people(chat_bot, chat_seeded):
    await add(chat_bot, f"<@{BOT_USER_ID}>, kanon, Priya")
    assert party(chat_bot) == ["1002", "1003"]
    assert str(BOT_USER_ID) not in party(chat_bot)


async def test_the_bots_name_inside_a_sentence_becomes_the_asker(chat_bot, chat_seeded):
    """The bot is removed, and the person who asked takes its place.

    "@bot and Priya" is somebody saying "Priya and me": the model puts the
    trigger mention where the member wrote "me". Dropping it and stopping there
    is how a card went up for Priya alone -- see
    `test_a_stripped_bot_reference_puts_the_asker_on_the_run`.
    """
    await add(chat_bot, "YuukiSakuna and Priya", author_id=1002)
    assert party(chat_bot) == ["1002", "1003"]
    assert str(BOT_USER_ID) not in party(chat_bot)


async def test_a_stripped_bot_reference_puts_the_asker_on_the_run(chat_bot, chat_seeded):
    """The live failure: "schedule it for me and @Priya".

    The model sent the two mentions the message contained -- the trigger mention
    it was addressed by, and Priya. The bot came out, Priya survived, and
    because the field was not empty the asker was never defaulted in: the card
    went up for Priya alone, and the model then told the channel the run was for
    the bot and Priya. Neither half of that sentence was true.
    """
    await add(chat_bot, f"<@{BOT_USER_ID}> and <@1003>", author_id=1002)
    assert party(chat_bot) == ["1002", "1003"]


async def test_a_named_member_alone_does_not_drag_the_asker_in(chat_bot, chat_seeded):
    """No bot reference and no "me": the field means exactly who it names.

    Somebody scheduling a run for other people is an ordinary thing to ask for,
    and the asker must not be added to it.
    """
    await add(chat_bot, "<@1003>", author_id=1002)
    assert party(chat_bot) == ["1003"]


async def test_a_name_that_merely_contains_the_bots_name_is_untouched(chat_bot, chat_seeded):
    """Word-bounded: stripping substrings would eat real members."""
    chat_bot.repo.upsert_member(1005, "YuukiSakunaFan", "YuukiSakunaFan", True)
    await add(chat_bot, "YuukiSakunaFan")
    assert party(chat_bot) == ["1005"]


# ---------------------------------------------------------------------------
# "me" is the asker, from the message
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("word", ["me", "Me", "myself", "I", "i"])
async def test_the_first_person_resolves_to_the_asker(chat_bot, chat_seeded, word):
    await add(chat_bot, word, author_id=1003)
    assert party(chat_bot) == ["1003"]


async def test_me_and_another_member_resolves_to_both(chat_bot, chat_seeded):
    await add(chat_bot, "me and <@1003>", author_id=1002)
    assert party(chat_bot) == ["1002", "1003"]


async def test_add_me_works(chat_bot, chat_seeded):
    await add(chat_bot, "add me", author_id=1002)
    assert party(chat_bot) == ["1002"]


async def test_the_trigger_mention_and_me_together(chat_bot, chat_seeded):
    """What the live message actually looked like once the model copied it."""
    await add(chat_bot, f"<@{BOT_USER_ID}> me", author_id=1002)
    assert party(chat_bot) == ["1002"]


async def test_the_asker_comes_from_the_message_not_the_model(chat_bot, chat_seeded):
    """`me` is whoever wrote the message, whatever the model thinks."""
    await add(chat_bot, "me", author_id=1003)
    assert party(chat_bot) == ["1003"]


async def test_a_first_person_word_inside_a_name_is_not_substituted(chat_bot, chat_seeded):
    """Word-bounded: `i` must not rewrite the i in Priya."""
    await add(chat_bot, "Priya")
    assert party(chat_bot) == ["1003"]


# ---------------------------------------------------------------------------
# everything else still refuses as before
# ---------------------------------------------------------------------------


async def test_a_stranger_is_still_refused(chat_bot, chat_seeded):
    answer = await add(chat_bot, "Nobody McGhost")
    assert "Nobody on the roster matches" in answer
    assert chat_bot.repo.list_amendments(status="proposed") == []


async def test_an_invented_snowflake_is_still_refused(chat_bot, chat_seeded):
    answer = await add(chat_bot, "424242424242424242")
    assert "not in the bossing role" in answer
    assert chat_bot.repo.list_amendments(status="proposed") == []


# ---------------------------------------------------------------------------
# what the model is told it just did
# ---------------------------------------------------------------------------


async def test_the_tool_result_names_the_resolved_party(chat_bot, chat_seeded):
    """The card's own party, by display name, so a reply can only echo it."""
    answer = await add(chat_bot, f"<@{BOT_USER_ID}> and <@1003>", author_id=1002)

    assert "kanon" in answer and "Priya" in answer
    assert "Hard Bellona" in answer
    assert str(BOT_USER_ID) not in answer
    assert "YuukiSakuna" not in answer


async def test_the_result_names_the_asker_even_when_the_model_named_nobody(chat_bot, chat_seeded):
    answer = await add(chat_bot, "", author_id=1003)
    assert "Priya" in answer


async def test_a_reply_can_be_built_from_the_tool_result_alone(chat_bot, chat_seeded):
    """End to end over a scripted model: what comes back is what can be said.

    The model is given the same tool call that failed live -- both mentions from
    the message, the bot's included -- and the `tool` message it then reads is
    asserted on directly. It names the two real people and neither names nor
    mentions the bot, so a reply composed from it cannot put the bot on the run
    or leave the asker off it.
    """
    from bot.chat.agent import ChatPilot

    from .chat_support import FakeOllama, message, says, wants

    agent = ChatPilot(
        chat_bot,
        client=FakeOllama(
            wants(
                "propose_add",
                boss="HBellona",
                when=tomorrow_at(),
                participants=f"<@{BOT_USER_ID}> <@1003>",
            ),
            says("Card up for Hard Bellona -- kanon and Priya. React ✅ to confirm."),
        ),
    )
    result = (
        await agent.offer(
            message(chat_bot, f"{BOT_USER_ID} schedule hbellona tomorrow for me and <@1003>")
        )
    ).answered

    tool_message = agent._client.conversation(1)[-1]
    assert tool_message["role"] == "tool"
    assert "kanon" in tool_message["content"] and "Priya" in tool_message["content"]
    assert str(BOT_USER_ID) not in tool_message["content"]
    assert "YuukiSakuna" not in tool_message["content"]
    # And the card really is for those two.
    assert party(chat_bot) == ["1002", "1003"]
    assert result.reply


async def test_the_refusal_never_speaks_about_the_bot_in_the_first_person(chat_bot, chat_seeded):
    """Whatever is refused, it is never the bot reporting itself as a member."""
    for text in (f"<@{BOT_USER_ID}>", "YuukiSakuna", "424242424242424242", "Nobody McGhost"):
        answer = await add(chat_bot, text)
        assert str(BOT_USER_ID) not in answer
        assert "YuukiSakuna" not in answer
        for row in chat_bot.repo.list_amendments():
            assert str(BOT_USER_ID) not in row["participants"]
