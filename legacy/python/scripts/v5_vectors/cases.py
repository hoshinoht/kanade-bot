"""Serialized inputs for the first v5 pure-domain vector subset."""

from __future__ import annotations

from typing import Any


def case(
    case_id: str, op: str, input_: dict[str, Any], error_type: str | None = None
) -> dict[str, Any]:
    result: dict[str, Any] = {"case_id": case_id, "op": op, "input": input_}
    if error_type is not None:
        result["error_type"] = error_type
    return result


RESET = {"timezone": "Asia/Kuala_Lumpur", "reset_weekday": 3, "reset_time": "00:00:00"}
LATE_RESET = {**RESET, "reset_time": "12:00:00"}
IDS = [
    "a1b2c3d4-1111-4222-8333-444444444444",
    "a1b2ffff-5555-4666-8777-888888888888",
    "99999999-0000-4000-8000-000000000000",
]
CATALOG = {
    "difficulties": [
        {"prefix": "n", "label": "Normal"},
        {"prefix": "h", "label": "Hard"},
        {"prefix": "x", "label": "Extreme"},
    ],
    "bosses": [
        {
            "short": "Star",
            "full": "Radiant Star",
            "level": 280,
            "difficulties": ["n", "h"],
            "aliases": ["star", "rstar"],
        },
        {
            "short": "Kalos",
            "full": "Gatekeeper Kalos",
            "level": 265,
            "difficulties": ["n", "x"],
            "aliases": ["kalos", "gatekeeper kalos"],
        },
    ],
}
CATALOG_INPUT = {"catalog_id": "synthetic-bosses-v1"}


