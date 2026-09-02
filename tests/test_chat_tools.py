"""The tool surface: what it answers, what it refuses, and what a write actually does.

The write tools are the whole security story of the feature, so the tests that
matter most are the ones asserting a *card* rather than a change: after
``propose_move`` the run is still where it was, and it only moves when the
existing ✅ path is run over the row the tool created.
"""

from __future__ import annotations

from datetime import timedelta

import pytest

from bot.chat import tools
from bot.extract.commit import commit, may_commit
from bot.ids import short_id

from .chat_support import CHAT_CHANNEL
from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ

pytestmark = pytest.mark.anyio


@pytest.fixture
def anyio_backend():
    return "asyncio"


def context(bot, author_id: int | str = 1002, message_id: int = 950000000000000123):
    return tools.ToolContext(
        bot=bot,
        author_id=str(author_id),
        channel_id=str(CHAT_CHANNEL),
        message_id=str(message_id),
    )


def cards(bot):
    return [post for post in bot.posts if post.kind == "card"]


def proposals(bot):
    return bot.repo.list_amendments(status="proposed")


def line_for(answer: str, run_id: str) -> str:
    """The one line of a listing that is about ``run_id``."""
    return next(line for line in answer.splitlines() if short_id(run_id) in line)


# ---------------------------------------------------------------------------
# the catalogue
# ---------------------------------------------------------------------------


def test_the_tool_list_is_exactly_the_documented_ten():
    assert tools.tool_names() == [
        "get_schedule",
        "get_run",
        "list_bosses",
        "get_pending",
        "propose_move",
        "propose_add",
        "propose_cancel",
        "propose_remove_fixed",
        "propose_change_fixed",
        "propose_rsvp",
    ]


def test_every_tool_declares_a_usable_schema():
    for tool in tools.TOOLS:
        function = tool["function"]
        parameters = function["parameters"]
        assert tool["type"] == "function"
        assert function["description"]
        assert parameters["type"] == "object"
        # Everything required must actually be declared, or the grammar the
        # model is handed asks for a field it cannot fill.
        assert set(parameters["required"]) <= set(parameters["properties"])


def test_no_tool_can_approve_reject_or_configure_anything():
    """The chatbot drafts; it never ratifies, and it never edits settings."""
    forbidden = ("approve", "reject", "config", "delete", "swap", "say", "rescan")
    assert not [n for n in tools.tool_names() if any(word in n for word in forbidden)]


# ---------------------------------------------------------------------------
# reads
# ---------------------------------------------------------------------------


async def test_get_schedule_lists_the_week(chat_bot, chat_seeded):
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "this"})
    assert "Hard Star + Hard FA" in answer
    assert "Extreme Kalos" in answer
    assert short_id(chat_seeded["star"]) in answer
    star = chat_bot.repo.get_run(chat_seeded["star"])
    line = line_for(answer, chat_seeded["star"])
    assert f"`[{short_id(chat_seeded['star'])}]`" in line
    assert f"*{star['datetime'].astimezone(chat_bot.tz):%a %d %b %H:%M}*" in line
    assert "**Hard Star + Hard FA**" in line
    assert "`planned`" in line and "`0/2 yes`" in line


async def test_get_schedule_says_so_when_a_week_is_empty(chat_bot, chat_seeded):
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "next"})
    assert "Nothing is scheduled" in answer


async def test_get_schedule_rejects_a_week_it_does_not_have(chat_bot, chat_seeded):
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "last"})
    assert "this" in answer and "next" in answer


async def test_get_schedule_marks_a_run_that_has_already_happened(
    chat_bot, chat_seeded, monkeypatch
):
    monkeypatch.setattr(tools, "utcnow", lambda: chat_seeded["week_start"] + timedelta(days=7))
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "this"})
    assert "already happened" in line_for(answer, chat_seeded["star"])


async def test_get_schedule_leaves_an_upcoming_run_unmarked(chat_bot, chat_seeded, monkeypatch):
    monkeypatch.setattr(tools, "utcnow", lambda: chat_seeded["week_start"])
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "this"})
    assert "already happened" not in line_for(answer, chat_seeded["star"])


