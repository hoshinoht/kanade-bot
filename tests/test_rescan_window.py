"""How much a rescan reads, how it is cut up, and what it refuses to propose."""

from __future__ import annotations

import asyncio
from datetime import timedelta

import pytest

from bot.agent.materialise import RUN_DONE_AFTER
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
    rows = [{"at": NOW}, {"at": NOW + timedelta(hours=4)}]
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
    """Stands in for a Plan without going near the model."""

    def __init__(self, amendment_ids=(), dropped=(), error=None, planned=()):
        self.amendment_ids = list(amendment_ids)
        self.dropped = list(dropped)
        self.planned = list(planned)
        self.error = error
        self.summary = ""
        self.raw = ""
        self.latency_ms = 5
        self.message_ids = []
        self.week_start = NOW


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


# ---------------------------------------------------------------------------
# rescan quality: grouping, context, consolidation, stale days, time carry-over
# ---------------------------------------------------------------------------


def at(day: int, hour: int, minute: int = 0):
    return {"id": f"{day}-{hour:02d}{minute:02d}", "created_at": kl(2026, 8, day, hour, minute)}


def test_a_planning_thread_with_long_pauses_stays_one_conversation():
    """The real 30 Aug thread ran 11:50 -> 13:15 with fifty-minute pauses.

    At a fifteen-minute gap it came apart into six bursts of one to four
    messages, and the model never saw "then weds lah" next to
    "Wed i can from 9:30pm" -- so it produced TBD cards from a settled thread.
    """
    from bot.extract.window import group_for_rescan

    thread = [at(30, 11, 50), at(30, 11, 55), at(30, 12, 45), at(30, 13, 7), at(30, 13, 15)]
    assert [len(g) for g in group_for_rescan(thread, TZ)] == [5]


def test_separate_days_are_separate_conversations():
    from bot.extract.window import group_for_rescan

    rows = [at(29, 21, 0), at(30, 11, 50), at(30, 13, 15)]
    assert [len(g) for g in group_for_rescan(rows, TZ)] == [1, 2]


def test_a_day_too_big_for_one_prompt_is_split_at_its_longest_pause():
    from bot.extract.window import MAX_BURST_MESSAGES, group_for_rescan

    morning = [at(30, 9, m) for m in range(0, 60, 2)]  # 30 messages
    evening = [at(30, 21, m) for m in range(0, 40, 2)]  # 20 messages
    groups = group_for_rescan(morning + evening, TZ)
    assert all(len(g) <= MAX_BURST_MESSAGES for g in groups)
    # The eleven-hour gap is the first cut, so no burst straddles it and the
    # morning's messages are never read as context for the evening's.
    assert [len(g) for g in groups if g[0] in morning] == [
        len(g) for g in groups if g[-1] in morning
    ]
    assert sum(len(g) for g in groups if g[0] in morning) == len(morning)


