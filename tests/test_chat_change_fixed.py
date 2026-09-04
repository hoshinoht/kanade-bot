"""Changing a weekly timing that already exists, rather than adding another one.

Live, a member asked the bot to "update this hard limbo timing to 23:30". There
was no tool for it: the model proposed moving the one run this week's
materialisation had produced (`propose_move` -- rejected, that was not the ask),
was told "make it every week", and then created a *second* weekly at 23:30 with
`propose_add`. The 21:30 baseline stayed, nothing removed it, and the two other
members were left on the old one.

So the tests that matter here are the ones about identity: the ✅ changes the
same row -- same fixed id, same run id, one weekly where there was one -- and a
query that could mean either of two timings ends in a question rather than a
guess.
"""

from __future__ import annotations

from datetime import time as clock_time

import pytest

from bot.agent import formatting
from bot.agent.materialise import materialise_week
from bot.chat import tools
from bot.domain.ids import short_id
from bot.domain.weeks import next_week_start, slot_in_week
from bot.extract.commit import FIX_EDIT, commit, may_commit

from .chat_support import ADOPTED_CHANNEL, CHAT_CHANNEL
from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ
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
        message_id="950000000000000444",
        is_admin=is_admin,
    )


def proposals(bot):
    return bot.repo.list_amendments(status="proposed")


def star_fixed(bot) -> dict:
    """The seeded HStar + HFA weekly: Mon 21:30, owned by 1001, with 1001 and 1002."""
    return next(f for f in bot.repo.list_fixed_runs() if "HStar" in f["bosses"])


def materialise_both_weeks(bot) -> None:
    """What ``BossBot.materialise_weeks`` does, without the live client."""
    from bot.domain.timeutil import utcnow
    from bot.domain.weeks import current_week_start, week_end

    now = utcnow()
    this_week = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME, now)
    for ws in (this_week, week_end(this_week, TZ)):
        materialise_week(bot.repo, ws, TZ, PING_TIME, COUNTDOWNS, now=now)


def approve(bot, row, actor=1002):
    """The ✅ path, wired to the same ``on_fixed_created`` the client wires."""
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
        on_fixed_created=lambda _fixed_id: materialise_both_weeks(bot),
    )


async def ask(bot, ctx=None, **arguments):
    return await tools.dispatch(ctx or context(bot), "propose_change_fixed", arguments)


# ---------------------------------------------------------------------------
# which timing -- never a guess
# ---------------------------------------------------------------------------


async def test_a_boss_with_one_weekly_timing_needs_no_day(chat_bot, chat_seeded):
    """A vague query is fine when it can only mean one thing."""
    answer = await ask(chat_bot, query="hstar", time="23:30")
    assert "✅" in answer
    assert proposals(chat_bot)[0]["payload"]["fixed_run_id"] == star_fixed(chat_bot)["id"]


async def test_two_timings_for_one_boss_end_in_a_question(chat_bot, chat_seeded):
    """Same boss, two nights: 'hstar' could mean either, so it must ask."""
    chat_bot.repo.add_fixed_run(
        1001, ["HStar"], 4, "20:00", ["1001", "1003"], channel_id=WATCHED_CHANNEL
    )
    answer = await ask(chat_bot, query="hstar", time="23:30")

    assert "more than one weekly timing" in answer
    assert "Ask which one they mean" in answer
    assert "do not pick one yourself" in answer
    assert "every Mon 21:30" in answer and "every Fri 20:00" in answer
    assert proposals(chat_bot) == []


async def test_the_candidates_are_named_well_enough_to_choose_between(chat_bot, chat_seeded):
    """Boss, night and party -- 'the Friday one with Priya' has to be answerable."""
    chat_bot.repo.add_fixed_run(
        1001, ["HStar"], 4, "20:00", ["1001", "1003"], channel_id=WATCHED_CHANNEL
    )
    answer = await ask(chat_bot, query="hstar", time="23:30")
    friday = next(line for line in answer.split("; ") if "Fri" in line)

    assert "Hard Star" in friday
    assert "Priya" in friday
    for fixed in chat_bot.repo.list_fixed_runs():
        if "HStar" in fixed["bosses"]:
            assert short_id(fixed["id"]) in answer