async def test_get_schedule_marks_a_done_run_whose_time_has_not_come(
    chat_bot, chat_seeded, monkeypatch
):
    """`done` is over whatever the clock says -- a run can be finished early."""
    monkeypatch.setattr(tools, "utcnow", lambda: chat_seeded["week_start"])
    chat_bot.repo.set_run_status(chat_seeded["kalos"], "done")
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "this"})
    assert "already happened" in line_for(answer, chat_seeded["kalos"])
    assert "already happened" not in line_for(answer, chat_seeded["star"])


async def test_get_schedule_says_when_nothing_upcoming_is_left(chat_bot, chat_seeded, monkeypatch):
    monkeypatch.setattr(tools, "utcnow", lambda: chat_seeded["week_start"] + timedelta(days=7))
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "this"})
    assert "nothing upcoming is left" in answer


async def test_get_schedule_stays_quiet_while_one_run_is_still_to_come(
    chat_bot, chat_seeded, monkeypatch
):
    """Monday 22:00: the Monday run is over, the Tuesday one is not."""
    monkeypatch.setattr(
        tools, "utcnow", lambda: chat_seeded["week_start"] + timedelta(days=4, hours=22)
    )
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "this"})
    assert "already happened" in line_for(answer, chat_seeded["star"])
    assert "already happened" not in line_for(answer, chat_seeded["kalos"])
    assert "nothing upcoming" not in answer


# participant="me" -- the bug: "what's for me" returned the entire schedule


async def test_get_schedule_for_me_returns_only_the_askers_runs(chat_bot, chat_seeded):
    """User 1002 is on both runs; participant='me' must return both and only those."""
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002), "get_schedule", {"week": "this", "participant": "me"}
    )
    assert short_id(chat_seeded["star"]) in answer
    assert short_id(chat_seeded["kalos"]) in answer
    assert "Your runs" in answer


async def test_get_schedule_for_me_excludes_runs_the_asker_is_not_on(chat_bot, chat_seeded):
    """User 1001 is only on Star; participant='me' must not return Kalos."""
    answer = await tools.dispatch(
        context(chat_bot, author_id=1001), "get_schedule", {"week": "this", "participant": "me"}
    )
    assert short_id(chat_seeded["star"]) in answer
    assert short_id(chat_seeded["kalos"]) not in answer


async def test_get_schedule_for_me_says_so_when_not_on_any_run(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot, author_id=9999), "get_schedule", {"week": "this", "participant": "me"}
    )
    assert "not on any runs" in answer
    assert short_id(chat_seeded["star"]) not in answer


async def test_get_schedule_accepts_askers_name_when_model_ignores_enum(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot, author_id=1001),
        "get_schedule",
        {"week": "this", "participant": "Alvin"},
    )
    assert short_id(chat_seeded["star"]) in answer
    assert short_id(chat_seeded["kalos"]) not in answer
    assert "Your runs" in answer


async def test_get_schedule_can_filter_to_a_named_member(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot, author_id=1001),
        "get_schedule",
        {"week": "this", "participant": "Priya"},
    )
    assert short_id(chat_seeded["star"]) not in answer
    assert short_id(chat_seeded["kalos"]) in answer
    assert "Priya's runs" in answer


async def test_get_schedule_refuses_an_unknown_participant_instead_of_listing_all(
    chat_bot, chat_seeded
):
    outcome = await tools.run(
        context(chat_bot),
        "get_schedule",
        {"week": "this", "participant": "nobody-here"},
    )
    assert not outcome.ok
    assert outcome.error == tools.REFUSED
    assert "Nobody on the roster matches" in outcome.output
    assert short_id(chat_seeded["star"]) not in outcome.output
    assert short_id(chat_seeded["kalos"]) not in outcome.output