def documents() -> dict[str, dict[str, Any]]:
    return {
        "weeks.json": {
            "schema_version": "v5-domain-v1",
            "family": "weeks",
            "provenance": {
                "oracle": "legacy/python bot.domain.weeks",
                "functions": [
                    "week_start",
                    "week_end",
                    "calendar_week_start",
                    "calendar_week_end",
                    "materialised_week_starts",
                    "slot_in_week",
                    "parse_weekday",
                    "parse_hhmm",
                ],
                "source_tests": [
                    "tests/test_weeks.py::test_exactly_on_the_reset_starts_the_new_week",
                    "tests/test_weeks.py::test_calendar_weeks_start_on_monday_while_boss_weeks_keep_their_reset",
                    "tests/test_weeks.py::test_slot_before_a_late_reset_is_pushed_into_the_week",
                    "tests/test_weeks.py::test_parse_hhmm",
                ],
                "inventory_surfaces": ["domain.weeks"],
            },
            "cases": [
                case(
                    "weeks.reset.exact", "week_start", {**RESET, "at": "2026-08-27T00:00:00+08:00"}
                ),
                case(
                    "weeks.reset.precise-before",
                    "week_start",
                    {**RESET, "at": "2026-08-26T23:59:59+08:00"},
                ),
                case(
                    "weeks.reset.utc-input",
                    "week_start",
                    {**RESET, "at": "2026-08-26T16:30:00+00:00"},
                ),
                case(
                    "weeks.reset.late-before",
                    "week_start",
                    {**LATE_RESET, "at": "2026-08-27T09:00:00+08:00"},
                ),
                case(
                    "weeks.calendar.distinct-from-boss",
                    "calendar_week_bounds",
                    {**RESET, "at": "2026-09-09T12:00:00+08:00"},
                ),
                case(
                    "weeks.materialised.three",
                    "materialised_week_starts",
                    {**RESET, "at": "2026-09-09T12:00:00+08:00"},
                ),
                case(
                    "weeks.slot.late-reset-forward",
                    "slot_in_week",
                    {
                        "timezone": "Asia/Kuala_Lumpur",
                        "week_start": "2026-08-27T12:00:00+08:00",
                        "weekday": 3,
                        "time": "09:00:00",
                    },
                ),
                case(
                    "weeks.dst.wall-clock-end",
                    "week_end",
                    {"timezone": "America/New_York", "week_start": "2026-03-08T00:00:00-05:00"},
                ),
                case("weeks.parse-weekday.alias", "parse_weekday", {"value": "weds"}),
                case(
                    "weeks.parse-weekday.invalid",
                    "parse_weekday",
                    {"value": "caturday"},
                    "ValueError",
                ),
                case("weeks.parse-hhmm.meridiem", "parse_hhmm", {"value": "930pm"}),
                case("weeks.parse-hhmm.invalid", "parse_hhmm", {"value": "13pm"}, "ValueError"),
            ],
        },
        "timeutil.json": {
            "schema_version": "v5-domain-v1",
            "family": "timeutil",
            "provenance": {
                "oracle": "legacy/python bot.domain.timeutil",
                "functions": ["to_iso", "from_iso", "local_naive"],
                "source_tests": [],
                "inventory_surfaces": ["domain.timeutil"],
            },
            "cases": [
                case("timeutil.to-iso.utc", "to_iso", {"at": "2026-09-09T12:00:00+08:00"}),
                case("timeutil.from-iso.naive", "from_iso", {"value": "2026-09-09T12:00:00"}),
                case(
                    "timeutil.from-iso.offset", "from_iso", {"value": "2026-09-09T12:00:00+08:00"}
                ),
                case(
                    "timeutil.local-naive",
                    "local_naive",
                    {"at": "2026-09-09T04:00:00+00:00", "timezone": "Asia/Kuala_Lumpur"},
                ),
                case(
                    "timeutil.to-iso.naive-error",
                    "to_iso",
                    {"at": "2026-09-09T12:00:00"},
                    "ValueError",
                ),
            ],
        },
        "ids.json": {
            "schema_version": "v5-domain-v1",
            "family": "ids",
            "provenance": {
                "oracle": "legacy/python bot.domain.ids",
                "functions": ["canonical", "short_id", "tag", "resolve_id"],
                "source_tests": [
                    "tests/test_ids.py::test_canonical_strips_decoration",
                    "tests/test_ids.py::test_short_id_is_the_first_eight_hex_characters",
                    "tests/test_ids.py::test_tag_adds_the_hash",
                    "tests/test_ids.py::test_an_ambiguous_prefix_lists_its_candidates",
                ],
                "inventory_surfaces": ["domain.ids"],
            },
            "cases": [
                case("ids.canonical.decorated", "canonical", {"value": f"  #{IDS[0].upper()}  "}),
                case("ids.short-id", "short_id", {"value": IDS[0]}),
                case("ids.tag", "tag", {"value": IDS[0]}),
                case("ids.resolve.exact", "resolve_id", {"text": IDS[0], "candidates": IDS}),
                case(
                    "ids.resolve.unique-prefix", "resolve_id", {"text": "#9999", "candidates": IDS}
                ),
                case(
                    "ids.resolve.ambiguous",
                    "resolve_id",
                    {"text": "a1b2", "candidates": IDS},
                    "IdAmbiguous",
                ),
                case(
                    "ids.resolve.not-found",
                    "resolve_id",
                    {"text": "dead", "candidates": IDS},
                    "IdNotFound",
                ),
                case(
                    "ids.resolve.too-short",
                    "resolve_id",
                    {"text": "#abc", "candidates": IDS},
                    "IdTooShort",
                ),
            ],
        },
        "bosses.json": {
            "schema_version": "v5-domain-v1",
            "family": "bosses",
            "provenance": {
                "oracle": "legacy/python bot.domain.bosses",
                "functions": [
                    "BossTable.from_dict",
                    "parse_token",
                    "parse",
                    "resolve_reference",
                    "names_in",
                    "describe",
                    "detail",
                ],
                "source_tests": [
                    "tests/test_bosses.py::test_parse_token",
                    "tests/test_bosses.py::test_bare_boss_name_demands_a_difficulty",
                    "tests/test_bosses.py::test_parse_list",
                    "tests/test_bosses.py::test_free_form_reference_resolution",
                    "tests/test_bosses.py::test_a_loose_sentence_gives_up_the_bosses_it_names",
                    "tests/test_bosses.py::test_describe_expands_to_the_full_in_game_name",
                    "tests/test_bosses.py::test_table_needs_difficulties_and_bosses",
                ],
                "inventory_surfaces": ["domain.bosses"],
            },
            "fixtures": {"catalogs": {"synthetic-bosses-v1": CATALOG}},
            "cases": [
                case("bosses.token.alias", "parse_token", {**CATALOG_INPUT, "token": "h-rstar"}),
                case("bosses.token.canonical", "parse_token", {**CATALOG_INPUT, "token": "XKalos"}),
                case(
                    "bosses.token.missing-difficulty",
                    "parse_token",
                    {**CATALOG_INPUT, "token": "kalos"},
                    "BossParseError",
                ),
                case(
                    "bosses.token.unsupported-difficulty",
                    "parse_token",
                    {**CATALOG_INPUT, "token": "hkalos"},
                    "BossParseError",
                ),
                case(
                    "bosses.token.not-found",
                    "parse_token",
                    {**CATALOG_INPUT, "token": "hzzz"},
                    "BossParseError",
                ),
                case(
                    "bosses.parse.spelled-and-order",
                    "parse",
                    {
                        **CATALOG_INPUT,
                        "text": "Hard Radiant Star, Extreme Gatekeeper Kalos, hard star",
                    },
                ),
                case(
                    "bosses.reference.free-form",
                    "resolve_reference",
                    {**CATALOG_INPUT, "text": "please explain Extreme Gatekeeper Kalos tonight"},
                ),
                case(
                    "bosses.reference.conflicting",
                    "resolve_reference",
                    {**CATALOG_INPUT, "text": "Hard NStar"},
                    "BossParseError",
                ),
                case(
                    "bosses.names-in",
                    "names_in",
                    {**CATALOG_INPUT, "text": "hstar and xkalos and hstar"},
                ),
                case("bosses.describe", "describe", {**CATALOG_INPUT, "canonical": "HStar"}),
                case("bosses.detail", "detail", {**CATALOG_INPUT, "canonical": "HStar"}),
            ],
        },
    }
