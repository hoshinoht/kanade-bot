"""Runs that have already happened, and what "mine" means.

Two things the owner hit: a boss week is materialised whole, so by Sunday it
still lists Thursday's finished runs; and `/fixed list` filtered on
participation, which hid the timings someone had set up but was not on.
"""

from __future__ import annotations

from datetime import timedelta

import pytest

from bot.agent.materialise import (
    LIVE_STATUSES,
    RUN_DONE_AFTER,
    is_past,
    mark_done,
    materialise_week,
)
from bot.domain.weeks import current_week_start
from bot.infrastructure.db import Repo

from .conftest import COUNTDOWNS, PING_TIME, RESET_TIME, RESET_WEEKDAY, TZ, kl

# Sunday 30 Aug 2026 midday, inside the boss week that reset Thu 27 Aug.
NOW = kl(2026, 8, 30, 12, 0)
WEEK = current_week_start(TZ, RESET_WEEKDAY, RESET_TIME, NOW)


# --- is_past ----------------------------------------------------------------


def test_a_run_is_not_past_while_it_could_still_be_happening():
    assert is_past(NOW - RUN_DONE_AFTER + timedelta(minutes=1), NOW) is False


def test_a_run_is_past_once_the_grace_has_gone():
    assert is_past(NOW - RUN_DONE_AFTER - timedelta(minutes=1), NOW) is True


def test_a_future_run_is_never_past():
    assert is_past(NOW + timedelta(hours=1), NOW) is False


# --- materialisation --------------------------------------------------------


def test_materialising_mid_week_skips_slots_that_have_already_passed(repo: Repo):
    """Adding a Friday timing on Sunday must not conjure last Friday's run."""
    repo.add_fixed_run(1, ["HStar"], 4, "23:00", ["1"])  # Fri, already gone
    repo.add_fixed_run(1, ["XKalos"], 1, "23:00", ["1"])  # Tue, still to come
    created = materialise_week(repo, WEEK, TZ, PING_TIME, COUNTDOWNS, now=NOW)
    assert len(created) == 1
    assert [r["bosses"] for r in repo.list_runs()] == [["XKalos"]]


def test_a_slot_inside_the_grace_is_still_created(repo: Repo):
    repo.add_fixed_run(1, ["HStar"], 6, "11:00", ["1"])  # Sunday 11:00, an hour ago
    assert len(materialise_week(repo, WEEK, TZ, PING_TIME, COUNTDOWNS, now=NOW)) == 1


def test_materialising_a_whole_week_ahead_is_unchanged(repo: Repo):
    """The normal case -- materialising at the reset -- must not lose anything."""
    repo.add_fixed_run(1, ["HStar"], 4, "23:00", ["1"])
    repo.add_fixed_run(1, ["XKalos"], 1, "23:00", ["1"])
    at_reset = WEEK + timedelta(minutes=1)
    assert len(materialise_week(repo, WEEK, TZ, PING_TIME, COUNTDOWNS, now=at_reset)) == 2


# --- mark_done --------------------------------------------------------------


def make_run(repo: Repo, at, status="planned"):
    return repo.create_run(WEEK, ["HStar"], at, ["1"], status=status)


def test_a_run_whose_night_has_passed_is_marked_done(repo: Repo):
    run_id = make_run(repo, kl(2026, 8, 28, 23, 0))
    assert mark_done(repo, NOW) == [run_id]
    assert repo.get_run(run_id)["status"] == "done"


def test_marking_done_removes_the_pings_that_never_fired(repo: Repo):
    run_id = make_run(repo, kl(2026, 8, 28, 23, 0))
    repo.add_reminder(run_id, "day_of", kl(2026, 8, 28, 9, 0))
    repo.add_reminder(run_id, "countdown_60", kl(2026, 8, 28, 22, 0))
    mark_done(repo, NOW)
    assert repo.list_reminders(run_id) == []