@pytest.mark.parametrize("participant", [None, "", "   ", False, 0, [], {}])
async def test_get_schedule_refuses_an_invalid_supplied_participant(
    chat_bot, chat_seeded, participant
):
    outcome = await tools.run(
        context(chat_bot),
        "get_schedule",
        {"week": "this", "participant": participant},
    )
    assert not outcome.ok
    assert outcome.error == tools.REFUSED
    assert "participant must be 'me' or one roster name" in outcome.output
    assert short_id(chat_seeded["star"]) not in outcome.output
    assert short_id(chat_seeded["kalos"]) not in outcome.output


async def test_get_schedule_refuses_an_ambiguous_participant(chat_bot, chat_seeded):
    chat_bot.repo.upsert_member(1004, "kanonn", "kanonn", True)
    answer = await tools.dispatch(
        context(chat_bot), "get_schedule", {"week": "this", "participant": "kano"}
    )
    assert "Ask them which participant they mean" in answer
    assert "kanon" in answer and "kanonn" in answer
    assert short_id(chat_seeded["star"]) not in answer
    assert short_id(chat_seeded["kalos"]) not in answer


async def test_get_run_by_short_id(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot), "get_run", {"query": short_id(chat_seeded["star"])}
    )
    assert "Hard Star + Hard FA" in answer
    assert "kanon" in answer
    assert f"`[{short_id(chat_seeded['star'])}]`" in answer
    assert "**Hard Star + Hard FA**" in answer
    assert "`planned`" in answer


async def test_get_run_by_boss_name(chat_bot, chat_seeded):
    answer = await tools.dispatch(context(chat_bot), "get_run", {"query": "kalos"})
    assert "Extreme Kalos" in answer


async def test_get_run_refuses_a_boss_nobody_runs(chat_bot, chat_seeded):
    answer = await tools.dispatch(context(chat_bot), "get_run", {"query": "zakum"})
    assert "No run matches" in answer


async def test_get_run_refuses_an_unknown_id(chat_bot, chat_seeded):
    answer = await tools.dispatch(context(chat_bot), "get_run", {"query": "deadbeef"})
    assert "No run matches" in answer


async def test_list_bosses_names_the_table(chat_bot):
    """Both vocabularies: the token to pass back, and the words to say."""
    answer = await tools.dispatch(context(chat_bot), "list_bosses", {})
    assert "Kalos" in answer
    assert "`XKalos`" in answer
    assert "Extreme Kalos" in answer
    assert "**Bosses this guild runs**\n\n" in answer


async def test_get_pending_is_empty_until_something_is_proposed(chat_bot, chat_seeded):
    assert "no proposal cards" in await tools.dispatch(context(chat_bot), "get_pending", {})


async def test_get_pending_lists_a_card_the_chatbot_raised(chat_bot, chat_seeded):
    await tools.dispatch(
        context(chat_bot), "propose_cancel", {"run_query": short_id(chat_seeded["star"])}
    )
    answer = await tools.dispatch(context(chat_bot), "get_pending", {})
    assert "Hard Star + Hard FA" in answer


# ---------------------------------------------------------------------------
# resolving a run
# ---------------------------------------------------------------------------


def test_resolve_run_narrows_by_day(chat_bot, chat_seeded):
    star = chat_bot.repo.get_run(chat_seeded["star"])
    day = star["datetime"].astimezone(TZ).strftime("%A").lower()
    assert tools.resolve_run(chat_bot, f"hstar {day}")["id"] == chat_seeded["star"]


def second_kalos(chat_bot, chat_seeded, days: int = 3) -> str:
    """Another XKalos night, so 'kalos' alone stops picking one out."""
    from datetime import timedelta

    kalos = chat_bot.repo.get_run(chat_seeded["kalos"])
    return chat_bot.repo.create_run(
        week_start=kalos["week_start"],
        bosses=["XKalos"],
        run_at=kalos["datetime"] + timedelta(days=days),
        participants=["1002", "1003"],
        status="planned",
        source="amend",
        channel_id=kalos["channel_id"],
    )


def test_resolve_run_refuses_a_query_matching_two_nights(chat_bot, chat_seeded):
    second_kalos(chat_bot, chat_seeded)
    with pytest.raises(tools.ToolError) as exc:
        tools.resolve_run(chat_bot, "kalos")
    assert "more than one run" in str(exc.value)


