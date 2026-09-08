"""Materialisation and reminder generation."""

from __future__ import annotations

from datetime import time, timedelta

import pytest

from bot.agent.materialise import (
    ensure_reminders,
    materialise_week,
    refresh_run_reminders,
    reminder_specs,
)
from bot.infrastructure.db import Repo

from .conftest import COUNTDOWNS, PING_TIME, TZ, kl

WEEK = kl(2026, 8, 27)  # Thu 00:00
MON = kl(2026, 8, 31, 21, 30)
TUE = kl(2026, 9, 1, 23, 0)
#: Half an hour after the Monday weekly, so an adoption is visible as a retime.
MON_LATE = kl(2026, 8, 31, 22, 0)


def add_mon_run(repo: Repo, channel_id: int | None = 900) -> int:
    return repo.add_fixed_run(
        "1",
        ["HMaleficStar", "HFA"],
        weekday=0,
        time_hhmm="21:30",
        participants=["1", "2", "3"],
        channel_id=channel_id,
    )


def add_tue_run(repo: Repo, channel_id: int | None = 901) -> int:
    return repo.add_fixed_run(
        "1",
        ["HCarling", "XKalos"],
        weekday=1,
        time_hhmm="23:00",
        participants=["1", "2"],
        channel_id=channel_id,
    )


def materialise(repo: Repo, now=None):
    return materialise_week(repo, WEEK, TZ, PING_TIME, COUNTDOWNS, now=now or kl(2026, 8, 27, 1))


def add_oneoff(
    repo: Repo,
    at=MON_LATE,
    bosses=("HMaleficStar", "HFA"),
    channel_id: int | None = 900,
    participants=("1", "9"),
    status: str = "planned",
    week_start=WEEK,
) -> str:
    """A standalone run somebody scheduled for themselves -- no weekly behind it."""
    run_id = repo.create_run(
        week_start=week_start,
        bosses=list(bosses),
        run_at=at,
        participants=list(participants),
        status=status,
        source="amend",
        channel_id=channel_id,
    )
    ensure_reminders(repo, repo.get_run(run_id), TZ, PING_TIME, COUNTDOWNS, now=kl(2026, 8, 27, 1))
    return run_id


# -- materialisation ---------------------------------------------------------


def test_fixed_runs_become_runs_at_the_right_local_time(repo: Repo):
    add_mon_run(repo)
    add_tue_run(repo)
    created = materialise(repo)
    assert len(created) == 2
    runs = repo.list_runs(week_start=WEEK)
    assert [r["datetime"] for r in runs] == [MON, TUE]
    assert runs[0]["bosses"] == ["HMaleficStar", "HFA"]
    assert runs[0]["status"] == "planned"
    assert runs[0]["source"] == "fixed"
    assert runs[0]["participants"] == ["1", "2", "3"]


def test_materialising_twice_creates_nothing_new(repo: Repo):
    add_mon_run(repo)
    assert len(materialise(repo)) == 1
    assert materialise(repo) == []
    assert len(repo.list_runs(week_start=WEEK)) == 1