def test_evenly_spaced_chatter_splits_down_the_middle():
    """No natural break, so the cap decides -- and it should not shave one off."""
    from bot.extract.window import MAX_BURST_MESSAGES, group_for_rescan

    rows = [at(30, 10 + i // 6, (i % 6) * 10) for i in range(50)]
    sizes = [len(g) for g in group_for_rescan(rows, TZ)]
    assert sum(sizes) == 50
    assert max(sizes) <= MAX_BURST_MESSAGES
    # Halving, not shaving: each cut lands mid-conversation, so no piece comes
    # out as the one or two messages a split-at-the-cap would leave behind.
    assert min(sizes) >= MAX_BURST_MESSAGES // 2


def test_a_burst_is_capped_at_what_one_prompt_can_carry():
    """The cap is what stopped a 21-message day overrunning the context window."""
    from bot.extract.window import MAX_BURST_MESSAGES, group_for_rescan

    day = [at(30, 9 + i // 6, (i % 6) * 10) for i in range(21)]
    groups = group_for_rescan(day, TZ)
    assert len(groups) > 1
    assert all(len(g) <= MAX_BURST_MESSAGES for g in groups)


def test_the_local_day_is_what_counts_not_utc():
    """A 00:30 run's chat belongs to the night before it, in the guild timezone."""
    from bot.extract.window import group_for_rescan

    rows = [at(30, 23, 50), at(31, 0, 20)]
    assert [len(g) for g in group_for_rescan(rows, TZ)] == [1, 1]


# --- context is what came before the burst ----------------------------------


def context_pipeline(fake_bot):
    from bot.extract.pipeline import Pipeline

    pipeline = Pipeline.__new__(Pipeline)
    pipeline.bot = fake_bot
    pipeline._bursts = {}
    return pipeline


def test_context_is_the_messages_before_the_burst_not_after_it(fake_bot):
    """A rescan replays old chat; "the last 25 messages" would include the answers."""
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    repo.upsert_member(1001, "kanon", None, True)
    for index, moment in enumerate(
        [kl(2026, 8, 29, 20, 0), kl(2026, 8, 29, 20, 5), kl(2026, 8, 30, 12, 0)]
    ):
        repo.record_message(700 + index, WATCHED_CHANNEL, 1001, moment, f"m{index}")
    burst = [repo.get_message(702)]

    rows = context_pipeline(fake_bot)._context_rows(str(WATCHED_CHANNEL), burst)
    assert [r["content"] for r in rows] == ["m0", "m1"]


def test_context_never_reaches_back_further_than_the_window(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    repo.upsert_member(1001, "kanon", None, True)
    repo.record_message(710, WATCHED_CHANNEL, 1001, kl(2026, 8, 20, 20, 0), "ancient")
    repo.record_message(711, WATCHED_CHANNEL, 1001, kl(2026, 8, 30, 12, 0), "the burst")
    burst = [repo.get_message(711)]
    assert context_pipeline(fake_bot)._context_rows(str(WATCHED_CHANNEL), burst) == []


def test_context_excludes_the_burst_itself(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    repo.upsert_member(1001, "kanon", None, True)
    repo.record_message(720, WATCHED_CHANNEL, 1001, kl(2026, 8, 30, 12, 0), "one")
    repo.record_message(721, WATCHED_CHANNEL, 1001, kl(2026, 8, 30, 12, 1), "two")
    burst = [repo.get_message(720), repo.get_message(721)]
    assert context_pipeline(fake_bot)._context_rows(str(WATCHED_CHANNEL), burst) == []


def test_an_empty_burst_has_no_context(fake_bot):
    assert context_pipeline(fake_bot)._context_rows("1", []) == []


# --- one card per channel per rescan ----------------------------------------


def planned_for(kind: str, bosses: list[str], run=None, at_time=None):
    return Planned(
        amendment=Amendment(kind=kind, bosses=bosses, confidence=0.9),
        resolved=Resolved(at=at_time),
        run=run,
    )


def test_the_latest_burst_wins_for_the_same_run():
    from bot.extract.pipeline import consolidate

    run = run_row(NOW + timedelta(days=2))
    sunday = planned_for("move", ["HStar"], run, NOW + timedelta(days=2))
    monday = planned_for("move", ["HStar"], run, NOW + timedelta(days=3))
    assert consolidate([sunday, monday]) == [monday]


def test_different_runs_both_survive():
    from bot.extract.pipeline import consolidate

    star = planned_for("move", ["HStar"], {**run_row(NOW), "id": "r1"})
    kalos = planned_for("move", ["XKalos"], {**run_row(NOW), "id": "r2"})
    assert len(consolidate([star, kalos])) == 2


def test_one_run_gets_one_change_with_the_rest_noted():
    """A card offering both "move to Tue" and "own time" cannot be one ✅."""
    from bot.extract.pipeline import consolidate

    run = run_row(NOW)
    move = planned_for("move", [], run, NOW + timedelta(days=2))
    move.resolved = Resolved(day=(NOW + timedelta(days=2)).date(), at=NOW + timedelta(days=2))
    kept = consolidate([move, planned_for("otot", [], run)])
    assert [e.kind for e in kept] == ["move"]
    assert kept[0].also_mentioned == ["otot"]


def test_a_new_run_is_keyed_on_its_bosses():
    from bot.extract.pipeline import consolidate

    first = planned_for("add", ["NStar", "NCarling"])
    second = planned_for("add", ["NCarling", "NStar"])  # same set, said again later
    assert consolidate([first, second]) == [second]


def test_consolidating_nothing_gives_nothing():
    from bot.extract.pipeline import consolidate

    assert consolidate([]) == []


def test_a_rescan_posts_one_card_for_the_whole_window(fake_bot):
    """Not one per burst: a week is often one decision revisited across evenings."""
    from .fake_bot import WATCHED_CHANNEL

    seed_chat(
        fake_bot.repo,
        WATCHED_CHANNEL,
        [kl(2026, 8, 28, 20, 0), kl(2026, 8, 29, 20, 0), kl(2026, 8, 30, 20, 0)],
    )
    run = run_row(kl(2026, 9, 2, 21, 30))
    pipeline = build_pipeline(
        fake_bot,
        [
            StubPlan(planned=[planned_for("move", ["HStar"], run)]),
            StubPlan(planned=[planned_for("move", ["HStar"], run)]),
            StubPlan(planned=[planned_for("move", ["HStar"], run)]),
        ],
    )
    applied: list[list] = []

    async def apply_plan(channel_id, rsvps, proposals, week, summary, report=None):
        applied.append(proposals)
        return ["one-card"]

    pipeline.apply_plan = apply_plan
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL))
    assert report.bursts == 3
    assert len(applied) == 1, "a card per burst is what we are fixing"
    assert len(applied[0]) == 1
    assert report.proposals == 1


def test_a_dry_run_posts_nothing_at_all(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    seed_chat(fake_bot.repo, WATCHED_CHANNEL, [kl(2026, 8, 30, 20, 0)])
    run = run_row(kl(2026, 9, 2, 21, 30))
    pipeline = build_pipeline(fake_bot, [StubPlan(planned=[planned_for("move", ["HStar"], run)])])
    called = []
    pipeline.apply_plan = lambda *a, **k: called.append(a)
    report = asyncio.run(pipeline.rescan_window(WATCHED_CHANNEL, post=False))
    assert called == []
    assert report.proposals == 0


# --- a day that has already been -------------------------------------------


def test_a_proposal_for_a_past_day_with_no_time_is_dropped():
    """ "we doing our nstar tonight?" read back two days later is about a gone night."""
    entry = Planned(
        amendment=Amendment(kind="add", bosses=["NStar"], confidence=0.9),
        resolved=Resolved(day=(NOW - timedelta(days=1)).date()),
    )
    assert already_passed(entry, NOW, TZ) is True


def test_a_proposal_for_today_with_no_time_is_kept():
    entry = Planned(
        amendment=Amendment(kind="add", bosses=["NStar"], confidence=0.9),
        resolved=Resolved(day=NOW.astimezone(TZ).date()),
    )
    assert already_passed(entry, NOW, TZ) is False


def test_a_proposal_for_a_future_day_is_kept():
    entry = Planned(
        amendment=Amendment(kind="add", bosses=["NStar"], confidence=0.9),
        resolved=Resolved(day=(NOW + timedelta(days=2)).date()),
    )
    assert already_passed(entry, NOW, TZ) is False


def test_without_a_timezone_the_day_rule_cannot_fire():
    entry = Planned(
        amendment=Amendment(kind="add", confidence=0.9),
        resolved=Resolved(day=(NOW - timedelta(days=5)).date()),
    )
    assert already_passed(entry, NOW) is False


# --- one time stated for a day applies to every run moved to it -------------


def move(bosses, day_ref, time_ref=None, evidence=("1",)):
    return Amendment(
        kind="move",
        bosses=bosses,
        day_ref=day_ref,
        time_ref=time_ref,
        confidence=0.9,
        evidence_message_ids=list(evidence),
    )


def test_a_single_time_for_a_day_carries_to_the_other_move():
    from bot.extract.merge import merge

    merged = merge(
        [
            move(["HStar", "HFA"], "wed", evidence=["101"]),
            move(["HCarling", "XKalos"], "wed", "9:30pm", evidence=["103"]),
        ],
        message_order=["101", "103"],
    )
    times = {tuple(a.bosses): a.time_ref for a in merged}
    assert times[("HStar", "HFA")] == "9:30pm"
    assert times[("HCarling", "XKalos")] == "9:30pm"


def test_the_borrowed_time_cites_the_message_it_came_from():
    from bot.extract.merge import merge

    merged = merge(
        [move(["HStar"], "wed", evidence=["101"]), move(["XKalos"], "wed", "9:30pm", ["103"])],
        message_order=["101", "103"],
    )
    borrower = next(a for a in merged if a.bosses == ["HStar"])
    assert "103" in borrower.evidence_message_ids


def test_two_different_times_for_a_day_carry_nothing():
    """The thread has not settled; picking one would invent a decision."""
    from bot.extract.merge import merge

    merged = merge(
        [
            move(["HStar"], "wed", "9pm", ["101"]),
            move(["XKalos"], "wed", "11pm", ["102"]),
            move(["HFA"], "wed", evidence=["103"]),
        ],
        message_order=["101", "102", "103"],
    )
    assert next(a for a in merged if a.bosses == ["HFA"]).time_ref is None


def test_a_time_does_not_cross_to_another_day():
    from bot.extract.merge import merge

    merged = merge(
        [move(["HStar"], "wed", "9:30pm", ["101"]), move(["XKalos"], "thu", evidence=["102"])],
        message_order=["101", "102"],
    )
    assert next(a for a in merged if a.bosses == ["XKalos"]).time_ref is None


def test_a_move_that_already_has_a_time_keeps_it():
    from bot.extract.merge import merge

    merged = merge(
        [move(["HStar"], "wed", "10pm", ["101"]), move(["XKalos"], "wed", "9:30pm", ["102"])],
        message_order=["101", "102"],
    )
    assert next(a for a in merged if a.bosses == ["HStar"]).time_ref == "10pm"


# --- what reaches the database and the channel ------------------------------


def effects_pipeline(fake_bot):
    """A real Pipeline over the fake bot: `_record` and `_post_card` are the real ones."""
    from bot.extract.pipeline import Pipeline

    pipeline = Pipeline.__new__(Pipeline)
    pipeline.bot = fake_bot
    pipeline._bursts = {}
    return pipeline


def seeded_run(repo, channel_id, bosses=("HStar", "HFA")):
    run_id = repo.create_run(
        kl(2026, 8, 27), list(bosses), NOW + timedelta(days=3), ["1001"], channel_id=channel_id
    )
    return repo.get_run(run_id)


def test_two_changes_for_one_run_are_both_still_pressable(fake_bot):
    """Live: a card whose ✅ silently did nothing on two of its six lines.

    `_record` superseded as it went, so recording the `sub` retired the `move`
    it had just written -- and committing needs the row to still be `proposed`.
    """
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    run = seeded_run(repo, WATCHED_CHANNEL)
    pipeline = effects_pipeline(fake_bot)
    ids, _retired = pipeline._record(
        [planned_for("move", ["HStar"], run, NOW + timedelta(days=4)), planned_for("sub", [], run)],
        str(WATCHED_CHANNEL),
        kl(2026, 8, 27),
        "",
    )
    assert len(ids) == 2
    assert [repo.get_amendment(i)["status"] for i in ids] == ["proposed", "proposed"]


def test_an_older_card_for_the_same_run_is_still_retired(fake_bot):
    """The reason `supersede` exists must survive the fix to when it runs."""
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    run = seeded_run(repo, WATCHED_CHANNEL)
    stale = repo.create_amendment(
        week_start=kl(2026, 8, 27), kind="move", run_id=run["id"], channel_id=str(WATCHED_CHANNEL)
    )
    pipeline = effects_pipeline(fake_bot)
    ids, retired = pipeline._record(
        [planned_for("move", ["HStar"], run, NOW + timedelta(days=4))],
        str(WATCHED_CHANNEL),
        kl(2026, 8, 27),
        "",
    )
    assert [a["id"] for a in retired] == [stale]
    assert repo.get_amendment(stale)["status"] == "superseded"
    assert repo.get_amendment(ids[0])["status"] == "proposed"


def test_a_draft_read_elsewhere_leaves_a_partys_own_card_alone(fake_bot):
    """Retiring is channel-scoped: `_record` says where its rows are going.

    The run is homed in one party's channel and has a live card sitting there.
    A burst read somewhere else proposes about the same run -- and must not
    reach into that channel and retire what the party was about to press, in a
    channel that saw neither the request nor the replacement.
    """
    from .fake_bot import OTHER_CHANNEL, WATCHED_CHANNEL

    repo = fake_bot.repo
    run = seeded_run(repo, OTHER_CHANNEL)
    theirs = repo.create_amendment(
        week_start=kl(2026, 8, 27), kind="move", run_id=run["id"], channel_id=str(OTHER_CHANNEL)
    )
    pipeline = effects_pipeline(fake_bot)
    ids, retired = pipeline._record(
        [planned_for("move", ["HStar"], run, NOW + timedelta(days=4))],
        str(WATCHED_CHANNEL),
        kl(2026, 8, 27),
        "",
    )
    assert retired == []
    assert repo.get_amendment(theirs)["status"] == "proposed"
    assert repo.get_amendment(ids[0])["status"] == "proposed"


def test_each_row_carries_its_own_bursts_summary(fake_bot):
    """A week is several conversations; one summary describes one of them."""
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    run = seeded_run(repo, WATCHED_CHANNEL)
    first = planned_for("move", ["HStar"], run, NOW + timedelta(days=4))
    first.summary = "Sunday: HStar moved to Wed"
    second = planned_for("add", ["NStar"], None, NOW + timedelta(days=5))
    second.summary = "Monday: NStar proposed for Thu"
    pipeline = effects_pipeline(fake_bot)
    ids, _retired = pipeline._record(
        [first, second], str(WATCHED_CHANNEL), kl(2026, 8, 27), "the first burst only"
    )
    assert [repo.get_amendment(i)["summary"] for i in ids] == [
        "Sunday: HStar moved to Wed",
        "Monday: NStar proposed for Thu",
    ]


def test_a_row_with_no_summary_of_its_own_falls_back(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    pipeline = effects_pipeline(fake_bot)
    ids, _retired = pipeline._record(
        [planned_for("add", ["NStar"], None, NOW + timedelta(days=5))],
        str(WATCHED_CHANNEL),
        kl(2026, 8, 27),
        "whatever the window said",
    )
    assert repo.get_amendment(ids[0])["summary"] == "whatever the window said"


# --- rows stranded by a failed post -----------------------------------------


def test_rows_left_without_a_card_get_one_on_the_next_pass(fake_bot):
    """A DNS failure mid-post left `proposed` rows nobody could see or answer."""
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    stranded = repo.create_amendment(
        week_start=kl(2026, 8, 27),
        kind="add",
        bosses=["NStar"],
        channel_id=str(WATCHED_CHANNEL),
        confidence=0.9,
    )
    assert repo.get_amendment(stranded)["proposal_message_id"] is None
    pipeline = effects_pipeline(fake_bot)
    asyncio.run(pipeline.apply_plan(str(WATCHED_CHANNEL), [], [], kl(2026, 8, 27), ""))
    assert repo.get_amendment(stranded)["proposal_message_id"] is not None
    assert len(fake_bot.posts) == 1


def test_a_row_that_already_has_a_card_is_not_posted_again(fake_bot):
    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    posted = repo.create_amendment(
        week_start=kl(2026, 8, 27),
        kind="add",
        bosses=["NStar"],
        channel_id=str(WATCHED_CHANNEL),
        confidence=0.9,
    )
    repo.set_amendment_proposal_message(posted, 900000000000000009)
    pipeline = effects_pipeline(fake_bot)
    asyncio.run(pipeline.apply_plan(str(WATCHED_CHANNEL), [], [], kl(2026, 8, 27), ""))
    assert fake_bot.posts == []


def test_a_card_that_cannot_be_posted_is_reported(fake_bot):
    from bot.extract.pipeline import RescanReport

    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    run = seeded_run(repo, WATCHED_CHANNEL)

    async def no_post(channel, card, mention_users=None):
        return None

    fake_bot._post = no_post
    pipeline = effects_pipeline(fake_bot)
    report = RescanReport(channel_id=str(WATCHED_CHANNEL), window="week", since=NOW)
    ids = asyncio.run(
        pipeline.apply_plan(
            str(WATCHED_CHANNEL),
            [],
            [planned_for("move", ["HStar"], run, NOW + timedelta(days=4))],
            kl(2026, 8, 27),
            "",
            report=report,
        )
    )
    # The rows are written either way; what changes is that somebody is told.
    assert len(ids) == 1
    assert report.unposted == 1
    assert report.errors
    assert repo.get_amendment(ids[0])["proposal_message_id"] is None


# --- the prompt budget, which is what actually bounds a burst ----------------


def test_the_estimator_charges_a_snowflake_far_more_than_its_length():
    """An 18-digit id is ~8 tokens, not the ~4 a characters/token ratio implies."""
    from bot.extract.prompt import estimate_tokens

    bare = estimate_tokens("hello there friend" * 4)
    with_ids = estimate_tokens("hello there friend" * 4 + " <@100000000000000001>")
    assert with_ids - bare >= 8


def test_the_estimator_leaves_short_numbers_alone():
    """ "930" is a time and "290" a level; neither is an id."""
    from bot.extract.prompt import estimate_tokens

    assert estimate_tokens("930 can postpone to 11") < 15


def test_the_budget_keeps_room_for_the_answer():
    from bot.extract.prompt import CONTEXT_RESERVE, prompt_budget

    assert prompt_budget(8192) == 8192 - CONTEXT_RESERVE


def test_split_until_leaves_a_burst_that_fits_alone():
    from bot.extract.window import split_until

    rows = [row(0), row(5), row(10)]
    assert split_until(rows, lambda chunk: True) == [rows]


def test_split_until_halves_at_the_longest_pause():
    from bot.extract.window import split_until

    rows = [row(0), row(2), row(240), row(242)]
    assert [len(g) for g in split_until(rows, lambda chunk: len(chunk) <= 2)] == [2, 2]


def test_split_until_gives_up_on_a_single_message():
    """One message that still does not fit is sent as it is, not dropped."""
    from bot.extract.window import split_until

    assert split_until([row(0)], lambda chunk: False) == [[row(0)]]


def test_a_burst_too_big_for_the_context_window_is_read_in_pieces(fake_bot):
    """The 21-message morning that came back as truncated JSON."""
    from bot.extract.pipeline import Pipeline

    from .fake_bot import WATCHED_CHANNEL

    repo = fake_bot.repo
    seed_chat(
        repo,
        WATCHED_CHANNEL,
        [kl(2026, 8, 30, 9, 0) + timedelta(minutes=7 * i) for i in range(12)],
    )
    pipeline = Pipeline.__new__(Pipeline)
    pipeline.bot = fake_bot
    pipeline._bursts = {}
    rows = repo.recent_messages(WATCHED_CHANNEL, kl(2026, 8, 30, 0, 0))

    assert len(pipeline.fit_to_budget(str(WATCHED_CHANNEL), rows)) == 1
    fake_bot.settings = type(fake_bot.settings)(
        _env_file=None, **{**fake_bot.settings.model_dump(), "ollama_num_ctx": 2560}
    )
    chunks = pipeline.fit_to_budget(str(WATCHED_CHANNEL), rows)
    assert len(chunks) > 1
    assert sum(len(c) for c in chunks) == len(rows)
    assert [r["id"] for c in chunks for r in c] == [r["id"] for r in rows]