def test_a_query_that_locates_nothing_is_refused_outright(chat_bot, chat_seeded):
    """Never resolve gibberish to "the only run" -- that is how the wrong night gets cancelled."""
    for run in chat_bot.repo.list_runs():
        if run["id"] != chat_seeded["star"]:
            chat_bot.repo.set_run_status(run["id"], "cancelled")
    with pytest.raises(tools.ToolError) as exc:
        tools.resolve_run(chat_bot, "the run lah")
    assert "No run matches" in str(exc.value)


def test_resolve_run_refuses_a_real_boss_on_the_wrong_day(chat_bot, chat_seeded):
    """Answering about a different night would be worse than saying no."""
    star = chat_bot.repo.get_run(chat_seeded["star"])
    wrong = "Sun" if star["datetime"].astimezone(TZ).strftime("%a") != "Sun" else "Sat"
    with pytest.raises(tools.ToolError) as exc:
        tools.resolve_run(chat_bot, f"hstar {wrong.lower()}day")
    assert "That boss is on" in str(exc.value)


def test_resolve_run_ignores_cancelled_runs(chat_bot, chat_seeded):
    """With one of the two Kalos nights off, 'kalos' is unambiguous again."""
    second = second_kalos(chat_bot, chat_seeded)
    chat_bot.repo.set_run_status(second, "cancelled")
    assert tools.resolve_run(chat_bot, "kalos")["id"] == chat_seeded["kalos"]


def test_resolve_run_needs_something_to_go_on(chat_bot, chat_seeded):
    with pytest.raises(tools.ToolError):
        tools.resolve_run(chat_bot, "   ")


@pytest.mark.parametrize(
    "query",
    [
        "152fa345",  # a hex id happens to contain the short name "FA"
        "deadbeef",
        "can we go faster",
        "the sofa run",
        "bmw",
    ],
)
def test_a_boss_short_name_is_matched_as_a_word_not_a_substring(chat_bot, chat_seeded, query):
    """`FA` and `BM` are two letters long and turn up inside ordinary text and ids.

    Matching them as substrings resolved a mistyped id to somebody's HFA night,
    which `propose_cancel` would then have offered to cancel.
    """
    with pytest.raises(tools.ToolError) as exc:
        tools.resolve_run(chat_bot, query)
    assert "No run matches" in str(exc.value)


def test_a_boss_short_name_still_matches_when_it_is_written(chat_bot, chat_seeded):
    assert tools.resolve_run(chat_bot, "fa")["id"] == chat_seeded["star"]
    assert tools.resolve_run(chat_bot, "hfa please")["id"] == chat_seeded["star"]


@pytest.mark.parametrize("spelling", ["monday", "mon", "Mon."])
def test_the_guilds_own_weekday_spellings_are_understood(chat_bot, chat_seeded, spelling):
    """The same alias table the extractor reads `weds`/`thurs` with."""
    star = chat_bot.repo.get_run(chat_seeded["star"])
    assert star["datetime"].astimezone(TZ).weekday() == 0
    assert tools.resolve_run(chat_bot, f"hstar {spelling}")["id"] == chat_seeded["star"]


def test_resolution_does_not_depend_on_the_ids_the_database_happened_to_generate(
    chat_bot, chat_seeded
):
    """Regression guard: the flake that exposed the substring bug was id-dependent."""
    for run in chat_bot.repo.list_runs():
        with pytest.raises(tools.ToolError):
            tools.resolve_run(chat_bot, short_id(run["id"]).replace("a", "z") + "zz")


# ---------------------------------------------------------------------------
# the card bridge: a write posts a card and changes nothing
# ---------------------------------------------------------------------------


