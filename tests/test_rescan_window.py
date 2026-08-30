"""How much a rescan reads, how it is cut up, and what it refuses to propose."""

from __future__ import annotations

import asyncio
from datetime import timedelta

import pytest

from bot.extract.pipeline import STALE_GRACE, Planned, already_passed
from bot.extract.resolve import Resolved
from bot.extract.schema import Amendment
from bot.extract.window import (
    BURST_GAP,
    WINDOWS,
    group_bursts,
    previous_week_start,
    should_widen,
    window_since,
)
from bot.materialise import RUN_DONE_AFTER

from .conftest import RESET_TIME, RESET_WEEKDAY, TZ, kl

# Sunday 30 Aug 2026, midday. The boss week reset was Thu 27 Aug 00:00.
NOW = kl(2026, 8, 30, 12, 0)


# --- windows ----------------------------------------------------------------


def test_the_week_window_starts_at_the_reset_not_seven_days_ago():
    since = window_since("week", TZ, RESET_WEEKDAY, RESET_TIME, NOW)
    assert since == kl(2026, 8, 27, 0, 0)


def test_two_weeks_reaches_the_reset_before_that():
    since = window_since("2weeks", TZ, RESET_WEEKDAY, RESET_TIME, NOW)
    assert since == kl(2026, 8, 20, 0, 0)


@pytest.mark.parametrize("window,hours", [("24h", 24), ("48h", 48)])
def test_the_fixed_windows_are_measured_from_now(window, hours):
    assert window_since(window, TZ, RESET_WEEKDAY, RESET_TIME, NOW) == NOW - timedelta(hours=hours)


def test_an_unknown_window_names_the_ones_that_work():
    with pytest.raises(ValueError, match="week, 2weeks, 48h, 24h"):
        window_since("since forever", TZ, RESET_WEEKDAY, RESET_TIME, NOW)


def test_every_offered_window_resolves():
    assert all(window_since(w, TZ, RESET_WEEKDAY, RESET_TIME, NOW) < NOW for w in WINDOWS)


def test_the_previous_week_is_exactly_one_reset_earlier():
    this_week = kl(2026, 8, 27, 0, 0)
    assert previous_week_start(this_week, TZ, RESET_WEEKDAY, RESET_TIME) == kl(2026, 8, 20, 0, 0)


# --- the widen decision -----------------------------------------------------


def test_a_silent_week_widens_once():
    assert should_widen("week", 0) is True


def test_a_week_with_scheduling_chat_does_not_widen():
    assert should_widen("week", 1) is False


@pytest.mark.parametrize("window", ["2weeks", "48h", "24h"])
def test_only_the_default_window_widens(window):
    """Someone who asked for 24h meant 24h, and two weeks is already the cap."""
    assert should_widen(window, 0) is False


# --- burst grouping ---------------------------------------------------------


def row(minute_offset: int, mid: str = "m"):
    return {"id": f"{mid}{minute_offset}", "created_at": NOW + timedelta(minutes=minute_offset)}


def test_messages_close_together_are_one_conversation():
    rows = [row(0), row(3), row(9)]
    assert [len(g) for g in group_bursts(rows)] == [3]


