"""Serialized dispatch scenarios; expected results come only from the v4 oracle."""

from __future__ import annotations

from typing import Any

from ..mutations.cases import CATALOG

WEEK = "2026-08-27T00:00:00+08:00"
NOW = "2026-08-31T09:00:30+08:00"
HOME_A, HOME_B, GONE, POST = "222", "333", "444", "555"

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
]


def _case(
    case_id: str,
    steps: list[dict[str, Any]],
    *,
    post_channel_id: str | None = POST,
    available: tuple[str, ...] = (HOME_A, HOME_B, POST),
) -> dict[str, Any]:
    return {
        "case_id": case_id,
        "input": {
            "clock": NOW,
            "timezone": "Asia/Kuala_Lumpur",
            "reset_weekday": 3,
            "reset_time": "00:00:00",
            "ping_time": "09:00:00",
            "countdowns": [60, 15],
            "guild_id": "111",
            "post_channel_id": post_channel_id,
            "available_channel_ids": list(available),
            "catalog": CATALOG,
            "members": MEMBERS,
            "uuid_sequence": [f"{n:08x}-0000-4000-8004-000000000000" for n in range(1, 60)],
            "steps": steps,
        },
    }


def _run(key: str, at: str, people: list[str], channel: str | None, status: str = "planned"):
    return {
        "op": "create_run",
        "run_key": key,
        "week_start": WEEK,
        "bosses": ["HMaleficStar", "HFA"],
        "at": at,
        "participants": people,
        "status": status,
        "source": "amend",
        "channel_id": channel,
    }


def _remind(key: str, kind: str, fire_at: str) -> dict[str, Any]:
    return {"op": "add_reminder", "run_key": key, "kind": kind, "fire_at": fire_at}


DUE = {"op": "due_reminders"}
DISPATCH = {"op": "dispatch_reminders"}
EVERYONE = ["1001", "1002", "1003"]

PROVENANCE = {
    "oracle": "legacy/python bot.agent.client.BossBot dispatch path over a real Repo journal",
    "functions": [
        "bot.agent.client.BossBot.dispatch_reminders",
        "bot.agent.client.BossBot._send_day_of",
        "bot.agent.client.BossBot._send_countdown",
        "bot.agent.client.BossBot.day_of_card_for",
        "bot.agent.client.BossBot.countdown_card_for",
        "bot.agent.client.BossBot.find_channel",
        "bot.agent.effects.send_card",
        "bot.infrastructure.maintenance.delivery.journal.DeliveryJournal.execute",
        "bot.agent.materialise.is_stale",
        "bot.agent.materialise.mark_done",
        "bot.infrastructure.db.Repo.due_reminders",
    ],
    "source_tests": [
        "tests/test_discord_effects.py::test_reminder_client_callsites_group_and_bind_native_rows",
        "tests/test_materialise.py::test_due_reminders_only_returns_unsent_ones_whose_time_has_come",
        "tests/test_qol.py::test_is_stale_grace_per_kind",
        "tests/test_qol.py::test_a_countdown_goes_to_the_whole_party_bar_the_decliners",
        "tests/test_at_risk.py::test_the_countdown_does_not_ping_the_person_who_declined",
        "tests/test_pings.py::test_a_card_carries_the_allow_list_it_was_built_with",
        "tests/test_past_runs.py::test_marking_done_removes_the_pings_that_never_fired",
        "tests/test_digest.py::test_an_ambiguous_digest_send_is_not_retried_on_the_next_tick",
    ],
    "inventory_surfaces": ["reminder dispatch", "delivery journal intents", "mention policy"],
}


