"""`propose_add`: scheduling a run that does not exist yet.

The live gap this closes: "schedule a new run for Bellona today at 2130" had no
tool behind it and the model could only apologise. It rides the extractor's
existing `add` kind and its commit handler, so the card and the ✅ that applies
it are the ones the extractor already produces from overheard chat.
"""

from __future__ import annotations

import pytest

from bot.chat import tools
from bot.extract.commit import commit, may_commit

from .chat_support import CHAT_CHANNEL
from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def context(bot, author_id: int | str = 1002):
    return tools.ToolContext(
        bot=bot,
        author_id=str(author_id),
        channel_id=str(CHAT_CHANNEL),
        message_id="950000000000000555",
    )


def proposals(bot):
    return bot.repo.list_amendments(status="proposed")


def tomorrow_at(bot, hhmm: str = "21:30") -> str:
    from datetime import timedelta

    from bot.timeutil import utcnow

    return (utcnow().astimezone(TZ) + timedelta(days=1)).strftime(f"%Y-%m-%d {hhmm}")


# ---------------------------------------------------------------------------
# the happy path
# ---------------------------------------------------------------------------


async def test_it_posts_a_card_and_creates_no_run(chat_bot, chat_seeded):
    before = len(chat_bot.repo.list_runs())
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_add",
        {"boss": "HBellona", "when": tomorrow_at(chat_bot)},
    )

    assert "✅" in answer
    assert len(chat_bot.repo.list_runs()) == before  # nothing scheduled yet

    row = proposals(chat_bot)[0]
    assert row["kind"] == "add"
    assert row["run_id"] is None
    assert row["bosses"] == ["HBellona"]
    assert row["new_datetime"].astimezone(TZ).strftime("%H:%M") == "21:30"
    assert row["channel_id"] == str(CHAT_CHANNEL)
    assert row["evidence_msg_ids"] == ["950000000000000555"]
    assert row["proposal_message_id"]


async def test_the_asker_is_the_default_party(chat_bot, chat_seeded):
    await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_add",
        {"boss": "HBellona", "when": tomorrow_at(chat_bot)},
    )
    assert proposals(chat_bot)[0]["participants"] == ["1002"]


async def test_a_named_party_is_resolved_against_the_roster(chat_bot, chat_seeded):
    await tools.dispatch(
        context(chat_bot),
        "propose_add",
        {"boss": "HBellona", "when": tomorrow_at(chat_bot), "participants": "kanon, Priya"},
    )
    assert proposals(chat_bot)[0]["participants"] == ["1002", "1003"]


async def test_a_confirmed_card_creates_the_run(chat_bot, chat_seeded):
    """Through the extractor's own `_add` handler and the real ✅ path."""
    await tools.dispatch(
        context(chat_bot),
        "propose_add",
        {"boss": "HBellona", "when": tomorrow_at(chat_bot)},
    )
    row = proposals(chat_bot)[0]
    assert may_commit(row, None, 1002, has_role=True) is True

    result = commit(
        chat_bot.repo,
        row,
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id=1002,
        channel_id=row["channel_id"],
    )
    assert result.applied is True
    created = chat_bot.repo.get_run(result.created_run_ids[0])
    assert created["bosses"] == ["HBellona"]
    assert created["participants"] == ["1002"]
    assert created["channel_id"] == str(CHAT_CHANNEL)
    assert chat_bot.repo.list_reminders(created["id"])  # it gets its pings


# ---------------------------------------------------------------------------
# refusals -- each one has to tell the model what to ask
# ---------------------------------------------------------------------------


async def test_a_boss_without_a_difficulty_is_refused_with_the_valid_forms(chat_bot, chat_seeded):
    """The live case: "bellona" is three different fights in game."""
    answer = await tools.dispatch(
        context(chat_bot), "propose_add", {"boss": "bellona", "when": tomorrow_at(chat_bot)}
    )
    assert "missing a difficulty prefix" in answer
    for form in ("EBellona", "NBellona", "HBellona"):
        assert form in answer
    assert "Ask them" in answer
    assert "do not choose a difficulty" in answer
    assert proposals(chat_bot) == []


