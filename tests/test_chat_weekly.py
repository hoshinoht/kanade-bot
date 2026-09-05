"""`propose_add` with ``weekly``: a recurring baseline instead of one night.

The dangerous confusion here is the mirror of the one in
`tests/test_chat_remove_fixed.py`. There, cancelling one night was mistaken for
retiring a baseline; here, "schedule a run tonight" must never be read as "and
every Tuesday from now on". One-time is the default and stays the default: only
the word `weekly` (which the schema only lets the model reach for when a member
actually said the run repeats) flips it, and the card says which one it is.

When it does flip, the card is the extractor's `fix` -- the same kind, handler
and ✅ path `/fixed add` produces -- so an approved weekly leaves the database in
the state the slash command would have left it in.
"""

from __future__ import annotations

from datetime import datetime, timedelta

import pytest

from bot.agent import formatting
from bot.agent.materialise import materialise_week
from bot.chat import tools
from bot.domain.timeutil import utcnow
from bot.domain.weeks import WEEKDAY_NAMES, current_week_start, week_end, week_start
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
        message_id="950000000000000777",
    )


def proposals(bot):
    return bot.repo.list_amendments(status="proposed")


def tomorrow(hhmm: str = "21:30") -> datetime:
    """Tomorrow at ``hhmm``, guild-local -- always ahead of now, never today."""
    hour, minute = (int(part) for part in hhmm.split(":"))
    local = utcnow().astimezone(TZ) + timedelta(days=1)
    return local.replace(hour=hour, minute=minute, second=0, microsecond=0)


def spoken(hhmm: str = "21:30") -> str:
    return tomorrow(hhmm).strftime("%Y-%m-%d %H:%M")


def materialise_both_weeks(bot, now: datetime | None = None) -> None:
    """What ``BossBot.materialise_weeks`` does, without the live client."""
    now = now or utcnow()
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


async def ask_weekly(bot, **overrides):
    args = {"boss": "HBellona", "when": spoken(), "weekly": True}
    args.update(overrides)
    return await tools.dispatch(context(bot), "propose_add", args)


# ---------------------------------------------------------------------------
# the card
# ---------------------------------------------------------------------------


async def test_it_posts_a_fix_card_and_schedules_nothing(chat_bot, chat_seeded):
    before_fixed = len(chat_bot.repo.list_fixed_runs())
    before_runs = len(chat_bot.repo.list_runs())

    answer = await ask_weekly(chat_bot)

    assert "✅" in answer
    assert len(chat_bot.repo.list_fixed_runs()) == before_fixed  # no baseline yet
    assert len(chat_bot.repo.list_runs()) == before_runs

    row = proposals(chat_bot)[0]
    assert row["kind"] == "fix"
    assert row["run_id"] is None
    assert row["bosses"] == ["HBellona"]
    assert row["channel_id"] == str(CHAT_CHANNEL)
    assert row["evidence_msg_ids"] == ["950000000000000777"]
    assert row["proposal_message_id"]


async def test_the_payload_is_the_weekday_and_time_a_baseline_stores(chat_bot, chat_seeded):
    await ask_weekly(chat_bot)
    payload = proposals(chat_bot)[0]["payload"]

    assert payload["weekday"] == tomorrow().weekday()
    assert payload["time"] == "21:30"
    # Not the *other* `fix`: nothing here retires anything.
    assert "op" not in payload
    assert "fixed_run_id" not in payload


async def test_the_asker_is_the_default_party(chat_bot, chat_seeded):
    await ask_weekly(chat_bot)
    assert proposals(chat_bot)[0]["participants"] == ["1002"]


async def test_a_named_party_is_resolved_against_the_roster(chat_bot, chat_seeded):
    await ask_weekly(chat_bot, participants="kanon, Priya")
    assert proposals(chat_bot)[0]["participants"] == ["1002", "1003"]


async def test_the_tool_result_says_weekly_and_names_the_party(chat_bot, chat_seeded):
    """What the model reads back has to be the card, recurrence included."""
    answer = await ask_weekly(chat_bot, participants="kanon, Priya")

    assert "Hard Bellona" in answer
    assert f"every {WEEKDAY_NAMES[tomorrow().weekday()]} 21:30" in answer
    assert "kanon" in answer and "Priya" in answer


async def test_the_one_time_result_names_a_date_instead(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot), "propose_add", {"boss": "HBellona", "when": spoken()}
    )
    assert "every" not in answer
    assert tomorrow().strftime("%H:%M") in answer


# ---------------------------------------------------------------------------
# wording -- three cards that must never be mistaken for each other
# ---------------------------------------------------------------------------