def test_a_sent_reminder_is_kept_as_the_record(repo: Repo):
    run_id = make_run(repo, kl(2026, 8, 28, 23, 0))
    reminder = repo.add_reminder(run_id, "day_of", kl(2026, 8, 28, 9, 0))
    repo.mark_reminder_sent(reminder, 900000000000000001)
    mark_done(repo, NOW)
    assert [r["kind"] for r in repo.list_reminders(run_id)] == ["day_of"]


def test_a_future_run_is_left_alone(repo: Repo):
    run_id = make_run(repo, kl(2026, 9, 1, 23, 0))
    assert mark_done(repo, NOW) == []
    assert repo.get_run(run_id)["status"] == "planned"


@pytest.mark.parametrize("status", LIVE_STATUSES)
def test_every_live_status_can_be_retired(repo: Repo, status):
    run_id = make_run(repo, kl(2026, 8, 28, 23, 0), status)
    assert mark_done(repo, NOW) == [run_id]


def test_a_cancelled_run_stays_cancelled(repo: Repo):
    """It never happened; calling it `done` would misreport the week."""
    run_id = make_run(repo, kl(2026, 8, 28, 23, 0), "cancelled")
    assert mark_done(repo, NOW) == []
    assert repo.get_run(run_id)["status"] == "cancelled"


def test_marking_done_is_idempotent(repo: Repo):
    make_run(repo, kl(2026, 8, 28, 23, 0))
    assert len(mark_done(repo, NOW)) == 1
    assert mark_done(repo, NOW) == []


# --- "mine" is owner or participant -----------------------------------------


def test_a_timing_you_own_but_are_not_on_is_still_yours(repo: Repo):
    """`/fixed add` does not add the invoker, so this is the pilot's own run."""
    owned = repo.add_fixed_run(
        owner_id=1, bosses=["HStar"], weekday=0, time_hhmm="21:30", participants=["2", "3"]
    )
    assert [f["id"] for f in repo.list_fixed_runs(involving=1)] == [owned]
    assert repo.list_fixed_runs(participant="1") == []


def test_a_timing_you_are_on_is_yours(repo: Repo):
    on_it = repo.add_fixed_run(
        owner_id=9, bosses=["XKalos"], weekday=1, time_hhmm="23:00", participants=["1"]
    )
    assert [f["id"] for f in repo.list_fixed_runs(involving=1)] == [on_it]


def test_someone_elses_timing_is_not_yours(repo: Repo):
    repo.add_fixed_run(owner_id=9, bosses=["HFA"], weekday=2, time_hhmm="22:00", participants=["8"])
    assert repo.list_fixed_runs(involving=1) == []


def test_an_id_is_matched_whole_not_as_a_substring(repo: Repo):
    """Member 1 must not match member 11's runs."""
    eleven = repo.add_fixed_run(
        owner_id=9, bosses=["HFA"], weekday=2, time_hhmm="22:00", participants=["11"]
    )
    assert repo.list_fixed_runs(involving=1) == []
    assert [f["id"] for f in repo.list_fixed_runs(involving=11)] == [eleven]


def test_the_same_rule_reaches_the_runs_a_timing_produced(repo: Repo):
    fixed = repo.add_fixed_run(
        owner_id=1, bosses=["HStar"], weekday=1, time_hhmm="23:00", participants=["2"]
    )
    materialise_week(repo, WEEK, TZ, PING_TIME, COUNTDOWNS, now=WEEK + timedelta(minutes=1))
    run = repo.list_runs()[0]
    assert run["fixed_run_id"] == fixed
    assert [r["id"] for r in repo.list_runs(involving=1)] == [run["id"]]
    assert repo.list_runs(participant="1") == []


def test_listing_runs_can_be_narrowed_to_statuses(repo: Repo):
    live = make_run(repo, kl(2026, 9, 1, 23, 0))
    make_run(repo, kl(2026, 8, 28, 23, 0), "done")
    assert [r["id"] for r in repo.list_runs(statuses=LIVE_STATUSES)] == [live]
    assert len(repo.list_runs()) == 2
