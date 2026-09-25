"""Serialized API-service mutation scenarios; expected results come only from the v4 oracle."""

from __future__ import annotations

from typing import Any

WEEK1 = "2026-08-27T00:00:00+08:00"
WEEK2 = "2026-09-03T00:00:00+08:00"
WEEK3 = "2026-09-10T00:00:00+08:00"
HOME, OTHER, UNWATCHED = "222", "333", "444"

CATALOG = {
    "difficulties": [
        {"prefix": "n", "label": "Normal"},
        {"prefix": "h", "label": "Hard"},
        {"prefix": "x", "label": "Extreme"},
    ],
    "bosses": [
        {
            "short": "MaleficStar",
            "full": "Malefic Star",
            "difficulties": ["n", "h"],
            "aliases": ["star", "mstar"],
        },
        {"short": "FA", "full": "First Adversary", "aliases": ["fa"]},
        {"short": "Kalos", "full": "Kalos", "difficulties": ["n", "x"], "aliases": ["kalos"]},
    ],
}

MEMBERS = [
    {
        "user_id": "1001",
        "display_name": "Alvin",
        "nickname": None,
        "has_role": True,
        "ping_level": "all",
    },
    {
        "user_id": "1002",
        "display_name": "kanon [AZUR]",
        "nickname": "kanon",
        "has_role": True,
        "ping_level": "essential",
    },
    {
        "user_id": "1003",
        "display_name": "Priya",
        "nickname": None,
        "has_role": True,
        "ping_level": "off",
    },
    {
        "user_id": "1009",
        "display_name": "Outsider",
        "nickname": None,
        "has_role": False,
        "ping_level": "essential",
    },
]


def _case(case_id: str, steps: list[dict[str, Any]], clock: str = "2026-08-27T01:00:00+08:00"):
    return {
        "case_id": case_id,
        "input": {
            "clock": clock,
            "timezone": "Asia/Kuala_Lumpur",
            "reset_weekday": 3,
            "reset_time": "00:00:00",
            "ping_time": "09:00:00",
            "countdowns": [60, 15],
            "watched_channel_ids": [HOME, OTHER],
            "portal_actor_id": "999",
            "catalog": CATALOG,
            "members": MEMBERS,
            "uuid_sequence": [f"00000000-0000-4000-8002-{n:012d}" for n in range(1, 90)],
            "steps": steps,
        },
    }


WEEKLY = {
    "op": "add_fixed",
    "fixed_key": "weekly",
    "owner_id": "1001",
    "bosses": ["HMaleficStar", "HFA"],
    "weekday": 0,
    "time": "21:30",
    "participants": ["1001", "1002"],
    "channel_id": HOME,
}
MATERIALISE = {"op": "materialise_weeks"}
RUN1 = f"weekly@{WEEK1}"
RUN2 = f"weekly@{WEEK2}"
RUN3 = f"weekly@{WEEK3}"


def _rsvp(key: str, user: str, state: str) -> dict[str, Any]:
    return {"op": "set_rsvp", "run_key": key, "user_id": user, "state": state, "source": "chat"}


def _status(key: str, status: str, *, announce: bool = True, mark: bool = True, error=None):
    step = {"op": "set_status", "run_key": key, "status": status, "announce": announce}
    step["mark"] = mark
    if error:
        step["error_type"] = error
    return step


def _amend(key: str, to: str, error: str | None = None) -> dict[str, Any]:
    step = {"op": "amend", "run_key": key, "to": to}
    if error:
        step["error_type"] = error
    return step


def _swap(key: str, remove: list[str], add: list[str], error: str | None = None):
    step = {"op": "swap", "run_key": key, "remove": remove, "add": add, "mark": True}
    if error:
        step["error_type"] = error
    return step


def _edit(changes: dict[str, Any], error: str | None = None) -> dict[str, Any]:
    step = {"op": "update_fixed", "fixed_key": "weekly", "changes": changes}
    if error:
        step["error_type"] = error
    return step