async def test_the_card_says_it_recurs(chat_bot, chat_seeded):
    await ask_weekly(chat_bot)
    name, value = formatting.proposal_line(proposals(chat_bot)[0], None, TZ)

    assert "new weekly" in name
    assert "Hard Bellona" in name
    assert f"every {WEEKDAY_NAMES[tomorrow().weekday()]} 21:30" in value
    assert "recurring from now on" in value
    assert "not a one-off" in value


async def test_a_one_time_card_reads_as_one_night(chat_bot, chat_seeded):
    """The regression that matters most: the default card must not say weekly."""
    await tools.dispatch(context(chat_bot), "propose_add", {"boss": "HBellona", "when": spoken()})
    name, value = formatting.proposal_line(proposals(chat_bot)[0], None, TZ)

    assert "new run" in name and "weekly" not in name
    assert "every week" not in value and "recurring" not in value
    assert tomorrow().strftime("%H:%M") in value


async def test_the_new_and_remove_weekly_cards_are_opposites(chat_bot, chat_seeded):
    await ask_weekly(chat_bot)
    await tools.dispatch(context(chat_bot), "propose_remove_fixed", {"query": "hstar"})
    rows = {row["bosses"][0]: row for row in proposals(chat_bot)}

    new_name, new_value = formatting.proposal_line(rows["HBellona"], None, TZ)
    gone_name, gone_value = formatting.proposal_line(rows["HStar"], None, TZ)

    assert new_name.startswith("new weekly") and gone_name.startswith("remove weekly")
    assert "stop scheduling" not in new_value
    assert "recurring from now on" not in gone_value


# ---------------------------------------------------------------------------
# approving it -- the state `/fixed add` would have left
# ---------------------------------------------------------------------------


async def test_a_confirmed_card_creates_the_baseline(chat_bot, chat_seeded):
    await ask_weekly(chat_bot)
    row = proposals(chat_bot)[0]
    assert may_commit(row, None, 1002, has_role=True) is True

    result = approve(chat_bot, row)

    assert result.applied is True
    fixed = chat_bot.repo.get_fixed_run(result.fixed_run_id)
    assert fixed["bosses"] == ["HBellona"]
    assert fixed["weekday"] == tomorrow().weekday()
    assert fixed["time"] == "21:30"
    assert fixed["participants"] == ["1002"]
    assert str(fixed["channel_id"]) == str(CHAT_CHANNEL)


async def test_it_materialises_this_weeks_run_like_fixed_add(chat_bot, chat_seeded):
    await ask_weekly(chat_bot)
    approve(chat_bot, proposals(chat_bot)[0])

    at = tomorrow()
    ws = week_start(at, TZ, RESET_WEEKDAY, RESET_TIME)
    runs = [r for r in chat_bot.repo.list_runs(week_start=ws) if "HBellona" in r["bosses"]]

    assert len(runs) == 1
    run = runs[0]
    assert run["datetime"] == at
    assert run["source"] == "fixed"
    assert run["fixed_run_id"]
    assert run["participants"] == ["1002"]
    assert str(run["channel_id"]) == str(CHAT_CHANNEL)
    assert chat_bot.repo.list_reminders(run["id"])  # it gets its pings


async def test_later_weeks_keep_materialising_it(chat_bot, chat_seeded):
    """The whole difference from an `add`: it comes back next week by itself."""
    await ask_weekly(chat_bot)
    approve(chat_bot, proposals(chat_bot)[0])

    week = week_start(tomorrow(), TZ, RESET_WEEKDAY, RESET_TIME)
    for _ in range(2):
        week = week_end(week, TZ)
        materialise_week(chat_bot.repo, week, TZ, PING_TIME, COUNTDOWNS, now=week)
        assert any("HBellona" in r["bosses"] for r in chat_bot.repo.list_runs(week_start=week)), (
            f"no run materialised for the week starting {week}"
        )


async def test_a_one_time_add_creates_no_baseline(chat_bot, chat_seeded):
    """Regression lock: the default path still creates a run and nothing more."""
    before = len(chat_bot.repo.list_fixed_runs())
    await tools.dispatch(context(chat_bot), "propose_add", {"boss": "HBellona", "when": spoken()})
    row = proposals(chat_bot)[0]
    assert row["kind"] == "add"

    result = approve(chat_bot, row)

    assert result.applied is True
    assert result.fixed_run_id is None
    assert len(chat_bot.repo.list_fixed_runs()) == before
    created = chat_bot.repo.get_run(result.created_run_ids[0])
    assert created["source"] == "amend"
    assert created["fixed_run_id"] is None


# ---------------------------------------------------------------------------
# "make this run every week" -- the one-off is folded in, not duplicated
# ---------------------------------------------------------------------------


async def one_off(bot, hhmm: str = "22:00", **overrides):
    """Approve a plain `add` card, as a member scheduling one night would."""
    args = {"boss": "HBellona", "when": spoken(hhmm)}
    args.update(overrides)
    await tools.dispatch(context(bot), "propose_add", args)
    return approve(bot, proposals(bot)[0])


