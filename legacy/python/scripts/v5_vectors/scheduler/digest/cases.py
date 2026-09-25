"""Serialized digest-schedule scenarios; expected results come only from the v4 oracle."""

from __future__ import annotations

from typing import Any

from ..dispatch.cases import MEMBERS
from ..mutations.cases import CATALOG

SUNDAY = "2026-08-30T12:00:00+08:00"
RESET = "2026-09-03T00:01:00+08:00"
LATER = "2026-09-06T10:00:00+08:00"
OLD_WEEK = "2026-08-27T00:00:00+08:00"
NEW_WEEK = "2026-09-03T00:00:00+08:00"
HOME, POST = "222", "555"
POST_DIGEST = {"op": "post_week_digest"}


def _clock(value: str) -> dict[str, Any]:
    return {"op": "set_clock", "clock": value}


def _case(
    case_id: str, steps: list[dict[str, Any]], *, post_channel_id: str | None = POST
) -> dict[str, Any]:
    return {
        "case_id": case_id,
        "input": {
            "clock": SUNDAY,
            "timezone": "Asia/Kuala_Lumpur",
            "reset_weekday": 3,
            "reset_time": "00:00:00",
            "ping_time": "09:00:00",
            "countdowns": [60, 15],
            "guild_id": "111",
            "post_channel_id": post_channel_id,
            "available_channel_ids": [HOME, POST],
            "catalog": CATALOG,
            "members": MEMBERS,
            "uuid_sequence": [f"{n:08x}-0000-4000-8005-000000000000" for n in range(1, 120)],
            "steps": steps,
        },
    }


def _fixed(key: str, weekday: int, hhmm: str, bosses: list[str]) -> dict[str, Any]:
    return {
        "op": "add_fixed",
        "fixed_key": key,
        "owner_id": "1001",
        "bosses": bosses,
        "weekday": weekday,
        "time": hhmm,
        "participants": ["1001", "1002"],
        "channel_id": HOME,
    }


def _status(key: str, status: str) -> dict[str, Any]:
    return {"op": "set_status", "run_key": key, "status": status}


PROVENANCE = {
    "oracle": "legacy/python bot.agent.client.BossBot digest schedule over a real Repo journal",
    "functions": [
        "bot.agent.client.BossBot.post_week_digest",
        "bot.agent.client.BossBot._post_week_digest",
        "bot.agent.client.BossBot._post_digest",
        "bot.agent.client.BossBot.materialise_weeks",
        "bot.agent.formatting.digest_card",
        "bot.agent.effects.send_card",
        "bot.infrastructure.db.Repo.retire_weekly_digests_before",
        "bot.infrastructure.db.Repo.set_weekly_digest",
    ],
    "source_tests": [
        "tests/test_digest.py::test_the_first_tick_on_a_new_database_starts_the_clock_rather_than_posting",
        "tests/test_digest.py::test_the_reset_posts_the_digest",
        "tests/test_digest.py::test_the_rest_of_the_week_posts_nothing_more",
        "tests/test_digest.py::test_a_host_that_slept_through_the_reset_posts_exactly_one_digest",
        "tests/test_digest.py::test_a_restart_between_the_reset_and_the_first_tick_still_posts",
        "tests/test_digest.py::test_an_ambiguous_digest_send_is_not_retried_on_the_next_tick",
        "tests/test_digest.py::test_nothing_is_posted_when_there_is_no_digest_channel",
        "tests/test_digest.py::test_setting_the_channel_later_does_not_back_post_a_half_finished_week",
        "tests/test_digest.py::test_the_previous_digest_is_retired_but_kept_as_a_weekly_log",
        "tests/test_digest.py::test_a_cancelled_run_is_left_out",
        "tests/test_digest.py::test_the_summary_counts_the_at_risk_runs_separately",
        "tests/test_digest.py::test_the_digest_names_people_rather_than_mentioning_them",
    ],
    "inventory_surfaces": ["weekly digest schedule", "delivery journal intents"],
}


