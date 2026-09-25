"""Serialized mention-policy scenarios; expected results come only from the v4 oracle."""

from __future__ import annotations

from typing import Any

MEMBERS = [
    {
        "user_id": "1",
        "display_name": "Alvin tan",
        "nickname": None,
        "has_role": True,
        "ping_level": "essential",
    },
    {
        "user_id": "2",
        "display_name": "kanon [AZUR]",
        "nickname": "kanon",
        "has_role": True,
        "ping_level": "all",
    },
    {
        "user_id": "3",
        "display_name": "Priya",
        "nickname": None,
        "has_role": True,
        "ping_level": "off",
    },
]
ALL = ["1", "2", "3"]
KINDS = [
    "day_of",
    "countdown",
    "proposal",
    "decline",
    "amend",
    "status",
    "swap",
    "fixed",
    "digest",
    "retract",
    "rescan",
    "test",
    "nonsense",
]


def _case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
    return {
        "case_id": case_id,
        "input": {
            "clock": "2026-08-27T01:00:00+08:00",
            "timezone": "Asia/Kuala_Lumpur",
            "members": MEMBERS,
            "steps": steps,
        },
    }


def _resolve(candidates: list[Any], kind: str) -> dict[str, Any]:
    return {"op": "resolve_mentions", "candidates": candidates, "kind": kind}


def _audience(people: list[str], kind: str, candidates: list[str] | None = None):
    return {"op": "audience", "people": people, "kind": kind, "candidates": candidates}


def _prepared(card: list[str], users: list[str] | None, quiet: bool = False) -> dict[str, Any]:
    return {
        "op": "prepared_allow_list",
        "card_mentions": card,
        "mention_users": users,
        "quiet_mode": quiet,
    }


PROVENANCE = {
    "oracle": "legacy/python bot.agent.pings, bot.agent.formatting, BossBot._prepared",
    "functions": [
        "bot.agent.pings.resolve_mentions",
        "bot.agent.pings.audience",
        "bot.agent.pings.wants_mention",
        "bot.agent.pings.normalise_level",
        "bot.agent.formatting.not_declined",
        "bot.agent.formatting.everyone_on",
        "bot.agent.client.BossBot._prepared",
        "bot.infrastructure.db.Repo.set_ping_level",
    ],
    "source_tests": [
        "tests/test_pings.py::test_an_essential_post_mentions_everyone_but_the_opted_out",
        "tests/test_pings.py::test_an_informational_post_only_mentions_the_people_who_asked",
        "tests/test_pings.py::test_off_is_never_mentioned_by_anything",
        "tests/test_pings.py::test_the_default_level_is_essential",
        "tests/test_pings.py::test_somebody_the_roster_has_never_seen_still_gets_their_reminders",
        "tests/test_pings.py::test_the_resolver_keeps_order_and_drops_duplicates",
        "tests/test_pings.py::test_an_unknown_kind_is_treated_as_informational",
        "tests/test_pings.py::test_an_invented_level_is_refused",
        "tests/test_pings.py::test_setting_the_level_of_somebody_who_is_not_on_the_roster",
        "tests/test_pings.py::test_an_audience_mentions_the_resolved_and_names_the_rest",
        "tests/test_pings.py::test_a_nickname_wins_over_the_display_name",
        "tests/test_pings.py::test_a_countdown_only_pings_the_people_who_have_not_answered",
        "tests/test_pings.py::test_a_decline_still_summons_the_rest_of_the_run",
        "tests/test_qol.py::test_a_countdown_goes_to_the_whole_party_bar_the_decliners",
        "tests/test_digest.py::test_the_digest_names_people_rather_than_mentioning_them",
        "tests/test_swap.py::test_the_channel_is_told_who_is_in_and_out",
    ],
    "inventory_surfaces": ["mention policy", "ping levels"],
}


def documents() -> dict[str, dict[str, Any]]:
    return {
        "mentions.json": {
            "schema_version": "v5-scheduler-mentions-v1",
            "family": "mentions",
            "provenance": PROVENANCE,
            "cases": [
                _case(
                    "levels-by-kind-and-unknown-kind",
                    [{"op": "ping_kinds"}]
                    + [_resolve(ALL, kind) for kind in KINDS]
                    + [
                        {"op": "wants_mention", "level": level, "kind": kind}
                        for level in ("off", "essential", "all")
                        for kind in ("day_of", "swap", "nonsense")
                    ],
                ),
                _case(
                    "order-dedup-and-unknown-members",
                    [
                        _resolve(["2", "1", "2", 1], "day_of"),
                        _resolve(["404", "1"], "day_of"),
                        _resolve(["404", "2"], "amend"),
                        _resolve([], "countdown"),
                    ],
                ),
                _case(
                    "audiences-per-post-kind",
                    [
                        _audience(ALL, "amend"),
                        _audience(["2"], "swap"),
                        _audience(["404"], "swap"),
                        _audience(ALL, "countdown", ["2", "3"]),
                        {
                            "op": "not_declined",
                            "participants": ALL,
                            "rsvps": [["1", "yes"], ["2", "maybe"], ["3", "no"]],
                        },
                        _audience(ALL, "countdown", ["1", "2"]),
                        {"op": "everyone_on", "runs": [["2"], ["1", "2", "3"], ["3", "404"]]},
                        _audience(["2", "1", "3", "404"], "day_of"),
                        _audience(["1", "3", "2"], "swap"),
                        _audience(["2", "3"], "decline"),
                        _audience(ALL, "digest"),
                    ],
                ),
                _case(
                    "allow-list-gate-and-digest-empty-list",
                    [
                        _prepared(["1", "2"], None),
                        _prepared(["1", "2"], []),
                        _prepared(["1", "2"], ["2"]),
                        _prepared(["1", "2"], None, quiet=True),
                    ],
                ),
                _case(
                    "level-changes-and-refusals",
                    [
                        {"op": "normalise_level", "value": " OFF "},
                        {"op": "normalise_level", "value": "loud", "error_type": "ValueError"},
                        {"op": "normalise_level", "value": None, "error_type": "ValueError"},
                        {"op": "set_ping_level", "user_id": "1", "level": "all"},
                        _resolve(ALL, "swap"),
                        {
                            "op": "set_ping_level",
                            "user_id": "1",
                            "level": "loud",
                            "error_type": "ValueError",
                        },
                        {
                            "op": "set_ping_level",
                            "user_id": "404",
                            "level": "off",
                            "error_type": "KeyError",
                        },
                    ],
                ),
            ],
        }
    }