async def test_propose_move_creates_a_card_and_moves_nothing(chat_bot, chat_seeded):
    before = chat_bot.repo.get_run(chat_seeded["star"])["datetime"]
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_move",
        {"run_query": short_id(chat_seeded["star"]), "to_when": "sunday 22:00"},
    )

    assert "✅" in answer
    assert chat_bot.repo.get_run(chat_seeded["star"])["datetime"] == before

    open_rows = proposals(chat_bot)
    assert len(open_rows) == 1
    row = open_rows[0]
    assert row["kind"] == "move"
    assert row["run_id"] == chat_seeded["star"]
    assert row["new_datetime"].astimezone(TZ).strftime("%H:%M") == "22:00"
    assert row["channel_id"] == str(CHAT_CHANNEL)
    # The evidence is the message that asked for it, so the card links back to a
    # real line in the channel.
    assert row["evidence_msg_ids"] == ["950000000000000123"]
    assert cards(chat_bot)


async def test_a_card_the_chatbot_raised_is_audited_against_the_asker(chat_bot, chat_seeded):
    """A rescan's card is the extractor's; one raised here belongs to whoever asked.

    Same surface either way -- a proposal always comes from something somebody
    said in a channel -- and the name is the message's author id, never anything
    the model wrote.
    """
    await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_cancel",
        {"run_query": short_id(chat_seeded["star"])},
    )
    raised = [row for row in chat_bot.repo.list_audit() if row["action"] == "propose"]
    assert [(row["surface"], row["actor"]) for row in raised] == [("chat", "1002")]
    assert raised[0]["subject"] == proposals(chat_bot)[0]["id"]


async def test_the_card_is_the_extractors_card_and_the_normal_tick_approves_it(
    chat_bot, chat_seeded
):
    """The whole point of the bridge: a ✅ from a participant applies it.

    Exercised through :func:`bot.extract.commit.may_commit` and
    :func:`~bot.extract.commit.commit` -- the two functions
    :meth:`bot.client.BossBot._handle_proposal_reaction` calls -- so this is the
    real reaction path over a row the chatbot wrote.
    """
    await tools.dispatch(
        context(chat_bot),
        "propose_move",
        {"run_query": short_id(chat_seeded["star"]), "to_when": "sunday 22:00"},
    )
    row = proposals(chat_bot)[0]
    run = chat_bot.repo.get_run(row["run_id"])

    # A participant may confirm it; somebody who is not on the run may not.
    assert may_commit(row, run, 1002, has_role=True) is True
    assert may_commit(row, run, 1003, has_role=True) is False

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
    moved = chat_bot.repo.get_run(chat_seeded["star"])
    assert moved["datetime"].astimezone(TZ).strftime("%H:%M") == "22:00"
    assert chat_bot.repo.get_amendment(row["id"])["status"] == "confirmed"


async def test_propose_move_rejects_a_time_it_cannot_read(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_move",
        {"run_query": short_id(chat_seeded["star"]), "to_when": "whenever lah"},
    )
    assert "couldn't read" in answer
    assert proposals(chat_bot) == []


async def test_propose_move_refuses_the_past(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_move",
        {"run_query": short_id(chat_seeded["star"]), "to_when": "2020-01-01 21:30"},
    )
    assert "in the past" in answer
    assert proposals(chat_bot) == []


async def test_propose_move_needs_a_time(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot), "propose_move", {"run_query": short_id(chat_seeded["star"])}
    )
    assert "day and time" in answer
    assert proposals(chat_bot) == []


async def test_propose_cancel_creates_a_card_and_cancels_nothing(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot), "propose_cancel", {"run_query": short_id(chat_seeded["kalos"])}
    )
    assert "✅" in answer
    assert chat_bot.repo.get_run(chat_seeded["kalos"])["status"] != "cancelled"
    row = proposals(chat_bot)[0]
    assert (row["kind"], row["run_id"]) == ("cancel", chat_seeded["kalos"])


async def test_propose_cancel_declines_a_run_that_is_already_off(chat_bot, chat_seeded):
    chat_bot.repo.set_run_status(chat_seeded["kalos"], "cancelled")
    answer = await tools.dispatch(
        context(chat_bot), "propose_cancel", {"run_query": chat_seeded["kalos"]}
    )
    assert "already cancelled" in answer
    assert proposals(chat_bot) == []