def documents() -> dict[str, dict[str, Any]]:
    return {
        "digest.json": {
            "schema_version": "v5-scheduler-digest-v1",
            "family": "digest",
            "provenance": PROVENANCE,
            "cases": [
                _case("first-tick-only-records-the-week", [POST_DIGEST, POST_DIGEST]),
                _case(
                    "reset-posts-once-then-nothing-all-week",
                    [
                        POST_DIGEST,
                        _clock(RESET),
                        POST_DIGEST,
                        _clock("2026-09-03T00:02:00+08:00"),
                        POST_DIGEST,
                        _clock("2026-09-04T09:00:00+08:00"),
                        POST_DIGEST,
                        _clock(LATER),
                        POST_DIGEST,
                    ],
                ),
                _case(
                    "slept-through-the-reset-posts-exactly-one",
                    [POST_DIGEST, _clock(LATER), POST_DIGEST, POST_DIGEST],
                ),
                _case(
                    "restart-before-first-tick-still-posts",
                    [
                        POST_DIGEST,
                        {
                            "op": "set_config",
                            "key": "last_materialised_week",
                            "value": "whatever on_ready wrote",
                        },
                        {"op": "materialise_weeks"},
                        _clock(RESET),
                        POST_DIGEST,
                    ],
                ),
                _case(
                    "uncertain-send-is-not-retried",
                    [
                        POST_DIGEST,
                        {"op": "set_transport", "fail_sends": True},
                        _clock(RESET),
                        POST_DIGEST,
                        {"op": "set_transport", "fail_sends": False},
                        _clock("2026-09-03T00:02:00+08:00"),
                        POST_DIGEST,
                        _clock(LATER),
                        POST_DIGEST,
                    ],
                ),
                _case(
                    "no-digest-channel-still-records-each-week",
                    [POST_DIGEST, _clock(RESET), POST_DIGEST, _clock(LATER), POST_DIGEST],
                    post_channel_id=None,
                ),
                _case(
                    "channel-set-later-does-not-back-post",
                    [
                        POST_DIGEST,
                        _clock(RESET),
                        POST_DIGEST,
                        {"op": "set_post_channel", "channel_id": POST},
                        _clock("2026-09-05T12:00:00+08:00"),
                        POST_DIGEST,
                        _clock("2026-09-10T00:01:00+08:00"),
                        POST_DIGEST,
                    ],
                    post_channel_id=None,
                ),
                _case(
                    "previous-digest-retired-but-kept-as-log",
                    [
                        {
                            "op": "set_weekly_digest",
                            "week_start": OLD_WEEK,
                            "channel_id": POST,
                            "message_id": "7001",
                        },
                        {"op": "set_config", "key": "last_digest_week", "value": OLD_WEEK},
                        _clock(RESET),
                        POST_DIGEST,
                        {"op": "retire_weekly_digests_before", "week_start": NEW_WEEK},
                        {
                            "op": "retire_weekly_digests_before",
                            "week_start": "2026-09-10T00:00:00+08:00",
                        },
                    ],
                ),
                _case(
                    "inclusion-excludes-cancelled-and-counts-at-risk",
                    [
                        _fixed("star", 0, "21:30", ["HMaleficStar", "HFA"]),
                        _fixed("kalos", 1, "23:00", ["XKalos"]),
                        _fixed("fa", 0, "20:00", ["NFA"]),
                        _fixed("cleared", 4, "21:00", ["HFA"]),
                        {"op": "materialise_weeks"},
                        {
                            "op": "create_run",
                            "run_key": "one-off",
                            "week_start": NEW_WEEK,
                            "bosses": ["NKalos"],
                            "at": "2026-09-05T22:00:00+08:00",
                            "participants": ["1003"],
                            "status": "planned",
                            "source": "amend",
                            "channel_id": HOME,
                        },
                        _status(f"star@{NEW_WEEK}", "at_risk"),
                        _status(f"kalos@{NEW_WEEK}", "cancelled"),
                        _status(f"fa@{NEW_WEEK}", "confirmed"),
                        _status(f"cleared@{NEW_WEEK}", "done"),
                        POST_DIGEST,
                        _clock(RESET),
                        POST_DIGEST,
                    ],
                ),
            ],
        }
    }
