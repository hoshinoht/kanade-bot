"""`/fixed edit` semantics: don't clobber amendments, don't co-opt the editor.

These drive the command module's helpers directly with a stand-in bot, so no
Discord connection is involved.
"""

from __future__ import annotations

from datetime import time
from types import SimpleNamespace

import pytest

from bot.commands import _apply_fixed_to_runs, _resolve_participants
from bot.db import Repo
from bot.materialise import materialise_week
from bot.weeks import current_week_start, slot_in_week

from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ


class FakeBot:
    """Just the attributes the command helpers actually reach for."""

    def __init__(self, repo: Repo, role_members: set[str] | None = None):
        self.repo = repo
        self.tz = TZ
        self.ping_time = PING_TIME
        self.countdowns = COUNTDOWNS
        self.settings = SimpleNamespace(reset_weekday=RESET_WEEKDAY, reset_time=RESET_TIME)
        self._roles = role_members if role_members is not None else {"1", "2", "3"}
        self.repo.has_role = lambda uid: str(uid) in self._roles  # type: ignore[method-assign]


@pytest.fixture
def week():
    return current_week_start(TZ, RESET_WEEKDAY, RESET_TIME)


@pytest.fixture
def setup(repo: Repo, week):
    """A Mon 21:30 fixed run, materialised into the current week."""
    fixed_id = repo.add_fixed_run(
        "1",
        ["HStar", "HFA"],
        weekday=0,
        time_hhmm="21:30",
        participants=["1", "2", "3"],
        channel_id=900,
    )
    # As of the reset, not the wall clock: `materialise_week` skips a slot that
    # has already passed, so from Monday 21:30 onwards there was no run to edit.
    materialise_week(repo, week, TZ, PING_TIME, COUNTDOWNS, now=week)
    run = repo.list_runs(week_start=week)[0]
    return FakeBot(repo), fixed_id, run["id"]


def _move_to_wednesday(repo: Repo, run_id: int, week):
    moved = slot_in_week(week, TZ, 2, time(21, 30))  # Wed 21:30
    repo.set_run_datetime(run_id, moved, week)
    repo.set_run_status(run_id, "planned")
    return moved


# -- bug 5: an unrelated edit must not undo this week's /amend ---------------


def test_editing_only_the_note_leaves_an_amended_run_where_it_was(setup, repo, week):
    bot, fixed_id, run_id = setup
    moved = _move_to_wednesday(repo, run_id, week)

    repo.update_fixed_run(fixed_id, note="bring pots")
    _apply_fixed_to_runs(bot, fixed_id, {"note"})

    assert repo.get_run(run_id)["datetime"] == moved


def test_editing_participants_does_not_drag_the_run_back_to_monday(setup, repo, week):
    bot, fixed_id, run_id = setup
    moved = _move_to_wednesday(repo, run_id, week)

    repo.update_fixed_run(fixed_id, participants=["1", "2"])
    _apply_fixed_to_runs(bot, fixed_id, {"participants"})

    run = repo.get_run(run_id)
    assert run["datetime"] == moved  # the amendment survives
    assert run["participants"] == ["1", "2"]  # the edit still lands


def test_editing_bosses_updates_only_the_bosses(setup, repo, week):
    bot, fixed_id, run_id = setup
    moved = _move_to_wednesday(repo, run_id, week)

    repo.update_fixed_run(fixed_id, bosses=["HStar"])
    _apply_fixed_to_runs(bot, fixed_id, {"bosses"})

    run = repo.get_run(run_id)
    assert run["bosses"] == ["HStar"]
    assert run["datetime"] == moved


def test_editing_the_channel_moves_where_the_pings_go_but_not_when(setup, repo, week):
    bot, fixed_id, run_id = setup
    moved = _move_to_wednesday(repo, run_id, week)

    repo.update_fixed_run(fixed_id, channel_id="901")
    _apply_fixed_to_runs(bot, fixed_id, {"channel_id"})

    run = repo.get_run(run_id)
    assert run["channel_id"] == "901"
    assert run["datetime"] == moved


def test_editing_the_day_does_reschedule_the_run(setup, repo, week):
    bot, fixed_id, run_id = setup
    _move_to_wednesday(repo, run_id, week)

    repo.update_fixed_run(fixed_id, weekday=4)  # Fri
    _apply_fixed_to_runs(bot, fixed_id, {"weekday"})

    run = repo.get_run(run_id)
    friday = slot_in_week(week, TZ, 4, time(21, 30))
    assert run["datetime"] == friday
    # ...and the morning ping follows it to Friday.
    day_of = next(r for r in repo.list_reminders(run_id) if r["kind"] == "day_of")
    assert day_of["fire_at"] == friday.astimezone(TZ).replace(hour=9, minute=0)