# ---------------------------------------------------------------------------
# rsvp: for the asker, and only ever the asker
# ---------------------------------------------------------------------------


async def test_propose_rsvp_records_the_asker(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_rsvp",
        {"run_query": short_id(chat_seeded["star"]), "answer": "no"},
    )
    assert "✅" in answer
    row = proposals(chat_bot)[0]
    assert (row["kind"], row["rsvp"], row["participants"]) == ("rsvp", "no", ["1002"])
    # Nothing is recorded until somebody confirms the card.
    assert chat_bot.repo.get_rsvps(chat_seeded["star"]) == {}


async def test_an_rsvp_card_applies_on_confirmation(chat_bot, chat_seeded):
    await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_rsvp",
        {"run_query": short_id(chat_seeded["star"]), "answer": "no"},
    )
    row = proposals(chat_bot)[0]
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
    assert chat_bot.repo.get_rsvps(chat_seeded["star"])["1002"] == "no"
    assert chat_bot.repo.get_run(chat_seeded["star"])["status"] == "at_risk"


async def test_propose_rsvp_refuses_somebody_not_on_the_run(chat_bot, chat_seeded):
    """1003 is on Kalos, not on Star."""
    answer = await tools.dispatch(
        context(chat_bot, author_id=1003),
        "propose_rsvp",
        {"run_query": short_id(chat_seeded["star"]), "answer": "yes"},
    )
    assert "not on run" in answer
    assert proposals(chat_bot) == []


async def test_propose_rsvp_only_accepts_yes_or_no(chat_bot, chat_seeded):
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_rsvp",
        {"run_query": short_id(chat_seeded["star"]), "answer": "maybe later"},
    )
    assert "must be" in answer
    assert proposals(chat_bot) == []


async def test_an_rsvp_card_for_somebody_taken_off_the_run_will_not_apply(chat_bot, chat_seeded):
    """A `/swap` between the card and the ✅ must not write a stranger's answer."""
    await tools.dispatch(
        context(chat_bot, author_id=1002),
        "propose_rsvp",
        {"run_query": short_id(chat_seeded["star"]), "answer": "yes"},
    )
    row = proposals(chat_bot)[0]
    chat_bot.repo.set_run_participants(chat_seeded["star"], ["1001"])
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
    assert result.applied is False
    assert "no longer on the run" in result.problem


# ---------------------------------------------------------------------------
# the dispatcher itself
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "name", ["approve", "set_config", "delete_run", "eval", "", "get_schedule "]
)
async def test_an_unknown_tool_is_refused_by_name(chat_bot, chat_seeded, name):
    answer = await tools.dispatch(context(chat_bot), name, {})
    assert "There is no tool called" in answer
    assert proposals(chat_bot) == []


async def test_arguments_may_arrive_as_a_json_string(chat_bot, chat_seeded):
    answer = await tools.dispatch(context(chat_bot), "get_schedule", '{"week": "this"}')
    assert "Hard Star + Hard FA" in answer


async def test_unreadable_arguments_do_not_raise(chat_bot, chat_seeded):
    for arguments in ("not json", None, 42, ["week"]):
        assert await tools.dispatch(chat_bot and context(chat_bot), "get_schedule", arguments)


async def test_a_failing_tool_comes_back_as_text(chat_bot, chat_seeded, monkeypatch):
    def boom(*_args, **_kwargs):
        raise RuntimeError("the database went away")

    monkeypatch.setattr(chat_bot.repo, "list_runs", boom)
    answer = await tools.dispatch(context(chat_bot), "get_schedule", {"week": "this"})
    assert "could not reach the schedule" in answer


async def test_a_write_that_cannot_post_its_card_says_so(chat_bot, chat_seeded):
    """No card in the channel means nobody can confirm it, and the model must not claim one."""
    chat_bot.channels[CHAT_CHANNEL].permissions.send_messages = False
    chat_bot.settings.post_channel_id = None
    answer = await tools.dispatch(
        context(chat_bot),
        "propose_cancel",
        {"run_query": short_id(chat_seeded["star"])},
    )
    assert "could not be posted" in answer
