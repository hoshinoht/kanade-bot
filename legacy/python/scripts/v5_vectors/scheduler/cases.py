"""Serialized scheduler scenarios; expected results are produced by the oracle."""

from __future__ import annotations

from typing import Any

SETUP = {
    "clock": "2026-08-27T01:00:00+08:00",
    "timezone": "Asia/Kuala_Lumpur",
    "reset_weekday": 3,
    "reset_time": "00:00:00",
    "ping_time": "09:00:00",
    "countdowns": [60, 15],
    "week_start": "2026-08-27T00:00:00+08:00",
    "uuid_sequence": [f"00000000-0000-4000-8000-{number:012d}" for number in range(1, 40)],
}


def _case(case_id: str, steps: list[dict[str, Any]]) -> dict[str, Any]:
    return {"case_id": case_id, "input": {**SETUP, "steps": steps}}


def documents() -> dict[str, dict[str, Any]]:
    return {
        "scheduler.json": {
            "schema_version": "v5-scheduler-v1",
            "family": "scheduler",
            "provenance": {
                "oracle": (
                    "legacy/python bot.agent.materialise, bot.agent.rsvp, bot.infrastructure.db"
                ),
                "functions": [
                    "Repo.add_fixed_run",
                    "Repo.create_run",
                    "materialise_week",
                    "apply_fixed_to_runs",
                    "retire_fixed_run",
                    "apply_reaction",
                ],
                "source_tests": [
                    "tests/test_materialise.py",
                    "tests/test_fixed_edit.py",
                    "tests/test_rsvp.py",
                    "tests/test_participants.py",
                ],
                "inventory_surfaces": ["scheduler materialisation", "fixed mutation", "RSVP state"],
            },
            "cases": [
                _case(
                    "materialise-repeat-idempotence",
                    [
                        {
                            "op": "add_fixed",
                            "fixed_key": "weekly",
                            "owner_id": "42",
                            "bosses": ["HMaleficStar", "HFA"],
                            "weekday": 0,
                            "time": "21:30",
                            "participants": ["1", "2", "3"],
                            "channel_id": "900",
                        },
                        {"op": "materialise"},
                        {"op": "materialise"},
                    ],
                ),
                _case(
                    "adopt-manual-matching-run-preserves-relevant-rsvp",
                    [
                        {
                            "op": "create_run",
                            "run_key": "manual",
                            "bosses": ["HMaleficStar", "HFA"],
                            "at": "2026-08-31T22:00:00+08:00",
                            "participants": ["1", "9"],
                            "status": "confirmed",
                            "source": "amend",
                            "channel_id": "900",
                        },
                        {
                            "op": "set_rsvp",
                            "run_key": "manual",
                            "user_id": "1",
                            "state": "yes",
                            "source": "chat",
                        },
                        {
                            "op": "set_rsvp",
                            "run_key": "manual",
                            "user_id": "9",
                            "state": "yes",
                            "source": "slash",
                        },
                        {
                            "op": "add_fixed",
                            "fixed_key": "weekly",
                            "owner_id": "42",
                            "bosses": ["HMaleficStar", "HFA"],
                            "weekday": 0,
                            "time": "21:30",
                            "participants": ["1", "2", "3"],
                            "channel_id": "900",
                        },
                        {"op": "materialise"},
                    ],
                ),
                _case(
                    "fixed-edit-and-retire-preserve-terminal-run",
                    [
                        {
                            "op": "add_fixed",
                            "fixed_key": "weekly",
                            "owner_id": "42",
                            "bosses": ["HMaleficStar", "HFA"],
                            "weekday": 0,
                            "time": "21:30",
                            "participants": ["1", "2"],
                            "channel_id": "900",
                        },
                        {"op": "materialise"},
                        {"op": "materialise", "week_start": "2026-09-03T00:00:00+08:00"},
                        {
                            "op": "set_status",
                            "run_key": "weekly@2026-08-27T00:00:00+08:00",
                            "status": "done",
                        },
                        {
                            "op": "edit_fixed",
                            "fixed_key": "weekly",
                            "fields": {"weekday": 4, "time": "22:30"},
                            "changed": ["weekday", "time"],
                            "week_starts": [
                                "2026-08-27T00:00:00+08:00",
                                "2026-09-03T00:00:00+08:00",
                            ],
                        },
                        {
                            "op": "retire_fixed",
                            "fixed_key": "weekly",
                            "week_starts": [
                                "2026-08-27T00:00:00+08:00",
                                "2026-09-03T00:00:00+08:00",
                            ],
                        },
                    ],
                ),
                _case(
                    "reaction-guards-removal-and-sticky-statuses",
                    [
                        {
                            "op": "create_run",
                            "run_key": "open",
                            "bosses": ["HMaleficStar"],
                            "at": "2026-08-31T21:30:00+08:00",
                            "participants": ["1", "2"],
                            "status": "planned",
                            "source": "amend",
                            "channel_id": "900",
                        },
                        {
                            "op": "reaction",
                            "run_key": "open",
                            "user_id": "9",
                            "emoji": "✅",
                            "added": True,
                        },
                        {
                            "op": "reaction",
                            "run_key": "open",
                            "user_id": "1",
                            "emoji": "✅",
                            "added": True,
                        },
                        {
                            "op": "reaction",
                            "run_key": "open",
                            "user_id": "2",
                            "emoji": "❌",
                            "added": True,
                        },
                        {
                            "op": "reaction",
                            "run_key": "open",
                            "user_id": "2",
                            "emoji": "❌",
                            "added": False,
                        },
                        {
                            "op": "reaction",
                            "run_key": "open",
                            "user_id": "2",
                            "emoji": "✅",
                            "added": True,
                        },
                        {
                            "op": "create_run",
                            "run_key": "cancelled",
                            "bosses": ["HFA"],
                            "at": "2026-09-01T21:30:00+08:00",
                            "participants": ["1"],
                            "status": "cancelled",
                            "source": "amend",
                            "channel_id": "900",
                        },
                        {
                            "op": "reaction",
                            "run_key": "cancelled",
                            "user_id": "1",
                            "emoji": "❌",
                            "added": True,
                        },
                        {
                            "op": "create_run",
                            "run_key": "done",
                            "bosses": ["HFA"],
                            "at": "2026-09-02T21:30:00+08:00",
                            "participants": ["1"],
                            "status": "done",
                            "source": "amend",
                            "channel_id": "900",
                        },
                        {
                            "op": "reaction",
                            "run_key": "done",
                            "user_id": "1",
                            "emoji": "✅",
                            "added": True,
                        },
                    ],
                ),
                _case(
                    "negative-invalid-rsvp-state",
                    [
                        {
                            "op": "create_run",
                            "run_key": "open",
                            "bosses": ["HFA"],
                            "at": "2026-08-31T21:30:00+08:00",
                            "participants": ["1"],
                            "status": "planned",
                            "source": "amend",
                            "channel_id": "900",
                        },
                        {
                            "op": "set_rsvp",
                            "run_key": "open",
                            "user_id": "1",
                            "state": "unknown",
                            "source": "chat",
                            "error_type": "ValueError",
                        },
                    ],
                ),
            ],
        }
    }