def bellona_week(hhmm: str = "23:00"):
    return week_start(tomorrow(hhmm), TZ, RESET_WEEKDAY, RESET_TIME)


def adoption_note(hhmm: str = "23:00") -> str:
    """Which week the fold happened in, which is what the note names.

    Tomorrow is next boss week whenever the reset falls in between, and the suite
    runs on every day of the week.
    """
    this_week = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME)
    return f"adopted {'this' if bellona_week(hhmm) == this_week else 'next'} week's run"


async def test_a_new_weekly_takes_over_the_run_already_on_the_board(chat_bot, chat_seeded):
    """The live ask: a one-off tomorrow, then "make it 23:00 and every week".

    Before, the weekly materialised a second night beside the first: the member's
    answers on one, the pings on the other, and nothing to say they were the same
    run.
    """
    run_id = (await one_off(chat_bot, "22:00")).created_run_ids[0]
    chat_bot.repo.set_rsvp(run_id, "1002", "yes")

    await ask_weekly(chat_bot, when=spoken("23:00"))
    result = approve(chat_bot, proposals(chat_bot)[0])

    runs = [
        r for r in chat_bot.repo.list_runs(week_start=bellona_week()) if "HBellona" in r["bosses"]
    ]
    assert [r["id"] for r in runs] == [run_id]  # the same run, not a second one
    assert runs[0]["datetime"] == tomorrow("23:00")
    assert runs[0]["fixed_run_id"] == result.fixed_run_id
    assert chat_bot.repo.get_rsvps(run_id)["1002"] == "yes"
    assert chat_bot.repo.list_reminders(run_id)


async def test_the_card_says_it_adopted_rather_than_created(chat_bot, chat_seeded):
    await one_off(chat_bot)
    await ask_weekly(chat_bot, when=spoken("23:00"))

    result = approve(chat_bot, proposals(chat_bot)[0])
    assert adoption_note() in result.notes


async def test_a_weekly_with_nothing_to_adopt_says_nothing(chat_bot, chat_seeded):
    await ask_weekly(chat_bot, when=spoken("23:00"))
    result = approve(chat_bot, proposals(chat_bot)[0])

    assert not any("adopted" in note for note in result.notes)
    run = chat_bot.repo.run_for_fixed(result.fixed_run_id, bellona_week())
    assert run["source"] == "fixed"


async def test_the_adopted_run_takes_the_weeklys_party(chat_bot, chat_seeded):
    """A member added by the weekly never agreed to the night they just joined."""
    run_id = (await one_off(chat_bot)).created_run_ids[0]
    chat_bot.repo.set_rsvp(run_id, "1002", "yes")
    chat_bot.repo.set_run_status(run_id, "confirmed")

    await ask_weekly(chat_bot, when=spoken("23:00"), participants="kanon, Priya")
    approve(chat_bot, proposals(chat_bot)[0])

    run = chat_bot.repo.get_run(run_id)
    assert run["participants"] == ["1002", "1003"]
    assert chat_bot.repo.get_rsvps(run_id)["1002"] == "yes"
    assert run["status"] == "planned"  # Priya has not answered


async def test_somebody_elses_night_is_not_adopted(chat_bot, chat_seeded):
    """A one-off for a different boss is nothing to do with this weekly."""
    run_id = (await one_off(chat_bot, boss="HLimbo")).created_run_ids[0]

    await ask_weekly(chat_bot, when=spoken("23:00"))
    result = approve(chat_bot, proposals(chat_bot)[0])

    assert chat_bot.repo.get_run(run_id)["fixed_run_id"] is None
    assert chat_bot.repo.run_for_fixed(result.fixed_run_id, bellona_week())["id"] != run_id
    assert not any("adopted" in note for note in result.notes)


# ---------------------------------------------------------------------------
# which way the flag falls
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("value", [True, "true", "True", "yes", 1])
async def test_the_words_a_model_writes_for_yes_all_mean_weekly(chat_bot, chat_seeded, value):
    await ask_weekly(chat_bot, weekly=value)
    assert proposals(chat_bot)[0]["kind"] == "fix"


@pytest.mark.parametrize("value", [False, "false", "no", 0, None, ""])
async def test_anything_that_is_not_a_yes_stays_one_time(chat_bot, chat_seeded, value):
    """`bool("false")` is True, which would quietly commit a party to every week."""
    await ask_weekly(chat_bot, weekly=value)
    row = proposals(chat_bot)[0]
    assert row["kind"] == "add"
    assert row["new_datetime"].astimezone(TZ) == tomorrow()


