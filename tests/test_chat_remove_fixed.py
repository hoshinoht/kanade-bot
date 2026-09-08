"""Removing a weekly timing, not just tonight's run.

Live, "remove the fixed run" cancelled the two materialised runs -- the
clarification and the two cards worked exactly as designed -- and the weekly
baseline quietly went on producing more. There was no tool for it.

The dangerous confusion this has to survive is `propose_cancel` vs
`propose_remove_fixed`: one frees an evening, the other stops the guild
scheduling a boss at all, and nothing re-materialises what this removes.
"""

from __future__ import annotations

import pytest

from bot.agent import formatting
from bot.agent.materialise import materialise_week
from bot.chat import tools
from bot.domain.ids import short_id
from bot.extract.commit import FIX_REMOVE, commit, may_commit

from .chat_support import CHAT_CHANNEL
from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ
from .fake_bot import WATCHED_CHANNEL

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def context(bot, author_id: int | str = 1002):
    return tools.ToolContext(
        bot=bot,
        author_id=str(author_id),
        channel_id=str(CHAT_CHANNEL),
        message_id="950000000000000333",
    )


def proposals(bot):
    return bot.repo.list_amendments(status="proposed")


def star_fixed(bot) -> dict:
    return next(f for f in bot.repo.list_fixed_runs() if "HMaleficStar" in f["bosses"])


def approve(bot, row, actor=1001):
    return commit(
        bot.repo,
        row,
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id=actor,
        channel_id=row["channel_id"],
    )


# ---------------------------------------------------------------------------
# resolving which baseline
# ---------------------------------------------------------------------------


def test_a_weekly_timing_resolves_by_boss(chat_bot, chat_seeded):
    assert tools.resolve_fixed(chat_bot, "hstar")["id"] == star_fixed(chat_bot)["id"]


def test_a_weekly_timing_resolves_by_short_id(chat_bot, chat_seeded):
    fixed = star_fixed(chat_bot)
    assert tools.resolve_fixed(chat_bot, short_id(fixed["id"]))["id"] == fixed["id"]


