"""Serialized reminder scenarios; expected results come only from the v4 oracle."""

from __future__ import annotations

from typing import Any

KL = "Asia/Kuala_Lumpur"
NY = "America/New_York"
KL_WEEK = "2026-08-27T00:00:00+08:00"
BOSSES = ["HMaleficStar", "HFA"]


def _ids(prefix: int, count: int = 40) -> list[str]:
    return [f"00000000-0000-4000-8{prefix:03d}-{n:012d}" for n in range(1, count + 1)]


def _case(case_id: str, timezone: str, clock: str, steps: list[dict[str, Any]]) -> dict:
    return {
        "case_id": case_id,
        "input": {"clock": clock, "timezone": timezone, "uuid_sequence": _ids(1), "steps": steps},
    }


def _run(key: str, at: str, status: str = "planned", week: str = KL_WEEK) -> dict[str, Any]:
    return {
        "op": "create_run",
        "run_key": key,
        "week_start": week,
        "bosses": BOSSES,
        "at": at,
        "participants": ["1", "2"],
        "status": status,
        "source": "amend",
        "channel_id": "900",
    }


def _fixed(key: str, weekday: int, hhmm: str) -> dict[str, Any]:
    return {
        "op": "add_fixed",
        "fixed_key": key,
        "owner_id": "42",
        "bosses": BOSSES,
        "weekday": weekday,
        "time": hhmm,
        "participants": ["1", "2"],
        "channel_id": "900",
    }


def _specs(at: str, status: str, ping: str, countdowns: list[int]) -> dict[str, Any]:
    return {
        "op": "reminder_specs",
        "at": at,
        "status": status,
        "ping_time": ping,
        "countdowns": countdowns,
    }


def _ensure(key: str, countdowns: list[int], rebuild: bool, ping: str = "09:00:00") -> dict:
    return {
        "op": "ensure_reminders",
        "run_key": key,
        "ping_time": ping,
        "countdowns": countdowns,
        "rebuild": rebuild,
    }


def _stale(kind: str, fire_at: str, now: str) -> dict[str, Any]:
    return {"op": "is_stale", "kind": kind, "fire_at": fire_at, "now": now}


def _list(key: str) -> dict[str, Any]:
    return {"op": "list_reminders", "run_key": key}


def _clock(value: str) -> dict[str, Any]:
    return {"op": "set_clock", "clock": value}


PROVENANCE = {
    "oracle": "legacy/python bot.agent.materialise, bot.infrastructure.db.Repo reminder methods",
    "functions": [
        "bot.agent.materialise.reminder_specs",
        "bot.agent.materialise.ensure_reminders",
        "bot.agent.materialise.materialise_week",
        "bot.agent.materialise.reconcile_day_of",
        "bot.agent.materialise.mark_done",
        "bot.agent.materialise.is_stale",
        "bot.infrastructure.db.Repo.add_reminder",
        "bot.infrastructure.db.Repo.mark_reminder_sent",
        "bot.infrastructure.db.Repo.reschedule_unposted_reminder",
    ],
    "source_tests": [
        "tests/test_materialise.py::test_reminder_times_are_computed_in_guild_local_time",
        "tests/test_materialise.py::test_a_run_just_after_midnight_is_pinged_the_morning_before",
        "tests/test_materialise.py::test_otot_keeps_only_the_day_of_ping",
        "tests/test_materialise.py::test_cancelled_and_done_runs_get_nothing",
        "tests/test_materialise.py::test_reminders_are_created_once_per_run_and_kind",
        "tests/test_materialise.py::test_reminders_already_in_the_past_are_marked_sent",
        "tests/test_materialise.py::test_going_otot_drops_the_countdowns_but_keeps_the_morning_ping",
        "tests/test_materialise.py::test_cancelling_after_the_morning_ping_clears_that_row_too",
        "tests/test_materialise.py::test_changing_the_countdown_offsets_prunes_the_stale_ones",
        "tests/test_materialise.py::test_moving_a_run_later_in_the_week_gets_a_fresh_day_of_ping",
        "tests/test_qol.py::test_is_stale_grace_per_kind",
        "tests/test_qol.py::test_day_of_at_0100_stays_on_the_run_day",
        "tests/test_qol.py::test_reconcile_moves_unsent_day_of_to_new_ping_time",
        "tests/test_qol.py::test_reconcile_reopens_a_skipped_day_of_when_its_new_time_is_future",
        "tests/test_qol.py::test_reconcile_skips_a_queued_day_of_moved_into_the_past",
        "tests/test_qol.py::test_reconcile_keeps_a_posted_day_of_and_its_message_mapping",
        "tests/test_past_runs.py::test_marking_done_removes_the_pings_that_never_fired",
        "tests/test_past_runs.py::test_a_sent_reminder_is_kept_as_the_record",
        "tests/test_weeks.py::test_week_end_is_exactly_seven_days_on_a_dst_free_zone",
    ],
    "inventory_surfaces": ["reminder rows", "day-of reconciliation", "stale reminder grace"],
}


