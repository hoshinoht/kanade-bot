import json
from pathlib import Path

import pytest

from bot.infrastructure.bundle import Bundle, PreflightContext, preflight
from bot.infrastructure.bundle.errors import BundleError

FIXTURE = Path(__file__).parent / "fixtures" / "bundle" / "example.json"


def value() -> dict:
    return json.loads(FIXTURE.read_text())


def context() -> PreflightContext:
    return PreflightContext(
        guild_id="100000000000000000",
        catalog_revision="synthetic-v1",
        catalog_tokens=frozenset({"SLotus"}),
        timezone="Asia/Kuala_Lumpur",
        reset_time="00:00",
    )


def test_preflight_accepts_fixture_without_side_effects() -> None:
    report = preflight(Bundle.model_validate_json(json.dumps(value())), context())
    assert report.counts == {
        "members": 1,
        "fixed_runs": 1,
        "runs": 1,
        "amendments": 2,
        "messages": 1,
    }


@pytest.mark.parametrize(
    ("mutate", "code"),
    [
        (lambda v: v["manifest"].update(guild_id="999999999999999999"), "guild-mismatch"),
        (
            lambda v: v["schedule"]["members"].append(v["schedule"]["members"][0]),
            "duplicate-member",
        ),
        (
            lambda v: v["schedule"]["runs"][0].update(
                fixed_run_id="00000000-0000-0000-0000-000000000099"
            ),
            "fixed-run-reference",
        ),
        (
            lambda v: v["schedule"]["rsvps"][0].update(user_id="999999999999999999"),
            "rsvp-reference",
        ),
        (
            lambda v: v["history"]["extractions"][0].update(message_ids=["999999999999999999"]),
            "extraction-reference",
        ),
        (lambda v: v["schedule"]["runs"][0].update(bosses=["Unknown"]), "unknown-boss"),
    ],
)
def test_preflight_rejects_context_and_foreign_key_mismatches(mutate, code: str) -> None:
    candidate = value()
    mutate(candidate)
    with pytest.raises(BundleError, match=code):
        preflight(Bundle.model_validate_json(json.dumps(candidate)), context())


def test_terminal_history_can_retain_explicit_historic_boss_only() -> None:
    candidate = value()
    candidate["catalog"]["historic_tokens"] = ["OldBoss"]
    candidate["schedule"]["runs"][0].update(status="done", bosses=["OldBoss"])
    preflight(Bundle.model_validate_json(json.dumps(candidate)), context())
    candidate["schedule"]["runs"][0]["status"] = "planned"
    with pytest.raises(BundleError, match="unknown-boss"):
        preflight(Bundle.model_validate_json(json.dumps(candidate)), context())


@pytest.mark.parametrize("status", ["done", "cancelled"])
def test_terminal_runs_can_retain_deleted_fixed_run(status: str) -> None:
    candidate = value()
    candidate["schedule"]["fixed_runs"] = []
    candidate["schedule"]["runs"][0]["status"] = status
    preflight(Bundle.model_validate_json(json.dumps(candidate)), context())


def test_live_run_cannot_retain_deleted_fixed_run() -> None:
    candidate = value()
    candidate["schedule"]["fixed_runs"] = []
    with pytest.raises(BundleError, match="fixed-run-reference"):
        preflight(Bundle.model_validate_json(json.dumps(candidate)), context())


def test_confirmed_amendment_can_retain_declared_historic_boss_only() -> None:
    candidate = value()
    candidate["catalog"]["historic_tokens"] = ["OldBoss"]
    candidate["schedule"]["amendments"][0]["bosses"] = ["OldBoss"]
    preflight(Bundle.model_validate_json(json.dumps(candidate)), context())
    candidate["catalog"]["historic_tokens"] = []
    with pytest.raises(BundleError, match="unknown-boss"):
        preflight(Bundle.model_validate_json(json.dumps(candidate)), context())


@pytest.mark.parametrize(
    ("mutate", "code"),
    [
        (
            lambda v: v["schedule"]["amendments"][0].update(
                evidence_msg_ids=["999999999999999999"]
            ),
            "amendment-evidence-reference",
        ),
        (
            lambda v: (
                v["schedule"]["members"].append(
                    {**v["schedule"]["members"][0], "user_id": "100000000000000099"}
                ),
                v["schedule"]["rsvps"][0].update(user_id="100000000000000099"),
            ),
            "rsvp-participant-reference",
        ),
        (
            lambda v: (
                v["schedule"]["members"].append(
                    {**v["schedule"]["members"][0], "user_id": "100000000000000099"}
                ),
                v["delivery"]["declines"][0].update(user_id="100000000000000099"),
                v["delivery"]["attempts"][3]["targets"][0].update(user_id="100000000000000099"),
            ),
            "decline-participant-reference",
        ),
    ],
)
def test_preflight_rejects_retained_relationship_mismatches(mutate, code: str) -> None:
    candidate = value()
    mutate(candidate)
    with pytest.raises(BundleError, match=code):
        preflight(Bundle.model_validate_json(json.dumps(candidate)), context())