def test_a_day_narrows_between_two_timings_for_one_boss(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(
        1001, ["HMaleficStar"], 4, "20:00", ["1001"], channel_id=WATCHED_CHANNEL
    )
    friday = tools.resolve_fixed(chat_bot, "hstar friday")
    assert friday["weekday"] == 4


def test_two_timings_for_one_boss_with_no_day_is_refused(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(
        1001, ["HMaleficStar"], 4, "20:00", ["1001"], channel_id=WATCHED_CHANNEL
    )
    with pytest.raises(tools.ToolError) as exc:
        tools.resolve_fixed(chat_bot, "hstar")
    assert "more than one weekly timing" in str(exc.value)
    assert "Ask which one" in str(exc.value)
    assert str(exc.value).count("every") == 2  # both candidates named


def test_a_boss_with_no_weekly_timing_is_refused(chat_bot, chat_seeded):
    """Named, and named as the boss it is: see test_chat_change_fixed.py."""
    with pytest.raises(tools.ToolError) as exc:
        tools.resolve_fixed(chat_bot, "bellona")
    assert "No weekly timing for Bellona exists" in str(exc.value)


def test_an_empty_query_asks_rather_than_guessing(chat_bot, chat_seeded):
    with pytest.raises(tools.ToolError) as exc:
        tools.resolve_fixed(chat_bot, "  ")
    assert "Ask them which weekly timing" in str(exc.value)


# ---------------------------------------------------------------------------
# the card
# ---------------------------------------------------------------------------


async def test_it_posts_a_card_and_removes_nothing(chat_bot, chat_seeded):
    fixed = star_fixed(chat_bot)
    answer = await tools.dispatch(context(chat_bot), "propose_remove_fixed", {"query": "hstar"})

    assert "✅" in answer
    assert chat_bot.repo.get_fixed_run(fixed["id"]) is not None
    assert chat_bot.repo.get_run(chat_seeded["star"])["status"] != "cancelled"

    row = proposals(chat_bot)[0]
    assert row["kind"] == "fix"
    assert row["payload"]["op"] == FIX_REMOVE
    assert row["payload"]["fixed_run_id"] == fixed["id"]
    assert row["run_id"] is None
    assert row["proposal_message_id"]


async def test_the_card_says_it_is_the_recurring_one(chat_bot, chat_seeded):
    """It must be unmistakable next to a single-night cancel card."""
    await tools.dispatch(context(chat_bot), "propose_remove_fixed", {"query": "hstar"})
    row = proposals(chat_bot)[0]
    name, value = formatting.proposal_line(row, None, TZ)

    assert "remove weekly" in name
    assert "Hard MaleficStar" in name
    assert "stop scheduling this every week" in value
    assert "future weeks will not be scheduled" in value
    assert "Mon 21:30" in value


async def test_a_single_run_cancel_card_still_reads_as_one_night(chat_bot, chat_seeded):
    await tools.dispatch(
        context(chat_bot), "propose_cancel", {"run_query": short_id(chat_seeded["star"])}
    )
    row = proposals(chat_bot)[0]
    name, value = formatting.proposal_line(row, chat_bot.repo.get_run(row["run_id"]), TZ)
    assert "remove weekly" not in name
    assert value.startswith("**off this week**")


# ---------------------------------------------------------------------------
# approving it
# ---------------------------------------------------------------------------


async def test_a_confirmed_card_removes_the_baseline_and_this_weeks_run(chat_bot, chat_seeded):
    fixed = star_fixed(chat_bot)
    await tools.dispatch(context(chat_bot), "propose_remove_fixed", {"query": "hstar"})
    row = proposals(chat_bot)[0]

    assert may_commit(row, None, 1001, has_role=True) is True
    result = approve(chat_bot, row)

    assert result.applied is True
    assert result.fixed_run_id == fixed["id"]
    assert chat_bot.repo.get_fixed_run(fixed["id"]) is None
    assert chat_bot.repo.get_run(chat_seeded["star"])["status"] == "cancelled"
    # The other party's timing is untouched.
    assert chat_bot.repo.get_run(chat_seeded["kalos"])["status"] != "cancelled"


async def test_future_weeks_stop_materialising_it(chat_bot, chat_seeded):
    """The whole point: no baseline, no more runs from it."""
    from bot.domain.weeks import next_week_start

    await tools.dispatch(context(chat_bot), "propose_remove_fixed", {"query": "hstar"})
    approve(chat_bot, proposals(chat_bot)[0])

    ws = next_week_start(TZ, RESET_WEEKDAY, RESET_TIME)
    materialise_week(chat_bot.repo, ws, TZ, PING_TIME, COUNTDOWNS, now=ws)
    bosses = [r["bosses"] for r in chat_bot.repo.list_runs(week_start=ws)]
    assert ["HMaleficStar", "HFA"] not in bosses
    assert ["XKalos"] in bosses


async def test_a_baseline_removed_twice_fails_the_second_time(chat_bot, chat_seeded):
    await tools.dispatch(context(chat_bot), "propose_remove_fixed", {"query": "hstar"})
    row = proposals(chat_bot)[0]
    approve(chat_bot, row)

    stale = dict(row)
    stale["id"] = row["id"]
    result = approve(chat_bot, stale)
    assert result.applied is False
    assert "already gone" in result.problem


def test_the_card_path_matches_what_fixed_remove_does(chat_bot, chat_seeded):
    """Both routes go through the one helper, so they cannot drift apart."""
    import inspect

    from bot.api import service
    from bot.extract import commit as commit_mod

    assert "retire_fixed_run" in inspect.getsource(service.delete_fixed)
    assert "retire_fixed_run" in inspect.getsource(commit_mod._unfix)


# ---------------------------------------------------------------------------
# steering and injection posture
# ---------------------------------------------------------------------------


def test_the_two_removal_tools_point_at_each_other():
    schemas = {t["function"]["name"]: t["function"]["description"] for t in tools.TOOLS}
    assert "propose_remove_fixed" in schemas["propose_cancel"]
    assert "ONE dated run" in schemas["propose_cancel"]
    assert "propose_cancel" in schemas["propose_remove_fixed"]
    assert "RECURRING" in schemas["propose_remove_fixed"]
    assert "weekly one?" in schemas["propose_remove_fixed"]


async def test_delete_fixed_is_not_reachable_as_a_tool(chat_bot, chat_seeded):
    fixed = star_fixed(chat_bot)
    for name in ("delete_fixed", "remove_fixed", "update_fixed", "create_fixed"):
        answer = await tools.dispatch(context(chat_bot), name, {"fixed_id": fixed["id"]})
        assert "There is no tool called" in answer
    assert chat_bot.repo.get_fixed_run(fixed["id"]) is not None


async def test_the_model_cannot_name_a_different_baseline_in_the_payload(chat_bot, chat_seeded):
    """The id on the card comes from resolution here, not from the tool call."""
    kalos = next(f for f in chat_bot.repo.list_fixed_runs() if "XKalos" in f["bosses"])
    await tools.dispatch(
        context(chat_bot),
        "propose_remove_fixed",
        {"query": "hstar", "fixed_run_id": kalos["id"], "fixed_id": kalos["id"]},
    )
    assert proposals(chat_bot)[0]["payload"]["fixed_run_id"] == star_fixed(chat_bot)["id"]