def documents() -> dict[str, dict[str, Any]]:
    return {
        "reminders.json": {
            "schema_version": "v5-scheduler-reminders-v1",
            "family": "reminders",
            "provenance": PROVENANCE,
            "cases": [
                _case(
                    "specs-status-countdown-and-late-night-policy",
                    KL,
                    "2026-08-27T01:00:00+08:00",
                    [
                        _specs(
                            "2026-08-31T21:30:00+08:00", "planned", "09:00:00", [15, 60, 15, 30]
                        ),
                        _specs("2026-08-31T21:30:00+08:00", "confirmed", "09:00:00", [60]),
                        _specs("2026-08-31T21:30:00+08:00", "at_risk", "09:00:00", [60]),
                        _specs("2026-08-31T21:30:00+08:00", "otot", "09:00:00", [60, 15]),
                        _specs("2026-08-31T21:30:00+08:00", "cancelled", "09:00:00", [60]),
                        _specs("2026-08-31T21:30:00+08:00", "done", "09:00:00", [60]),
                        _specs("2026-08-31T21:30:00+08:00", "planned", "09:00:00", []),
                        _specs("2026-08-31T21:30:00+08:00", "planned", "09:00:00", [0]),
                        _specs("2026-09-01T00:30:00+08:00", "planned", "09:00:00", [60]),
                        _specs("2026-08-31T09:00:00+08:00", "planned", "09:00:00", [60]),
                        _specs("2026-08-31T09:01:00+08:00", "planned", "09:00:00", [60]),
                        _specs("2026-08-31T21:30:00+08:00", "planned", "01:00:00", [60]),
                        _specs("2026-08-31T13:30:00+00:00", "planned", "09:00:00", [60]),
                    ],
                ),
                _case(
                    "ensure-writes-past-slots-sent-and-prunes-unsent",
                    KL,
                    "2026-08-31T21:00:00+08:00",
                    [
                        _run("tonight", "2026-08-31T21:30:00+08:00"),
                        _ensure("tonight", [60, 15], False),
                        _ensure("tonight", [60, 15], False),
                        _ensure("tonight", [30], False),
                        _run("boundary", "2026-08-31T22:00:00+08:00"),
                        _ensure("boundary", [60], False),
                        _clock("2026-08-31T21:00:01+08:00"),
                        _run("later", "2026-08-31T23:00:00+08:00"),
                        _ensure("later", [120, 60], False),
                    ],
                ),
                _case(
                    "rebuild-deletes-sent-rows-and-duplicate-add-is-ignored",
                    KL,
                    "2026-08-31T10:00:00+08:00",
                    [
                        _run("open", "2026-08-31T21:30:00+08:00"),
                        _ensure("open", [60, 15], False),
                        {
                            "op": "add_reminder",
                            "run_key": "open",
                            "kind": "day_of",
                            "fire_at": "2026-08-31T08:00:00+08:00",
                            "sent_at": None,
                        },
                        {
                            "op": "add_reminder",
                            "run_key": "open",
                            "kind": "countdown_5",
                            "fire_at": "2026-08-31T21:25:00+08:00",
                            "sent_at": None,
                        },
                        {
                            "op": "mark_reminder_sent",
                            "run_key": "open",
                            "kind": "day_of",
                            "message_id": "4242",
                        },
                        _ensure("open", [60, 15], False),
                        _list("open"),
                        {"op": "set_status", "run_key": "open", "status": "otot"},
                        _ensure("open", [60, 15], True),
                        _list("open"),
                        {"op": "set_status", "run_key": "open", "status": "planned"},
                        _ensure("open", [60, 15], True),
                        _list("open"),
                        {"op": "set_status", "run_key": "open", "status": "cancelled"},
                        _ensure("open", [60, 15], True),
                        _list("open"),
                    ],
                ),
                _case(
                    "reconcile-day-of-and-reschedule-unposted",
                    KL,
                    "2026-09-01T09:30:00+08:00",
                    [
                        _run("skipped", "2026-09-01T21:30:00+08:00"),
                        _ensure("skipped", [60], False),
                        _run("queued", "2026-09-02T21:30:00+08:00"),
                        _ensure("queued", [60], False),
                        _run("posted", "2026-09-03T21:30:00+08:00"),
                        _ensure("posted", [60], False),
                        {
                            "op": "mark_reminder_sent",
                            "run_key": "posted",
                            "kind": "day_of",
                            "message_id": "4343",
                        },
                        _run("off", "2026-09-02T22:30:00+08:00"),
                        _ensure("off", [60], False),
                        {"op": "set_status", "run_key": "off", "status": "cancelled"},
                        {"op": "reconcile_day_of", "ping_time": "10:00:00"},
                        {"op": "reconcile_day_of", "ping_time": "10:00:00"},
                        {"op": "reconcile_day_of", "ping_time": "08:00:00"},
                        {"op": "reconcile_day_of", "ping_time": "08:00:00"},
                        {
                            "op": "reschedule_unposted_reminder",
                            "run_key": "queued",
                            "kind": "countdown_60",
                            "fire_at": "2026-09-02T20:00:00+08:00",
                        },
                        {
                            "op": "reschedule_unposted_reminder",
                            "run_key": "queued",
                            "kind": "countdown_60",
                            "fire_at": "2026-09-02T20:00:00+08:00",
                        },
                        {
                            "op": "reschedule_unposted_reminder",
                            "run_key": "queued",
                            "kind": "countdown_60",
                            "fire_at": "2026-09-01T09:30:00+08:00",
                        },
                        {
                            "op": "reschedule_unposted_reminder",
                            "run_key": "posted",
                            "kind": "day_of",
                            "fire_at": "2026-09-03T11:00:00+08:00",
                        },
                    ],
                ),
                _case(
                    "mark-done-strictly-after-two-hours",
                    KL,
                    "2026-08-31T12:00:00+08:00",
                    [
                        _run("planned", "2026-08-31T21:30:00+08:00"),
                        _ensure("planned", [60, 15], False),
                        _run("otot", "2026-08-31T21:30:00+08:00", "otot"),
                        _ensure("otot", [60, 15], False),
                        _run("cancelled", "2026-08-31T21:30:00+08:00", "cancelled"),
                        _run("later", "2026-08-31T21:30:01+08:00", "confirmed"),
                        _ensure("later", [60, 15], False),
                        _clock("2026-08-31T23:30:00+08:00"),
                        {"op": "mark_done"},
                        _clock("2026-08-31T23:30:01+08:00"),
                        {"op": "mark_done"},
                        {"op": "mark_done"},
                    ],
                ),
                _case(
                    "is-stale-strict-grace-boundaries",
                    KL,
                    "2026-08-31T12:00:00+08:00",
                    [
                        _stale("day_of", "2026-08-31T00:00:00+08:00", "2026-08-31T12:00:00+08:00"),
                        _stale("day_of", "2026-08-31T00:00:00+08:00", "2026-08-31T12:00:01+08:00"),
                        _stale(
                            "countdown_60", "2026-08-31T12:00:00+08:00", "2026-08-31T12:30:00+08:00"
                        ),
                        _stale(
                            "countdown_60", "2026-08-31T12:00:00+08:00", "2026-08-31T12:30:01+08:00"
                        ),
                        _stale(
                            "countdown_15", "2026-08-31T04:00:00+00:00", "2026-08-31T12:30:00+08:00"
                        ),
                        _stale("custom", "2026-08-31T12:00:00+08:00", "2026-08-31T12:30:01+08:00"),
                        _stale("day_of", "2026-08-31T13:00:00+08:00", "2026-08-31T12:00:00+08:00"),
                    ],
                ),
                _case(
                    "dst-spring-forward-gap-ping-and-fixed-slots",
                    NY,
                    "2026-03-05T01:00:00-05:00",
                    [
                        _specs("2026-03-08T03:00:00-04:00", "planned", "02:30:00", [60, 15]),
                        _specs("2026-03-08T01:30:00-05:00", "planned", "02:30:00", [60]),
                        _specs("2026-03-08T12:00:00-04:00", "planned", "02:30:00", [60]),
                        _specs("2026-03-09T21:30:00-04:00", "planned", "09:00:00", [60]),
                        _fixed("gap", 6, "02:30"),
                        _fixed("after", 6, "03:00"),
                        {
                            "op": "materialise",
                            "week_start": "2026-03-05T00:00:00-05:00",
                            "ping_time": "02:30:00",
                            "countdowns": [60, 15],
                        },
                    ],
                ),
                _case(
                    "dst-fall-back-overlap-ping-and-fixed-slots",
                    NY,
                    "2026-10-29T01:00:00-04:00",
                    [
                        _specs("2026-11-01T01:30:00-05:00", "planned", "01:45:00", [60]),
                        _specs("2026-11-01T01:15:00-04:00", "planned", "01:30:00", [60]),
                        _specs("2026-11-01T01:50:00-05:00", "planned", "01:45:00", [60]),
                        _specs("2026-11-01T21:30:00-05:00", "planned", "09:00:00", [60]),
                        _fixed("overlap", 6, "01:30"),
                        {
                            "op": "materialise",
                            "week_start": "2026-10-29T00:00:00-04:00",
                            "ping_time": "01:45:00",
                            "countdowns": [60, 15],
                        },
                    ],
                ),
                _case(
                    "negative-unknown-run-status-rejected-by-repo",
                    KL,
                    "2026-08-31T10:00:00+08:00",
                    [
                        _run("open", "2026-08-31T21:30:00+08:00"),
                        {
                            "op": "set_status",
                            "run_key": "open",
                            "status": "archived",
                            "error_type": "ValueError",
                        },
                    ],
                ),
            ],
        }
    }