async def test_the_same_boss_on_the_same_day_still_ends_in_a_question(chat_bot, chat_seeded):
    """Only the time tells these two apart, and the query does not carry one."""
    chat_bot.repo.add_fixed_run(
        1001, ["HStar"], 0, "23:30", ["1001", "1003"], channel_id=WATCHED_CHANNEL
    )
    answer = await ask(chat_bot, query="hstar monday", day="wednesday")

    assert "more than one weekly timing" in answer
    assert "every Mon 21:30" in answer and "every Mon 23:30" in answer
    assert proposals(chat_bot) == []


async def test_the_short_id_ends_the_ambiguity(chat_bot, chat_seeded):
    """What the refusal tells the model to come back with."""
    chat_bot.repo.add_fixed_run(
        1001, ["HStar"], 0, "23:30", ["1001", "1003"], channel_id=WATCHED_CHANNEL
    )
    fixed = star_fixed(chat_bot)
    answer = await ask(chat_bot, query=short_id(fixed["id"]), day="wednesday")

    assert "✅" in answer
    assert proposals(chat_bot)[0]["payload"]["fixed_run_id"] == fixed["id"]


async def test_a_query_that_names_nothing_asks_which_boss(chat_bot, chat_seeded):
    """Two different bosses, and nothing in the text picks either: refuse."""
    answer = await ask(chat_bot, query="the weekly run", time="23:30")
    assert "No weekly timing matches" in answer
    assert "which boss's weekly run" in answer
    assert proposals(chat_bot) == []