def test_editing_the_time_reschedules_too(setup, repo, week):
    bot, fixed_id, run_id = setup
    repo.update_fixed_run(fixed_id, time="22:30")
    _apply_fixed_to_runs(bot, fixed_id, {"time"})
    assert repo.get_run(run_id)["datetime"] == slot_in_week(week, TZ, 0, time(22, 30))


def test_a_cancelled_run_is_left_alone(setup, repo, week):
    bot, fixed_id, run_id = setup
    repo.set_run_status(run_id, "cancelled")
    before = repo.get_run(run_id)["datetime"]

    repo.update_fixed_run(fixed_id, weekday=4)
    _apply_fixed_to_runs(bot, fixed_id, {"weekday"})

    assert repo.get_run(run_id)["datetime"] == before


def test_no_changed_fields_is_a_no_op(setup, repo, week):
    bot, fixed_id, run_id = setup
    moved = _move_to_wednesday(repo, run_id, week)
    _apply_fixed_to_runs(bot, fixed_id, set())
    assert repo.get_run(run_id)["datetime"] == moved


# -- bug 6: editing someone else's party must not add the editor -------------


def test_add_includes_the_invoker(repo: Repo):
    bot = FakeBot(repo)
    ids, problem = _resolve_participants(bot, "<@2> <@3>", invoker_id=1)
    assert problem is None
    assert ids == ["1", "2", "3"]


def test_edit_takes_the_list_as_given(repo: Repo):
    bot = FakeBot(repo)
    ids, problem = _resolve_participants(bot, "<@2> <@3>", invoker_id=1, include_invoker=False)
    assert problem is None
    assert ids == ["2", "3"]  # the admin editing this party is not on it


def test_edit_rejects_an_empty_participant_list(repo: Repo):
    bot = FakeBot(repo)
    ids, problem = _resolve_participants(bot, "", invoker_id=1, include_invoker=False)
    assert ids == []
    assert "at least one participant" in problem


def test_a_name_that_matches_nobody_says_so(repo: Repo):
    bot = FakeBot(repo)
    ids, problem = _resolve_participants(bot, "nobodyhere", invoker_id=1, include_invoker=False)
    assert ids == []
    assert "couldn't match" in problem and "nobodyhere" in problem


def test_pickers_alone_are_enough(repo: Repo):
    bot = FakeBot(repo)
    picked = (SimpleNamespace(id=2, bot=False), SimpleNamespace(id=3, bot=False), None)
    ids, problem = _resolve_participants(
        bot, None, invoker_id=1, picked=picked, include_invoker=False
    )
    assert problem is None
    assert ids == ["2", "3"]


def test_add_puts_the_invoker_first_then_pickers(repo: Repo):
    bot = FakeBot(repo)
    picked = (SimpleNamespace(id=3, bot=False), SimpleNamespace(id=2, bot=False))
    ids, problem = _resolve_participants(bot, None, invoker_id=1, picked=picked)
    assert problem is None
    assert ids == ["1", "3", "2"]


def test_a_picked_bot_is_rejected(repo: Repo):
    bot = FakeBot(repo)
    picked = (SimpleNamespace(id=9, bot=True),)
    ids, problem = _resolve_participants(bot, None, invoker_id=1, picked=picked)
    assert "bots can't be participants" in problem
    assert "9" not in ids


def test_non_role_members_are_rejected(repo: Repo):
    bot = FakeBot(repo, role_members={"1", "2"})
    _, problem = _resolve_participants(bot, "<@2> <@99>", invoker_id=1)
    assert "not in the bossing role" in problem
    assert "<@99>" in problem


def test_bot_accounts_are_rejected_as_participants(repo: Repo):
    bot = FakeBot(repo)
    guild = SimpleNamespace(get_member=lambda uid: SimpleNamespace(bot=uid == 2))
    ids, problem = _resolve_participants(bot, "<@2> <@3>", invoker_id=1, guild=guild)
    assert "bots can't be participants" in problem
    assert "<@2>" in problem
    assert "2" not in ids  # and it is dropped, not silently kept