PROVENANCE = {
    "oracle": "legacy/python bot.api.service over a real in-memory Repo",
    "functions": [
        "bot.api.service.set_status",
        "bot.api.service.amend_run",
        "bot.api.service.swap_participants",
        "bot.api.service.update_fixed",
        "bot.api.service._apply_fixed_to_runs",
        "bot.api.service._announce",
        "bot.agent.client.BossBot.materialise_weeks",
        "bot.agent.rsvp.recompute_after_roster_change",
        "bot.infrastructure.db.Repo.run_move_conflict",
    ],
    "source_tests": [
        "tests/test_notices.py::test_cancelling_tells_the_home_channel",
        "tests/test_notices.py::test_restoring_says_it_is_back_on",
        "tests/test_notices.py::test_setting_the_status_it_already_has_posts_nothing",
        "tests/test_notices.py::test_editing_a_weekly_timing_is_announced",
        "tests/test_notices.py::test_own_time_keeps_only_the_morning_ping",
        "tests/test_notices.py::test_coming_back_from_cancelled_rebuilds_the_pings",
        "tests/test_notices.py::test_confirming_keeps_the_answers_people_gave",
        "tests/test_notices.py::test_coming_back_from_cancelled_clears_them",
        "tests/test_notices.py::test_at_risk_cannot_be_set_by_hand",
        "tests/test_notices.py::test_an_invented_status_is_refused",
        "tests/test_swap.py::test_the_person_who_left_has_their_answer_cleared",
        "tests/test_swap.py::test_losing_the_person_who_said_no_can_settle_the_run",
        "tests/test_swap.py::test_a_run_cannot_be_emptied",
        "tests/test_swap.py::test_removing_someone_who_is_not_on_the_run_is_refused",
        "tests/test_swap.py::test_someone_without_the_bossing_role_cannot_be_added",
        "tests/test_swap.py::test_a_no_op_swap_changes_nothing_and_says_nothing",
        "tests/test_swap.py::test_the_channel_is_told_who_is_in_and_out",
        "tests/test_api_routes.py::test_amend_moves_the_run_and_announces_it",
        "tests/test_api_routes.py::test_amend_rebuilds_the_runs_reminders",
        "tests/test_api_routes.py::test_amend_into_an_occupied_week_is_refused",
        "tests/test_api_routes.py::test_editing_a_fixed_run_updates_three_materialised_instances",
        "tests/test_api_routes.py::test_an_empty_patch_says_so",
        "tests/test_fixed_edit.py::test_editing_the_channel_moves_where_the_pings_go_but_not_when",
    ],
    "inventory_surfaces": ["run status mutation", "run move", "roster swap", "fixed edit"],
}


def documents() -> dict[str, dict[str, Any]]:
    return {
        "mutations.json": {
            "schema_version": "v5-scheduler-mutations-v1",
            "family": "mutations",
            "provenance": PROVENANCE,
            "cases": [
                _case(
                    "set-status-transitions-restore-clears-rsvps",
                    [
                        WEEKLY,
                        MATERIALISE,
                        _rsvp(RUN1, "1001", "yes"),
                        _rsvp(RUN1, "1002", "maybe"),
                        _rsvp(RUN1, "1003", "no"),
                        _status(RUN1, "confirmed"),
                        _status(RUN1, "confirmed"),
                        _status(RUN1, "otot", mark=False),
                        _status(RUN1, "planned"),
                        _rsvp(RUN1, "1001", "yes"),
                        _status(RUN1, "cancelled", announce=False),
                        _status(RUN1, "planned"),
                        _rsvp(RUN1, "1002", "yes"),
                        _status(RUN1, "done"),
                        _status(RUN1, "planned"),
                        _status(RUN1, "at_risk", error="BadRequest"),
                        _status(RUN1, "archived", error="BadRequest"),
                    ],
                ),
                _case(
                    "amend-resets-answers-status-and-refuses-occupied-week",
                    [
                        WEEKLY,
                        MATERIALISE,
                        _rsvp(RUN1, "1001", "yes"),
                        _rsvp(RUN1, "1002", "yes"),
                        _status(RUN1, "confirmed", announce=False),
                        _amend(RUN1, "2026-09-02 21:45"),
                        _amend(RUN1, "2026-09-08 22:30", error="BadRequest"),
                        _amend(RUN1, "sometime soon-ish", error="BadRequest"),
                        {
                            "op": "create_run",
                            "run_key": "one-off",
                            "week_start": WEEK1,
                            "bosses": ["XKalos"],
                            "at": "2026-08-29T22:00:00+08:00",
                            "participants": ["1002", "1003"],
                            "status": "at_risk",
                            "source": "amend",
                            "channel_id": OTHER,
                        },
                        _amend("one-off", "2026-09-05 22:00"),
                        _status(RUN2, "otot", announce=False),
                        _amend(RUN2, "2026-09-08 20:00"),
                    ],
                ),
                _case(
                    "swap-errors-and-rsvp-recompute",
                    [
                        WEEKLY,
                        MATERIALISE,
                        _rsvp(RUN1, "1001", "yes"),
                        _rsvp(RUN1, "1002", "no"),
                        _swap(RUN1, ["1002"], []),
                        _swap(RUN1, [], ["1003"]),
                        _swap(RUN1, [], ["1001"]),
                        _swap(RUN1, ["1002"], [], error="BadRequest"),
                        _swap(RUN1, [], ["1009"], error="BadRequest"),
                        _swap(RUN1, [], ["abc"], error="BadRequest"),
                        _swap(RUN1, ["1001", "1003"], [], error="BadRequest"),
                        _swap(RUN1, ["1003"], ["1002"]),
                    ],
                ),
                _case(
                    "update-fixed-pushes-only-touched-fields-to-live-runs",
                    [
                        WEEKLY,
                        MATERIALISE,
                        _status(RUN2, "cancelled", announce=False),
                        _rsvp(RUN1, "1002", "yes"),
                        _amend(RUN3, "2026-09-16 21:30"),
                        _edit({"bosses": "xkalos"}),
                        _edit({"participants": ["1001", "1003"]}),
                        _edit({"channel_id": OTHER}),
                        _edit({"note": "ring fee split"}),
                        _edit({"day": "wed", "time": "22:15"}),
                        _edit({"channel_id": UNWATCHED}, error="BadRequest"),
                        _edit({"bosses": "hkalos"}, error="BadRequest"),
                        _edit({"participants": ["1009"]}, error="BadRequest"),
                        _edit({}, error="BadRequest"),
                    ],
                ),
            ],
        }
    }