def test_a_quiet_gap_starts_a_new_conversation():
    rows = [row(0), row(3), row(3 + int(BURST_GAP.total_seconds() // 60) + 1)]
    assert [len(g) for g in group_bursts(rows)] == [2, 1]


def test_a_gap_exactly_the_size_of_the_threshold_does_not_split():
    rows = [row(0), row(int(BURST_GAP.total_seconds() // 60))]
    assert [len(g) for g in group_bursts(rows)] == [2]


def test_no_messages_means_no_conversations():
    assert group_bursts([]) == []


def test_a_whole_week_becomes_several_conversations():
    """One prompt per week would be enormous; one per evening is the point."""
    rows = [row(day * 24 * 60 + minute) for day in range(4) for minute in (0, 2, 5)]
    assert [len(g) for g in group_bursts(rows)] == [3, 3, 3, 3]


def test_grouping_can_key_off_a_different_field():
    rows = [{"at": NOW}, {"at": NOW + timedelta(hours=1)}]
    assert len(group_bursts(rows, key=lambda r: r["at"])) == 2


# --- "already passed" -------------------------------------------------------


def planned(at=None, run=None):
    return Planned(
        amendment=Amendment(kind="move", confidence=0.9),
        resolved=Resolved(at=at),
        run=run,
    )


def run_row(at, status="planned"):
    return {"id": "r1", "datetime": at, "status": status, "bosses": ["HStar"], "participants": []}


def test_a_change_pointing_well_into_the_past_is_dropped():
    assert already_passed(planned(at=NOW - STALE_GRACE - timedelta(minutes=1)), NOW) is True


def test_a_change_being_settled_right_now_is_kept():
    """ "start now lah" a few minutes after the hour is a real amendment."""
    assert already_passed(planned(at=NOW - timedelta(minutes=20)), NOW) is False


def test_a_change_in_the_future_is_kept():
    assert already_passed(planned(at=NOW + timedelta(days=1)), NOW) is False


def test_a_change_with_no_time_yet_is_kept():
    """A suggestion whose day or time is still TBD is exactly what a card is for."""
    assert already_passed(planned(at=None), NOW) is False


@pytest.mark.parametrize("status", ["done", "cancelled"])
def test_a_change_to_a_finished_run_is_dropped(status):
    run = run_row(NOW + timedelta(days=1), status)
    assert already_passed(planned(at=NOW + timedelta(days=1), run=run), NOW) is True


def test_a_change_to_a_run_whose_night_has_passed_is_dropped():
    run = run_row(NOW - RUN_DONE_AFTER - timedelta(minutes=1))
    assert already_passed(planned(at=None, run=run), NOW) is True


def test_a_change_to_a_run_still_in_progress_is_kept():
    run = run_row(NOW - timedelta(minutes=30))
    assert already_passed(planned(at=None, run=run), NOW) is False


# --- the plan drops them ----------------------------------------------------


def test_plan_burst_drops_a_stale_move_with_a_reason(bosses):
    from bot.extract.pipeline import plan_burst
    from bot.extract.schema import Extraction

    run = run_row(kl(2026, 8, 24, 21, 30))  # last Monday
    extraction = Extraction(
        amendments=[
            Amendment(
                kind="move",
                bosses=["HStar"],
                day_ref="mon",
                time_ref="9:30pm",
                confidence=0.9,
                evidence_message_ids=["1"],
            )
        ],
        summary="move to monday",
    )
    plan = plan_burst(
        extraction,
        anchor=kl(2026, 8, 23, 20, 0),
        tz=TZ,
        channel_runs=[run],
        now=NOW,
    )
    assert plan.planned == []
    assert [entry.match_reason for entry in plan.dropped] == ["already passed"]


# --- the whole rescan -------------------------------------------------------


class StubPlan:
    def __init__(self, amendment_ids=(), dropped=(), error=None):
        self.amendment_ids = list(amendment_ids)
        self.dropped = list(dropped)
        self.planned = []
        self.error = error
        self.summary = ""
        self.raw = ""
        self.latency_ms = 5


def build_pipeline(fake_bot, plans=None):
    """A Pipeline whose model call is replaced by a canned plan per burst."""
    from bot.extract.pipeline import Pipeline

    pipeline = Pipeline.__new__(Pipeline)
    pipeline.bot = fake_bot
    pipeline._bursts = {}
    pipeline.extracted: list[list[dict]] = []
    queue = list(plans or [])

    async def extract(channel_id, rows, post=True):
        pipeline.extracted.append(rows)
        return queue.pop(0) if queue else StubPlan()

    pipeline.extract = extract
    return pipeline


def seed_chat(repo, channel_id, moments, author="1001"):
    repo.upsert_member(author, "kanon", None, True)
    for index, moment in enumerate(moments):
        repo.record_message(1000 + index, channel_id, author, moment, "we doing hstar tonight?")


def test_a_rescan_backfills_before_it_reads(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    seed_chat(fake_bot.repo, WATCHED_CHANNEL, [kl(2026, 8, 28, 20, 0)])
    fake_bot.backfill_count = 12
    pipeline = build_pipeline(fake_bot)
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL))
    assert report.backfilled == 12
    assert fake_bot.backfills, "history must be pulled from Discord first"
    # The window's start, not "24 hours ago": a rescan covers the boss week.
    assert fake_bot.backfills[0][1].astimezone(TZ).strftime("%a") == "Thu"


def test_a_week_of_chat_becomes_one_call_per_conversation(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    seed_chat(
        fake_bot.repo,
        WATCHED_CHANNEL,
        [kl(2026, 8, 28, 20, 0), kl(2026, 8, 28, 20, 5), kl(2026, 8, 29, 21, 0)],
    )
    pipeline = build_pipeline(fake_bot, [StubPlan(["a"]), StubPlan(["b", "c"])])
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL))
    assert report.bursts == 2
    assert report.extracted == 2
    assert report.proposals == 3
    assert [len(rows) for rows in pipeline.extracted] == [2, 1]


def test_an_empty_week_widens_to_the_previous_one(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    pipeline = build_pipeline(fake_bot)
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL, window="week"))
    assert report.widened is True
    assert report.bursts == 0
    # Two backfills: the week, then the week before it.
    assert len(fake_bot.backfills) == 2
    assert fake_bot.backfills[1][1] < fake_bot.backfills[0][1]


def test_a_week_with_chat_does_not_widen(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    seed_chat(fake_bot.repo, WATCHED_CHANNEL, [kl(2026, 8, 28, 20, 0)])
    pipeline = build_pipeline(fake_bot)
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL))
    assert report.widened is False
    assert len(fake_bot.backfills) == 1


def test_a_narrow_window_never_widens(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    pipeline = build_pipeline(fake_bot)
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL, window="24h"))
    assert report.widened is False


def test_messages_from_people_without_the_role_are_not_read(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    fake_bot.repo.upsert_member(2002, "a lurker", None, False)
    fake_bot.repo.record_message(
        50, WATCHED_CHANNEL, 2002, kl(2026, 8, 28, 20, 0), "hstar tonight?"
    )
    pipeline = build_pipeline(fake_bot)
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL))
    assert report.gated == 0
    assert report.extracted == 0


def test_banter_never_reaches_the_model(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    fake_bot.repo.upsert_member(1001, "kanon", None, True)
    fake_bot.repo.record_message(60, WATCHED_CHANNEL, 1001, kl(2026, 8, 28, 20, 0), "cao botter")
    pipeline = build_pipeline(fake_bot)
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL))
    assert report.stored == 1
    assert report.gated == 0
    assert report.asked is False


def test_a_model_failure_is_reported_not_raised(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    seed_chat(fake_bot.repo, WATCHED_CHANNEL, [kl(2026, 8, 28, 20, 0)])
    pipeline = build_pipeline(fake_bot, [StubPlan(error="connection refused")])
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL))
    assert report.errors == ["connection refused"]