def test_a_new_fixed_run_is_picked_up_without_duplicating_the_old_ones(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    add_tue_run(repo)
    created = materialise(repo)
    assert len(created) == 1
    assert len(repo.list_runs(week_start=WEEK)) == 2


def test_each_week_gets_its_own_run(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    next_week = kl(2026, 9, 3)
    materialise_week(repo, next_week, TZ, PING_TIME, COUNTDOWNS, now=kl(2026, 8, 27, 1))
    assert len(repo.list_runs(week_start=WEEK)) == 1
    assert len(repo.list_runs(week_start=next_week)) == 1
    assert repo.list_runs(week_start=next_week)[0]["datetime"] == kl(2026, 9, 7, 21, 30)


# -- adoption: a weekly folds in the one-off the week already had -------------
#
# "Make this run every week" used to leave the party with two nights for the
# same boss: the one carrying their answers, and the one carrying the pings.
# `/fixed add`, the portal and the chatbot's ✅ all reach materialisation, so
# adoption living here is what makes the three doors agree.


def test_a_new_weekly_takes_over_this_weeks_matching_one_off(repo: Repo):
    run_id = add_oneoff(repo)
    add_mon_run(repo)

    created = materialise(repo)

    assert created == []  # nothing was made; something was taken over
    runs = repo.list_runs(week_start=WEEK)
    assert [r["id"] for r in runs] == [run_id]
    assert runs[0]["datetime"] == MON  # retimed to the weekly's slot
    assert runs[0]["participants"] == ["1", "2", "3"]  # and to its party
    assert runs[0]["source"] == "amend"  # still the run it always was


def test_the_adopted_run_belongs_to_the_timing_afterwards(repo: Repo):
    run_id = add_oneoff(repo)
    fixed_id = add_mon_run(repo)
    materialise(repo)

    assert repo.get_run(run_id)["fixed_run_id"] == fixed_id
    assert repo.run_for_fixed(fixed_id, WEEK)["id"] == run_id
    # And the week is settled: a second pass has nothing left to do.
    assert materialise(repo) == []
    assert len(repo.list_runs(week_start=WEEK)) == 1


def test_the_adopted_run_is_pinged_for_its_new_slot(repo: Repo):
    run_id = add_oneoff(repo, at=MON_LATE)
    add_mon_run(repo)
    materialise(repo)

    by_kind = {r["kind"]: r["fire_at"] for r in repo.list_reminders(run_id)}
    assert by_kind["countdown_60"] == MON - timedelta(minutes=60)
    assert by_kind["day_of"] == kl(2026, 8, 31, 9, 0)


def test_answers_from_members_who_stayed_survive_the_adoption(repo: Repo):
    """The party changes, so the run goes back to being derived -- not wiped."""
    run_id = add_oneoff(repo, participants=("1", "9"))
    repo.set_rsvp(run_id, "1", "yes")
    repo.set_rsvp(run_id, "9", "yes")
    repo.set_run_status(run_id, "confirmed")
    add_mon_run(repo)

    materialise(repo)

    assert repo.get_rsvps(run_id)["1"] == "yes"
    # 1 stayed, 2 and 3 have not answered: nobody may call that confirmed.
    assert repo.get_run(run_id)["status"] == "planned"


def test_a_party_that_does_not_change_leaves_a_confirmed_run_confirmed(repo: Repo):
    """Nobody joined, so nothing argues with the decision somebody made.

    The tally is deliberately incomplete: recomputing anyway would put this run
    back to `planned`, which is the thing a manual confirm exists to resist.
    """
    run_id = add_oneoff(repo, participants=("1", "2", "3"))
    repo.set_rsvp(run_id, "1", "yes")
    repo.set_run_status(run_id, "confirmed")
    add_mon_run(repo)

    materialise(repo)

    assert repo.get_run(run_id)["datetime"] == MON  # it was adopted
    assert repo.get_run(run_id)["status"] == "confirmed"


# -- ...and the cases where a duplicate is the safer answer ------------------


def test_two_runs_that_both_fit_are_left_alone(repo: Repo):
    """Ambiguous adoption reschedules a night nobody asked about; a duplicate
    is something anybody can cancel."""
    first = add_oneoff(repo, at=MON_LATE)
    second = add_oneoff(repo, at=kl(2026, 8, 31, 23, 0))
    add_mon_run(repo)

    created = materialise(repo)

    assert len(created) == 1
    assert repo.get_run(first)["fixed_run_id"] is None
    assert repo.get_run(second)["fixed_run_id"] is None


@pytest.mark.parametrize("status", ["done", "cancelled"])
def test_a_night_that_is_over_or_called_off_is_not_adopted(repo: Repo, status: str):
    run_id = add_oneoff(repo, status=status)
    add_mon_run(repo)

    assert len(materialise(repo)) == 1
    assert repo.get_run(run_id)["fixed_run_id"] is None


def test_a_run_whose_night_has_passed_is_not_adopted(repo: Repo):
    run_id = add_oneoff(repo, at=kl(2026, 8, 28, 21, 30))  # Friday, long gone
    add_mon_run(repo)

    assert len(materialise(repo, now=kl(2026, 8, 31, 12, 0))) == 1
    assert repo.get_run(run_id)["fixed_run_id"] is None


def test_a_different_boss_set_is_not_adopted(repo: Repo):
    run_id = add_oneoff(repo, bosses=("HMaleficStar",))  # the weekly is HMaleficStar + HFA
    add_mon_run(repo)

    assert len(materialise(repo)) == 1
    assert repo.get_run(run_id)["fixed_run_id"] is None


def test_another_partys_channel_is_not_adopted_from(repo: Repo):
    run_id = add_oneoff(repo, channel_id=901)
    add_mon_run(repo, channel_id=900)

    assert len(materialise(repo)) == 1
    assert repo.get_run(run_id)["fixed_run_id"] is None


def test_a_run_another_timing_already_produced_is_not_adopted(repo: Repo):
    """Its own timing decides its slot; a second one must not take it over."""
    add_mon_run(repo)
    materialise(repo)
    existing = repo.list_runs(week_start=WEEK)[0]

    add_tue_run(repo, channel_id=900)
    repo.update_fixed_run(
        next(f["id"] for f in repo.list_fixed_runs() if "HCarling" in f["bosses"]),
        bosses=["HMaleficStar", "HFA"],
    )
    created = materialise(repo)

    assert len(created) == 1
    assert repo.get_run(existing["id"])["datetime"] == MON


def test_next_weeks_one_off_is_not_this_weeks_run(repo: Repo):
    next_week = kl(2026, 9, 3)
    run_id = add_oneoff(repo, at=kl(2026, 9, 7, 22, 0), week_start=next_week)
    add_mon_run(repo)

    assert len(materialise(repo)) == 1
    assert repo.get_run(run_id)["fixed_run_id"] is None


def test_fixed_add_now_adopts_where_it_used_to_duplicate(repo: Repo):
    """`/fixed add` writes the row and calls ``materialise_weeks``; nothing else.

    So the door gets adoption for free, and this pins that: before, the member
    who already had Monday's run on the board ended up with two.
    """
    run_id = add_oneoff(repo)
    fixed_id = repo.add_fixed_run(
        "1",
        ["HMaleficStar", "HFA"],
        weekday=0,
        time_hhmm="21:30",
        participants=["1", "2"],
        channel_id=900,
    )
    materialise(repo)

    assert [r["id"] for r in repo.list_runs(week_start=WEEK)] == [run_id]
    assert repo.get_run(run_id)["fixed_run_id"] == fixed_id


# -- reminder specs (pure) ---------------------------------------------------


def test_reminder_times_are_computed_in_guild_local_time():
    specs = reminder_specs(MON, "planned", TZ, PING_TIME, COUNTDOWNS)
    by_kind = {s.kind: s.fire_at for s in specs}
    assert by_kind["day_of"] == kl(2026, 8, 31, 9, 0)
    assert by_kind["countdown_60"] == kl(2026, 8, 31, 20, 30)
    assert by_kind["countdown_15"] == kl(2026, 8, 31, 21, 15)


def test_a_run_just_after_midnight_is_pinged_the_morning_before():
    just_after_midnight = kl(2026, 9, 1, 0, 30)
    specs = {
        s.kind: s.fire_at
        for s in reminder_specs(just_after_midnight, "planned", TZ, PING_TIME, COUNTDOWNS)
    }
    assert specs["day_of"] == kl(2026, 8, 31, 9, 0)
    assert specs["day_of"] < just_after_midnight


def test_otot_keeps_only_the_day_of_ping():
    specs = reminder_specs(MON, "otot", TZ, PING_TIME, COUNTDOWNS)
    assert [s.kind for s in specs] == ["day_of"]


def test_cancelled_and_done_runs_get_nothing():
    assert reminder_specs(MON, "cancelled", TZ, PING_TIME, COUNTDOWNS) == []
    assert reminder_specs(MON, "done", TZ, PING_TIME, COUNTDOWNS) == []


# -- reminder rows -----------------------------------------------------------


def test_reminders_are_created_once_per_run_and_kind(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    run = repo.list_runs(week_start=WEEK)[0]
    kinds = sorted(r["kind"] for r in repo.list_reminders(run["id"]))
    assert kinds == ["countdown_15", "countdown_60", "day_of"]

    materialise(repo)  # idempotent
    assert len(repo.list_reminders(run["id"])) == 3


def test_reminders_already_in_the_past_are_marked_sent(repo: Repo):
    add_mon_run(repo)
    # "Now" is after the morning ping but before the run itself.
    materialise(repo, now=kl(2026, 8, 31, 12, 0))
    run = repo.list_runs(week_start=WEEK)[0]
    by_kind = {r["kind"]: r for r in repo.list_reminders(run["id"])}
    assert by_kind["day_of"]["sent_at"] is not None  # never spam a stale ping
    assert by_kind["countdown_60"]["sent_at"] is None
    assert by_kind["countdown_15"]["sent_at"] is None
    assert repo.due_reminders(kl(2026, 8, 31, 12, 0)) == []


def test_due_reminders_only_returns_unsent_ones_whose_time_has_come(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    assert repo.due_reminders(kl(2026, 8, 31, 8, 59)) == []
    due = repo.due_reminders(kl(2026, 8, 31, 9, 0))
    assert [r["kind"] for r in due] == ["day_of"]

    repo.mark_reminder_sent(due[0]["id"], message_id=999)
    assert repo.due_reminders(kl(2026, 8, 31, 9, 0)) == []
    assert [r["run_id"] for r in repo.reminders_by_message(999)] == [due[0]["run_id"]]


def test_going_otot_drops_the_countdowns_but_keeps_the_morning_ping(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    run_id = repo.list_runs(week_start=WEEK)[0]["id"]

    repo.set_run_status(run_id, "otot")
    refresh_run_reminders(repo, run_id, TZ, PING_TIME, COUNTDOWNS, now=kl(2026, 8, 27, 1))
    assert [r["kind"] for r in repo.list_reminders(run_id)] == ["day_of"]


def test_cancelling_drops_every_unsent_reminder(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    run_id = repo.list_runs(week_start=WEEK)[0]["id"]

    repo.set_run_status(run_id, "cancelled")
    refresh_run_reminders(repo, run_id, TZ, PING_TIME, COUNTDOWNS, now=kl(2026, 8, 27, 1))
    assert repo.list_reminders(run_id) == []


def test_cancelling_after_the_morning_ping_clears_that_row_too(repo: Repo):
    add_mon_run(repo)
    materialise(repo, now=kl(2026, 8, 31, 12, 0))  # day_of already sent
    run_id = repo.list_runs(week_start=WEEK)[0]["id"]

    repo.set_run_status(run_id, "cancelled")
    refresh_run_reminders(repo, run_id, TZ, PING_TIME, COUNTDOWNS, now=kl(2026, 8, 31, 12, 0))
    # A rebuild drops sent rows as well (see below); the attendance answers
    # themselves live in `rsvps`, which is untouched.
    assert repo.list_reminders(run_id) == []


def test_moving_a_run_moves_its_unsent_reminders(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    run_id = repo.list_runs(week_start=WEEK)[0]["id"]

    moved_to = kl(2026, 9, 2, 21, 30)  # Mon -> Wed
    repo.set_run_datetime(run_id, moved_to, WEEK)
    refresh_run_reminders(repo, run_id, TZ, PING_TIME, COUNTDOWNS, now=kl(2026, 8, 27, 1))

    by_kind = {r["kind"]: r["fire_at"] for r in repo.list_reminders(run_id)}
    assert by_kind["day_of"] == kl(2026, 9, 2, 9, 0)
    assert by_kind["countdown_60"] == moved_to - timedelta(minutes=60)


def test_changing_the_countdown_offsets_prunes_the_stale_ones(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    run = repo.list_runs(week_start=WEEK)[0]

    ensure_reminders(repo, run, TZ, PING_TIME, [30], now=kl(2026, 8, 27, 1))
    assert sorted(r["kind"] for r in repo.list_reminders(run["id"])) == [
        "countdown_30",
        "day_of",
    ]


def test_pingtime_change_reschedules_the_morning_ping(repo: Repo):
    add_mon_run(repo)
    materialise(repo)
    run_id = repo.list_runs(week_start=WEEK)[0]["id"]

    refresh_run_reminders(repo, run_id, TZ, time(7, 30), COUNTDOWNS, now=kl(2026, 8, 27, 1))
    by_kind = {r["kind"]: r["fire_at"] for r in repo.list_reminders(run_id)}
    assert by_kind["day_of"] == kl(2026, 8, 31, 7, 30)


# -- home channels (DESIGN.md §1, "Party channels") --------------------------


def test_a_run_inherits_its_fixed_runs_home_channel(repo: Repo):
    add_mon_run(repo, channel_id=900)
    materialise(repo)
    assert repo.list_runs(week_start=WEEK)[0]["channel_id"] == "900"


def test_a_fixed_run_without_a_home_channel_leaves_it_unset(repo: Repo):
    add_mon_run(repo, channel_id=None)
    materialise(repo)
    # Falls back to POST_CHANNEL_ID at posting time.
    assert repo.list_runs(week_start=WEEK)[0]["channel_id"] is None


def test_each_party_keeps_its_own_channel(repo: Repo):
    add_mon_run(repo, channel_id=900)
    add_tue_run(repo, channel_id=901)
    materialise(repo)
    assert [r["channel_id"] for r in repo.list_runs(week_start=WEEK)] == ["900", "901"]


# -- regression: a same-week move must still get a morning ping --------------


def test_moving_a_run_later_in_the_week_gets_a_fresh_day_of_ping(repo: Repo):
    """A run moved Mon -> Wed *after* Monday's 09:00 ping must be pinged on Wed.

    The sent Monday `day_of` row used to survive the rebuild, and
    UNIQUE(run_id, kind) then silently blocked the Wednesday one.
    """
    add_mon_run(repo)
    monday_noon = kl(2026, 8, 31, 12, 0)
    materialise(repo, now=monday_noon)
    run_id = repo.list_runs(week_start=WEEK)[0]["id"]
    assert repo.list_reminders(run_id)[0]["sent_at"] is not None  # Monday ping fired

    wednesday = kl(2026, 9, 2, 21, 30)
    repo.set_run_datetime(run_id, wednesday, WEEK)
    refresh_run_reminders(repo, run_id, TZ, PING_TIME, COUNTDOWNS, now=monday_noon)

    by_kind = {r["kind"]: r for r in repo.list_reminders(run_id)}
    assert by_kind["day_of"]["fire_at"] == kl(2026, 9, 2, 9, 0)
    assert by_kind["day_of"]["sent_at"] is None  # still to come
    assert repo.due_reminders(kl(2026, 9, 2, 9, 0))


def test_the_superseded_message_stops_mapping_to_the_run(repo: Repo):
    # Intended: a move resets the run to `planned`, so ✅/❌ given for the old
    # time must be given again rather than silently carrying over.
    add_mon_run(repo)
    materialise(repo, now=kl(2026, 8, 31, 9, 0))
    run_id = repo.list_runs(week_start=WEEK)[0]["id"]
    day_of = repo.list_reminders(run_id)[0]
    repo.mark_reminder_sent(day_of["id"], message_id=4242)
    assert repo.reminders_by_message(4242)

    repo.set_run_datetime(run_id, kl(2026, 9, 2, 21, 30), WEEK)
    refresh_run_reminders(repo, run_id, TZ, PING_TIME, COUNTDOWNS, now=kl(2026, 8, 31, 9, 0))
    assert repo.reminders_by_message(4242) == []


def test_a_move_into_the_past_is_not_replayed(repo: Repo):
    add_mon_run(repo)
    materialise(repo, now=kl(2026, 8, 27, 1))
    run_id = repo.list_runs(week_start=WEEK)[0]["id"]

    # Rebuild "now" being after the new day_of time: it must be written as sent.
    repo.set_run_datetime(run_id, kl(2026, 8, 28, 21, 30), WEEK)
    refresh_run_reminders(repo, run_id, TZ, PING_TIME, COUNTDOWNS, now=kl(2026, 8, 29, 12, 0))
    by_kind = {r["kind"]: r for r in repo.list_reminders(run_id)}
    assert by_kind["day_of"]["sent_at"] is not None
    assert repo.due_reminders(kl(2026, 8, 29, 12, 0)) == []