async def test_a_day_that_two_bosses_share_ends_in_a_question(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(
        1002, ["HBellona"], 0, "20:00", ["1002"], channel_id=WATCHED_CHANNEL
    )
    answer = await ask(chat_bot, query="monday", time="23:30")

    assert "more than one weekly timing" in answer
    assert "Hard Star" in answer and "Hard Bellona" in answer
    assert proposals(chat_bot) == []


# ---------------------------------------------------------------------------
# a boss with no weekly at all -- the day token must not answer for it
# ---------------------------------------------------------------------------


async def test_a_boss_with_no_weekly_is_not_answered_with_other_bosses_nights(
    chat_bot, chat_seeded
):
    """The live failure, whole.

    A member had a one-off Hard Jupiter on Tuesday and asked to move it to 23:00
    "and make it run for every week". No Jupiter weekly exists, so the boss
    search found nothing, the token "tue" survived, and the tool answered with
    three other parties' Tuesday nights -- which the model relayed as "which Hard
    Jupiter weekly are you tweaking?".
    """
    chat_bot.repo.add_fixed_run(1001, ["HCarling"], 1, "23:00", ["1001"], channel_id=CHAT_CHANNEL)
    answer = await ask(chat_bot, query="hard jupiter tue", time="23:00")

    assert "No weekly timing for Jupiter exists" in answer
    assert "more than one weekly timing" not in answer
    assert "Carling" not in answer and "Kalos" not in answer
    assert proposals(chat_bot) == []


async def test_the_refusal_points_at_the_tool_that_would_have_worked(chat_bot, chat_seeded):
    """ "Make it every week" about a run that exists is `propose_add`, not this."""
    answer = await ask(chat_bot, query="hjupiter", time="23:00")

    assert "propose_add with weekly = true" in answer
    assert "folds this week's run into the new weekly" in answer
    assert "duplicate" in answer


@pytest.mark.parametrize("query", ["jupiter", "hjupiter", "hard jupiter", "jup thursday"])
async def test_every_spelling_of_the_missing_boss_is_recognised(chat_bot, chat_seeded, query):
    """Short name, prefixed token, difficulty in words, and an alias with a day."""
    answer = await ask(chat_bot, query=query, time="23:00")
    assert "No weekly timing for Jupiter exists" in answer


async def test_a_boss_that_does_have_a_weekly_never_gets_that_refusal(chat_bot, chat_seeded):
    """The guard is about absence, and `hlimb` is how the guild says Limbo.

    The boss search below it does not know the table's aliases, so this query
    finds nothing either -- but "no weekly timing for Limbo exists" would be
    flatly untrue, and the vaguer refusal is the honest one.
    """
    chat_bot.repo.add_fixed_run(1001, ["HLimbo"], 2, "22:00", ["1001"], channel_id=CHAT_CHANNEL)
    answer = await ask(chat_bot, query="hlimb", time="23:00")

    assert "No weekly timing for Limbo exists" not in answer
    assert "No weekly timing matches" in answer
    assert proposals(chat_bot) == []


async def test_a_day_only_query_still_disambiguates(chat_bot, chat_seeded):
    """Nothing above may reach a query that named no boss: it must still list."""
    chat_bot.repo.add_fixed_run(1002, ["HBellona"], 1, "20:00", ["1002"], channel_id=CHAT_CHANNEL)
    answer = await ask(chat_bot, query="tuesday", time="23:30")

    assert "more than one weekly timing" in answer
    assert "Extreme Kalos" in answer and "Hard Bellona" in answer
    assert proposals(chat_bot) == []


async def test_a_boss_and_a_day_together_still_resolve(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(
        1001, ["HStar"], 4, "20:00", ["1001", "1002"], channel_id=WATCHED_CHANNEL
    )
    answer = await ask(chat_bot, query="hstar friday", time="23:30")

    assert "✅" in answer
    assert proposals(chat_bot)[0]["payload"]["weekly_when"] == "Fri 20:00"


# ---------------------------------------------------------------------------
# the card -- and nothing else
# ---------------------------------------------------------------------------


async def test_a_new_time_posts_a_card_and_changes_nothing(chat_bot, chat_seeded):
    fixed = star_fixed(chat_bot)
    before = chat_bot.repo.get_run(chat_seeded["star"])["datetime"]

    answer = await ask(chat_bot, query="hstar", time="23:30")

    assert "✅" in answer
    assert chat_bot.repo.get_fixed_run(fixed["id"])["time"] == "21:30"
    assert chat_bot.repo.get_run(chat_seeded["star"])["datetime"] == before

    row = proposals(chat_bot)[0]
    assert row["kind"] == "fix"
    assert row["run_id"] is None
    assert row["payload"]["op"] == FIX_EDIT
    assert row["payload"]["fixed_run_id"] == fixed["id"]
    assert row["payload"]["weekday"] == 0
    assert row["payload"]["time"] == "23:30"
    assert row["payload"]["weekly_when"] == "Mon 21:30"
    assert "participants" not in row["payload"]
    assert row["proposal_message_id"]


async def test_a_new_day_keeps_the_time_it_already_has(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", day="wednesday")
    payload = proposals(chat_bot)[0]["payload"]
    assert payload["weekday"] == 2
    assert payload["time"] == "21:30"


async def test_a_new_day_and_time_together(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", day="fri", time="10:15pm")
    payload = proposals(chat_bot)[0]["payload"]
    assert payload["weekday"] == 4
    assert payload["time"] == "22:15"


async def test_a_new_party_alone_leaves_the_night_out_of_the_payload(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", participants="kanon, Priya")
    payload = proposals(chat_bot)[0]["payload"]

    assert payload["participants"] == ["1002", "1003"]
    assert "weekday" not in payload and "time" not in payload
    # The row's own party stays the one it has now: that is who may press ✅.
    assert proposals(chat_bot)[0]["participants"] == ["1001", "1002"]


async def test_a_new_night_and_a_new_party_at_once(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", time="23:30", participants="kanon, Priya")
    payload = proposals(chat_bot)[0]["payload"]
    assert payload["time"] == "23:30"
    assert payload["participants"] == ["1002", "1003"]


async def test_an_omitted_party_leaves_the_party_alone(chat_bot, chat_seeded):
    """The opposite of `propose_add`, where an empty field means the asker."""
    await ask(chat_bot, query="hstar", time="23:30")
    assert "participants" not in proposals(chat_bot)[0]["payload"]

    approve(chat_bot, proposals(chat_bot)[0])
    assert star_fixed(chat_bot)["participants"] == ["1001", "1002"]


async def test_the_trigger_mention_is_not_a_new_party(chat_bot, chat_seeded):
    """A model that copies the @bot into the field must not cut the party to one."""
    from .chat_support import BOT_USER_ID

    await ask(chat_bot, query="hstar", time="23:30", participants=f"<@{BOT_USER_ID}>")
    assert "participants" not in proposals(chat_bot)[0]["payload"]


async def test_the_tool_result_reads_back_both_sides_of_the_change(chat_bot, chat_seeded):
    answer = await ask(chat_bot, query="hstar", time="23:30", participants="kanon, Priya")

    assert "Hard Star + Hard FA" in answer
    assert "every Mon 21:30 → every Mon 23:30" in answer
    assert "Alvin tan, kanon → kanon, Priya" in answer


async def test_the_result_still_names_the_night_when_only_the_party_changes(chat_bot, chat_seeded):
    """It has to say which timing it carded; there may be two for that boss."""
    answer = await ask(chat_bot, query="hstar", participants="kanon, Priya")
    assert "every Mon 21:30 (same night)" in answer


# ---------------------------------------------------------------------------
# refusals
# ---------------------------------------------------------------------------


async def test_a_call_that_changes_nothing_asks_what_should_change(chat_bot, chat_seeded):
    answer = await ask(chat_bot, query="hstar")
    assert "would change" in answer
    assert "the day, the time, or who is on it" in answer
    assert proposals(chat_bot) == []


async def test_naming_the_night_it_already_has_changes_nothing(chat_bot, chat_seeded):
    answer = await ask(chat_bot, query="hstar", day="monday", time="21:30")
    assert "would change" in answer
    assert proposals(chat_bot) == []


async def test_naming_the_party_it_already_has_changes_nothing(chat_bot, chat_seeded):
    answer = await ask(chat_bot, query="hstar", participants="Alvin tan, kanon")
    assert "would change" in answer
    assert proposals(chat_bot) == []


async def test_an_unreadable_day_is_refused_with_a_question(chat_bot, chat_seeded):
    answer = await ask(chat_bot, query="hstar", day="whenever lah")
    assert "unknown weekday" in answer
    assert "Ask them which day of the week" in answer
    assert proposals(chat_bot) == []


async def test_an_unreadable_time_is_refused_with_a_question(chat_bot, chat_seeded):
    answer = await ask(chat_bot, query="hstar", time="late-ish")
    assert "expected a time like" in answer
    assert "Ask them what time it should start" in answer
    assert proposals(chat_bot) == []


async def test_a_party_nobody_matches_is_refused(chat_bot, chat_seeded):
    answer = await ask(chat_bot, query="hstar", participants="Nobody McGhost")
    assert "Nobody on the roster matches" in answer
    assert proposals(chat_bot) == []


# ---------------------------------------------------------------------------
# authority -- the same rule as every other fixed-timing write
# ---------------------------------------------------------------------------


async def test_a_timing_that_is_not_theirs_is_refused(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(1001, ["HBellona"], 3, "21:00", ["1001"], channel_id=CHAT_CHANNEL)
    answer = await ask(
        chat_bot, ctx=context(chat_bot, author_id=1002), query="hbellona", time="23:30"
    )

    assert "not theirs" in answer
    assert "Do not name anybody on it" in answer
    assert proposals(chat_bot) == []


async def test_the_owner_may_change_a_timing_they_are_not_on(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(1002, ["HBellona"], 3, "21:00", ["1001"], channel_id=CHAT_CHANNEL)
    answer = await ask(
        chat_bot, ctx=context(chat_bot, author_id=1002), query="hbellona", time="23:30"
    )
    assert "✅" in answer


async def test_a_timing_in_another_chat_channel_is_refused(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(
        1002, ["HBellona"], 3, "21:00", ["1002"], channel_id=ADOPTED_CHANNEL
    )
    answer = await ask(
        chat_bot, ctx=context(chat_bot, author_id=1002), query="hbellona", time="23:30"
    )

    assert f"<#{ADOPTED_CHANNEL}>" in answer
    assert "its own channel" in answer
    assert proposals(chat_bot) == []


async def test_an_admin_may_change_any_timing_from_anywhere(chat_bot, chat_seeded):
    chat_bot.repo.add_fixed_run(
        1001, ["HBellona"], 3, "21:00", ["1001"], channel_id=ADOPTED_CHANNEL
    )
    answer = await ask(
        chat_bot,
        ctx=context(chat_bot, author_id=1009, is_admin=True),
        query="hbellona",
        time="23:30",
    )
    assert "✅" in answer


async def test_a_read_only_turn_cannot_post_one(chat_bot, chat_seeded):
    """The rejection follow-up may only ever ask a question."""
    ctx = context(chat_bot)
    ctx.read_only = True
    answer = await tools.dispatch(ctx, "propose_change_fixed", {"query": "hstar", "time": "23:30"})

    assert "cannot post a card" in answer
    assert proposals(chat_bot) == []


def test_it_is_not_offered_to_a_read_only_turn(chat_bot):
    assert "propose_change_fixed" not in [t["function"]["name"] for t in tools.read_tools()]


# ---------------------------------------------------------------------------
# the card's wording -- three `fix` cards that must never be confused
# ---------------------------------------------------------------------------


async def test_the_card_says_it_changes_the_weekly(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", time="23:30")
    name, value = formatting.proposal_line(proposals(chat_bot)[0], None, TZ)

    assert "change weekly" in name
    assert "Hard Star + Hard FA" in name
    assert "~~every Mon 21:30~~ → **every Mon 23:30**" in value
    assert "every week from now on" in value
    assert "this week's run moves with it" in value


async def test_the_card_shows_the_party_change_as_old_to_new(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", participants="kanon, Priya")
    _name, value = formatting.proposal_line(proposals(chat_bot)[0], None, TZ)
    party = next(line for line in value.splitlines() if "→" in line)

    assert "1001" in party and "1003" in party
    assert party.index("1001") < party.index("→") < party.index("1003")
    # The old party is not repeated underneath as if it were the new one.
    assert value.count("1003") == 1


async def test_the_three_weekly_cards_are_told_apart(chat_bot, chat_seeded):
    from .test_chat_weekly import spoken

    await ask(chat_bot, query="hstar", time="23:30")
    await tools.dispatch(context(chat_bot), "propose_remove_fixed", {"query": "xkalos"})
    await tools.dispatch(
        context(chat_bot),
        "propose_add",
        {"boss": "HBellona", "when": spoken(), "weekly": True},
    )
    names = {
        row["bosses"][0]: formatting.proposal_line(row, None, TZ)[0] for row in proposals(chat_bot)
    }

    assert names["HStar"].startswith("change weekly")
    assert names["XKalos"].startswith("remove weekly")
    assert names["HBellona"].startswith("new weekly")


# ---------------------------------------------------------------------------
# approving it -- in place, which is the whole point
# ---------------------------------------------------------------------------


async def test_a_confirmed_card_changes_the_same_baseline(chat_bot, chat_seeded):
    fixed = star_fixed(chat_bot)
    before = len(chat_bot.repo.list_fixed_runs())
    await ask(chat_bot, query="hstar", time="23:30")
    row = proposals(chat_bot)[0]
    assert may_commit(row, None, 1002, has_role=True) is True

    result = approve(chat_bot, row)

    assert result.applied is True
    assert result.fixed_run_id == fixed["id"]
    assert len(chat_bot.repo.list_fixed_runs()) == before  # no duplicate
    updated = chat_bot.repo.get_fixed_run(fixed["id"])
    assert updated["time"] == "23:30"
    assert updated["weekday"] == 0
    assert updated["participants"] == ["1001", "1002"]
    assert updated["bosses"] == ["HStar", "HFA"]


async def test_this_weeks_run_moves_and_keeps_its_id(chat_bot, chat_seeded):
    """What `/fixed edit` and the portal do: the run is re-timed, not re-made."""
    await ask(chat_bot, query="hstar", time="23:30")
    result = approve(chat_bot, proposals(chat_bot)[0])

    run = chat_bot.repo.get_run(chat_seeded["star"])
    assert run["datetime"] == slot_in_week(chat_seeded["week_start"], TZ, 0, clock_time(23, 30))
    assert run["fixed_run_id"] == result.fixed_run_id
    fixed_runs = [
        r
        for r in chat_bot.repo.list_runs(week_start=chat_seeded["week_start"])
        if r["fixed_run_id"] == result.fixed_run_id
    ]
    assert [r["id"] for r in fixed_runs] == [chat_seeded["star"]]
    assert chat_bot.repo.list_reminders(run["id"])
    assert "updated 1 scheduled run(s)" in result.notes


async def test_a_party_change_reaches_this_weeks_run(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", participants="kanon, Priya")
    approve(chat_bot, proposals(chat_bot)[0])

    assert star_fixed(chat_bot)["participants"] == ["1002", "1003"]
    assert chat_bot.repo.get_run(chat_seeded["star"])["participants"] == ["1002", "1003"]


async def test_a_run_that_already_happened_is_left_alone(chat_bot, chat_seeded):
    """Rewriting a night that has been and gone would falsify the record of it."""
    chat_bot.repo.set_run_status(chat_seeded["star"], "done")
    before = chat_bot.repo.get_run(chat_seeded["star"])["datetime"]

    await ask(chat_bot, query="hstar", time="23:30")
    approve(chat_bot, proposals(chat_bot)[0])

    assert chat_bot.repo.get_run(chat_seeded["star"])["datetime"] == before
    assert star_fixed(chat_bot)["time"] == "23:30"


async def test_next_week_materialises_at_the_new_time(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", day="wednesday", time="23:30")
    approve(chat_bot, proposals(chat_bot)[0])

    ws = next_week_start(TZ, RESET_WEEKDAY, RESET_TIME)
    materialise_week(chat_bot.repo, ws, TZ, PING_TIME, COUNTDOWNS, now=ws)
    runs = [r for r in chat_bot.repo.list_runs(week_start=ws) if "HStar" in r["bosses"]]

    assert len(runs) == 1
    assert runs[0]["datetime"].astimezone(TZ).strftime("%a %H:%M") == "Wed 23:30"


async def test_a_baseline_that_has_gone_fails_the_second_time(chat_bot, chat_seeded):
    await ask(chat_bot, query="hstar", time="23:30")
    row = proposals(chat_bot)[0]
    chat_bot.repo.delete_fixed_run(row["payload"]["fixed_run_id"])

    result = approve(chat_bot, row)
    assert result.applied is False
    assert "has gone" in result.problem


async def test_the_card_path_lands_where_the_portal_lands(chat_bot, chat_seeded):
    """Two identical weeklies, changed the two ways: the same state, or a bug.

    `/fixed edit`, the portal's ``PATCH`` and this card cannot be made to share
    one function without reaching into :mod:`bot.api.service`, so what is asserted
    is the thing that actually matters: they leave the run and the baseline in the
    same state, arrow for arrow.
    """
    from bot.api import service

    twin_id = chat_bot.repo.add_fixed_run(
        1001, ["HBellona"], 0, "21:30", ["1001", "1002"], channel_id=WATCHED_CHANNEL
    )
    materialise_week(
        chat_bot.repo,
        chat_seeded["week_start"],
        TZ,
        PING_TIME,
        COUNTDOWNS,
        now=chat_seeded["week_start"],
    )
    twin_run = chat_bot.repo.run_for_fixed(twin_id, chat_seeded["week_start"])

    await ask(chat_bot, query="hstar", day="wednesday", time="23:30", participants="kanon, Priya")
    commit(
        chat_bot.repo,
        proposals(chat_bot)[0],
        tz=TZ,
        reset_weekday=RESET_WEEKDAY,
        reset_time=RESET_TIME,
        ping_time=PING_TIME,
        countdowns=COUNTDOWNS,
        actor_id=1002,
    )
    await service.update_fixed(
        chat_bot, twin_id, day="wednesday", time="23:30", participants=["1002", "1003"]
    )

    def state(fixed_id, run_id):
        fixed = chat_bot.repo.get_fixed_run(fixed_id)
        run = chat_bot.repo.get_run(run_id)
        return (
            fixed["weekday"],
            fixed["time"],
            fixed["participants"],
            fixed["owner_id"],
            run["datetime"],
            run["participants"],
            run["status"],
            sorted(r["kind"] for r in chat_bot.repo.list_reminders(run["id"])),
        )

    assert state(star_fixed(chat_bot)["id"], chat_seeded["star"]) == state(twin_id, twin_run["id"])


# ---------------------------------------------------------------------------
# injection posture
# ---------------------------------------------------------------------------


async def test_the_model_cannot_name_a_different_baseline_in_the_payload(chat_bot, chat_seeded):
    """The id on the card comes from resolution here, not from the tool call."""
    kalos = next(f for f in chat_bot.repo.list_fixed_runs() if "XKalos" in f["bosses"])
    await ask(
        chat_bot,
        query="hstar",
        time="23:30",
        fixed_run_id=kalos["id"],
        payload={"op": "remove", "fixed_run_id": kalos["id"]},
        op="remove",
    )
    row = proposals(chat_bot)[0]

    assert row["payload"]["op"] == FIX_EDIT
    assert row["payload"]["fixed_run_id"] == star_fixed(chat_bot)["id"]

    approve(chat_bot, row)
    # It changed one baseline and retired nobody else's.
    assert chat_bot.repo.get_fixed_run(kalos["id"]) is not None


async def test_extra_arguments_do_not_change_who_or_where(chat_bot, chat_seeded):
    await ask(
        chat_bot,
        query="hstar",
        time="23:30",
        channel_id="999",
        author_id="1001",
        owner_id="1001",
        status="confirmed",
    )
    row = proposals(chat_bot)[0]

    assert row["channel_id"] == str(CHAT_CHANNEL)
    assert row["status"] == "proposed"
    assert row["participants"] == ["1001", "1002"]
    approve(chat_bot, row)
    assert str(star_fixed(chat_bot)["channel_id"]) == str(WATCHED_CHANNEL)
    assert str(star_fixed(chat_bot)["owner_id"]) == "1001"


async def test_update_fixed_is_not_reachable_as_a_tool(chat_bot, chat_seeded):
    fixed = star_fixed(chat_bot)
    for name in ("update_fixed", "edit_fixed", "fixed_edit", "patch_fixed"):
        answer = await tools.dispatch(context(chat_bot), name, {"fixed_id": fixed["id"]})
        assert "There is no tool called" in answer
        assert "propose_change_fixed" in answer
    assert chat_bot.repo.get_fixed_run(fixed["id"])["time"] == "21:30"


# ---------------------------------------------------------------------------
# steering -- the reason the model reached for the wrong tool
# ---------------------------------------------------------------------------


def test_the_three_timing_tools_point_at_each_other():
    schemas = {t["function"]["name"]: t["function"]["description"] for t in tools.TOOLS}

    assert "propose_change_fixed" in schemas["propose_move"]
    assert "ONE dated run" in schemas["propose_move"]
    assert "propose_change_fixed" in schemas["propose_add"]
    assert "Never use this to change a weekly that already exists" in schemas["propose_add"]
    assert "duplicate" in schemas["propose_change_fixed"]
    assert "propose_move" in schemas["propose_change_fixed"]
    assert "never reach for propose_add" in schemas["propose_change_fixed"]
    assert "EXISTING" in schemas["propose_change_fixed"]


def test_the_schema_tells_it_to_ask_which_timing_when_there_are_several():
    change = next(
        t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_change_fixed"
    )
    query = change["parameters"]["properties"]["query"]["description"]

    assert "several weekly timings" in query
    assert "current day" in query
    assert "never pick one yourself" in query
    assert change["parameters"]["required"] == ["query"]


def test_every_optional_argument_is_optional():
    change = next(
        t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_change_fixed"
    )
    for field in ("day", "time", "participants"):
        assert field not in change["parameters"]["required"]
        assert "Optional" in change["parameters"]["properties"][field]["description"]


def test_the_party_argument_says_it_is_the_whole_party():
    """A request to add one person must not reach the tool as a party of one."""
    change = next(
        t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_change_fixed"
    )
    people = change["parameters"]["properties"]["participants"]["description"]

    assert "WHOLE party" in people
    assert "not only the people joining or leaving" in people


def test_the_party_argument_says_the_asker_counts_as_one():
    """Live: "add me to the weekly" reached the tool as a party without them in it.

    The prompt labels every turn with who said it (`ChatPilot._speaker`), so the
    model has always known who is asking; nothing told it that person belongs in
    the field.
    """
    change = next(
        t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_change_fixed"
    )
    people = change["parameters"]["properties"]["participants"]["description"]

    assert "the person asking" in people
    assert "add me to the weekly" in people
    assert "labelled with who said it" in people


def test_propose_add_says_the_asker_counts_as_a_participant():
    """Live: "schedule a run for me @X and @Y" carded X and Y and dropped the asker.

    The follow-up "add @asker" then raised a second card for them alone -- two
    cards for one run, which is the same duplicate `propose_change_fixed` exists
    to stop, arrived at from the other end.
    """
    add = next(t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_add")
    people = add["parameters"]["properties"]["participants"]["description"]

    assert "for me" in people and "count me in" in people
    assert "never leave out the person asking for it" in people
    assert "labelled with who said it" in people


def test_propose_add_reads_this_is_fixed_as_a_weekly():
    """Live: "...for tonight 1900, this is fixed" got a one-off card.

    `_TRUTHY` has accepted the word all along -- the gap was that nothing said
    the phrase belongs in this argument at all, so the model never set it.
    """
    add = next(t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_add")
    weekly = add["parameters"]["properties"]["weekly"]["description"]

    assert "this is fixed" in weekly
    assert "make it weekly" in weekly
    assert "a separate sentence" in weekly.lower()
    # Still the safe default: none of the above may soften the one-time rule.
    assert "One-time is the safe default" in weekly


def test_propose_add_says_a_run_that_exists_can_be_made_weekly_here():
    """The other half of the matcher fix: where the model should have gone.

    Told to make an existing Hard Jupiter run repeat, the model reached for
    propose_change_fixed -- reasonably, since the sentence is about a run that
    exists -- and got a list of three unrelated Tuesday weeklies back.
    """
    add = next(t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_add")
    weekly = add["parameters"]["properties"]["weekly"]["description"]

    assert "ALREADY" in weekly
    assert "make it run every week" in weekly
    assert "not propose_change_fixed" in weekly
    assert "folds this week's run into the new weekly" in weekly
    assert "One-time is the safe default" in weekly