async def test_a_difficulty_the_boss_does_not_have_is_refused(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot), "propose_add", {"boss": "xbellona", "when": tomorrow_at(chat_bot)}
    )
    assert "no Extreme difficulty" in answer
    assert proposals(chat_bot) == []


@pytest.mark.parametrize("boss", ["zakum", "", "   "])
async def test_an_unknown_or_missing_boss_is_refused(chat_bot, chat_seeded, boss):
    answer = await tools.dispatch(
        context(chat_bot), "propose_add", {"boss": boss, "when": tomorrow_at(chat_bot)}
    )
    assert "Ask them" in answer
    assert proposals(chat_bot) == []


async def test_a_missing_time_is_refused_with_a_question(chat_bot, chat_seeded):
    answer = await tools.dispatch(context(chat_bot), "propose_add", {"boss": "HBellona"})
    assert answer == "Ask them what day and time the run should be."
    assert proposals(chat_bot) == []


async def test_an_unreadable_time_is_refused_with_a_question(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot), "propose_add", {"boss": "HBellona", "when": "whenever lah"}
    )
    assert "couldn't read" in answer
    assert "Ask them for the day and time" in answer
    assert proposals(chat_bot) == []


async def test_a_time_in_the_past_is_refused(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot), "propose_add", {"boss": "HBellona", "when": "2020-01-01 21:30"}
    )
    assert "in the past" in answer
    assert proposals(chat_bot) == []


async def test_a_participant_nobody_matches_is_refused(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_add",
        {"boss": "HBellona", "when": tomorrow_at(chat_bot), "participants": "Nobody McGhost"},
    )
    assert "Nobody on the roster matches" in answer
    assert proposals(chat_bot) == []


async def test_a_participant_without_the_bossing_role_is_refused(chat_bot, chat_seeded):
    """`NotABosser` is in the members table with `has_role` false.

    Refused a layer earlier than the invented-id case below: the roster the
    model may name people from is the bossing role, so somebody outside it is
    not a name that resolves at all. `validate_participants` still backstops the
    id path, which does not go through name matching.
    """
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_add",
        {"boss": "HBellona", "when": tomorrow_at(chat_bot), "participants": "NotABosser"},
    )
    assert "Nobody on the roster matches" in answer
    assert proposals(chat_bot) == []


async def test_the_model_cannot_invent_a_member_id(chat_bot, chat_seeded):
    """A bare snowflake nobody holds is not a person; it must not reach a run."""
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_add",
        {
            "boss": "HBellona",
            "when": tomorrow_at(chat_bot),
            "participants": "424242424242424242",
        },
    )
    assert "not in the bossing role" in answer
    assert proposals(chat_bot) == []


# ---------------------------------------------------------------------------
# injection posture
# ---------------------------------------------------------------------------


async def test_extra_arguments_do_not_change_who_or_where(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_add",
        {
            "boss": "HBellona",
            "when": tomorrow_at(chat_bot),
            "channel_id": "999",
            "author_id": "1001",
            "status": "confirmed",
            "run_id": "whatever",
        },
    )
    assert "✅" in answer
    row = proposals(chat_bot)[0]
    assert row["participants"] == ["1002"]
    assert row["channel_id"] == str(CHAT_CHANNEL)
    assert row["run_id"] is None
    assert row["status"] == "proposed"


async def test_an_add_supersedes_an_earlier_card_for_the_same_bosses(chat_bot, chat_seeded):
    """The extractor's own supersede rule, unchanged: one live card per thing."""
    for _ in range(2):
        await tools.dispatch(
            context(chat_bot),
            "propose_add",
            {"boss": "HBellona", "when": tomorrow_at(chat_bot)},
        )
    assert len(proposals(chat_bot)) == 1
    assert len(chat_bot.repo.list_amendments(status="superseded")) == 1