def documents() -> dict[str, dict[str, Any]]:
    return {
        "dispatch.json": {
            "schema_version": "v5-scheduler-dispatch-v1",
            "family": "dispatch",
            "provenance": PROVENANCE,
            "cases": [
                _case(
                    "classify-suppress-group-and-countdown",
                    [
                        _run("late", "2026-08-31T21:30:00+08:00", EVERYONE, HOME_A),
                        _run("early", "2026-08-31T19:00:00+08:00", ["1002"], HOME_A),
                        _run("other", "2026-08-31T22:00:00+08:00", ["1001", "1003"], HOME_B),
                        _run("off", "2026-08-31T20:00:00+08:00", EVERYONE, HOME_A, "cancelled"),
                        _run("tomorrow", "2026-09-01T21:30:00+08:00", EVERYONE, HOME_A),
                        _run("soon", "2026-08-31T09:30:00+08:00", EVERYONE, HOME_B),
                        {
                            "op": "set_rsvp",
                            "run_key": "soon",
                            "user_id": "1002",
                            "state": "no",
                            "source": "reaction",
                        },
                        _remind("late", "day_of", "2026-08-31T09:00:00+08:00"),
                        _remind("early", "day_of", "2026-08-31T09:00:00+08:00"),
                        _remind("other", "day_of", "2026-08-31T09:00:00+08:00"),
                        _remind("off", "day_of", "2026-08-31T09:00:00+08:00"),
                        _remind("soon", "countdown_30", "2026-08-31T08:30:30+08:00"),
                        _remind("soon", "countdown_31", "2026-08-31T08:30:29+08:00"),
                        _remind("soon", "day_of", "2026-08-30T21:00:30+08:00"),
                        _remind("late", "day_off", "2026-08-31T08:59:00+08:00"),
                        _remind("tomorrow", "day_of", "2026-09-01T09:00:00+08:00"),
                        DUE,
                        DISPATCH,
                        DUE,
                        DISPATCH,
                    ],
                ),
                _case(
                    "stale-day-of-boundary-is-strict",
                    [
                        _run("next", "2026-08-31T21:30:00+08:00", ["1002"], HOME_A),
                        _run("later", "2026-08-31T22:30:00+08:00", ["1001"], HOME_A),
                        _remind("next", "day_of", "2026-08-30T21:00:30+08:00"),
                        _remind("later", "day_of", "2026-08-30T21:00:29+08:00"),
                        DISPATCH,
                    ],
                ),
                _case(
                    "unavailable-home-falls-back-then-binding-refuses",
                    [
                        _run("gone", "2026-08-31T21:30:00+08:00", ["1001"], GONE),
                        _run("homeless", "2026-08-31T22:00:00+08:00", ["1002"], None),
                        _remind("gone", "day_of", "2026-08-31T09:00:00+08:00"),
                        _remind("gone", "countdown_60", "2026-08-31T09:00:00+08:00"),
                        _remind("homeless", "day_of", "2026-08-31T09:00:00+08:00"),
                        DISPATCH,
                        DUE,
                        DISPATCH,
                        {"op": "set_clock", "clock": "2026-08-31T21:00:01+08:00"},
                        DISPATCH,
                        DUE,
                    ],
                ),
                _case(
                    "no-channel-at-all-leaves-reminders-queued",
                    [
                        _run("gone", "2026-08-31T21:30:00+08:00", ["1001"], GONE),
                        _remind("gone", "day_of", "2026-08-31T09:00:00+08:00"),
                        _remind("gone", "countdown_60", "2026-08-31T09:00:00+08:00"),
                        DISPATCH,
                        DUE,
                    ],
                    post_channel_id=None,
                ),
                _case(
                    "ambiguous-send-is-never-replayed",
                    [
                        _run("late", "2026-08-31T21:30:00+08:00", ["1001", "1002"], HOME_A),
                        _remind("late", "day_of", "2026-08-31T09:00:00+08:00"),
                        {"op": "set_transport", "fail_sends": True},
                        DISPATCH,
                        {"op": "set_transport", "fail_sends": False},
                        DUE,
                        DISPATCH,
                        {"op": "set_clock", "clock": "2026-08-31T21:00:01+08:00"},
                        DISPATCH,
                        DUE,
                    ],
                ),
                _case(
                    "tick-order-mark-done-before-dispatch",
                    [
                        _run("past", "2026-08-31T06:59:59+08:00", ["1001"], HOME_A),
                        _run("tonight", "2026-08-31T21:30:00+08:00", ["1002"], HOME_A),
                        _remind("past", "countdown_15", "2026-08-31T06:44:59+08:00"),
                        _remind("past", "day_of", "2026-08-30T09:00:30+08:00"),
                        _remind("tonight", "day_of", "2026-08-31T09:00:00+08:00"),
                        DUE,
                        {"op": "mark_done"},
                        DUE,
                        DISPATCH,
                    ],
                ),
            ],
        }
    }