async def test_an_omitted_flag_is_a_one_time_run(chat_bot, chat_seeded):
    before = len(chat_bot.repo.list_fixed_runs())
    await tools.dispatch(context(chat_bot), "propose_add", {"boss": "HBellona", "when": spoken()})
    assert proposals(chat_bot)[0]["kind"] == "add"
    assert len(chat_bot.repo.list_fixed_runs()) == before


# ---------------------------------------------------------------------------
# refusals -- identical to the one-time path
# ---------------------------------------------------------------------------


async def test_a_boss_without_a_difficulty_is_refused(chat_bot, chat_seeded):
    answer = await ask_weekly(chat_bot, boss="bellona")
    assert "missing a difficulty prefix" in answer
    assert "do not choose a difficulty" in answer
    assert proposals(chat_bot) == []


async def test_an_unreadable_time_is_refused_with_a_question(chat_bot, chat_seeded):
    answer = await ask_weekly(chat_bot, when="whenever lah")
    assert "couldn't read" in answer
    assert "Ask them for the day and time" in answer
    assert proposals(chat_bot) == []


async def test_a_missing_time_is_refused(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot), "propose_add", {"boss": "HBellona", "weekly": True}
    )
    assert answer == "Ask them what day and time the run should be."
    assert proposals(chat_bot) == []


async def test_a_time_in_the_past_is_refused(chat_bot, chat_seeded):
    answer = await ask_weekly(chat_bot, when="2020-01-01 21:30")
    assert "in the past" in answer
    assert proposals(chat_bot) == []


async def test_a_participant_nobody_matches_is_refused(chat_bot, chat_seeded):
    answer = await ask_weekly(chat_bot, participants="Nobody McGhost")
    assert "Nobody on the roster matches" in answer
    assert proposals(chat_bot) == []


async def test_a_participant_without_the_bossing_role_is_refused(chat_bot, chat_seeded):
    answer = await ask_weekly(chat_bot, participants="NotABosser")
    assert "Nobody on the roster matches" in answer
    assert proposals(chat_bot) == []


# ---------------------------------------------------------------------------
# injection posture
# ---------------------------------------------------------------------------


async def test_a_competing_payload_in_the_arguments_is_ignored(chat_bot, chat_seeded):
    """The card's payload is built from the parsed time, never from the call."""
    kalos = next(f for f in chat_bot.repo.list_fixed_runs() if "XKalos" in f["bosses"])
    await ask_weekly(
        chat_bot,
        payload={"op": "remove", "fixed_run_id": kalos["id"]},
        op="remove",
        fixed_run_id=kalos["id"],
        weekday=6,
        time="03:00",
    )
    row = proposals(chat_bot)[0]

    assert row["payload"]["weekday"] == tomorrow().weekday()
    assert row["payload"]["time"] == "21:30"
    assert "op" not in row["payload"]
    assert "fixed_run_id" not in row["payload"]

    approve(chat_bot, row)
    # The card created a baseline; it retired nobody else's.
    assert chat_bot.repo.get_fixed_run(kalos["id"]) is not None


async def test_extra_arguments_do_not_change_who_or_where(chat_bot, chat_seeded):
    await ask_weekly(
        chat_bot,
        channel_id="999",
        author_id="1001",
        status="confirmed",
        owner_id="1001",
    )
    row = proposals(chat_bot)[0]
    assert row["participants"] == ["1002"]
    assert row["channel_id"] == str(CHAT_CHANNEL)
    assert row["status"] == "proposed"


async def test_create_fixed_is_not_reachable_as_a_tool(chat_bot, chat_seeded):
    before = len(chat_bot.repo.list_fixed_runs())
    for name in ("create_fixed", "add_fixed", "fixed_add", "propose_fixed"):
        answer = await tools.dispatch(
            context(chat_bot),
            name,
            {"bosses": "HBellona", "day": "tue", "time_hhmm": "21:30"},
        )
        assert "There is no tool called" in answer
        assert "propose_add" in answer  # it is told what it may use instead
    assert len(chat_bot.repo.list_fixed_runs()) == before


# ---------------------------------------------------------------------------
# steering
# ---------------------------------------------------------------------------


def test_the_schema_makes_one_time_the_default_and_says_so():
    add = next(t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_add")
    weekly = add["parameters"]["properties"]["weekly"]

    assert "ONE-TIME" in add["description"]
    assert weekly["type"] == "boolean"
    assert "weekly" not in add["parameters"]["required"]
    assert "ONLY for explicit" in weekly["description"]
    assert "every week" in weekly["description"]
    assert "one run that day only" in weekly["description"]


def test_an_unclear_request_is_told_not_to_ask():
    """Asking "weekly or just this once?" every time is worse than defaulting."""
    add = next(t["function"] for t in tools.TOOLS if t["function"]["name"] == "propose_add")
    weekly = add["parameters"]["properties"]["weekly"]["description"]

    assert "Unclear wording" in weekly
    assert "leave it out" in weekly
