import json
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator
from pydantic import ValidationError

from bot.infrastructure.bundle import Bundle, assert_crosswalk_complete, canonical_json
from bot.infrastructure.bundle.crosswalk import TABLE_CROSSWALK

FIXTURE = Path(__file__).parent / "fixtures" / "bundle" / "example.json"
SCHEMA = Path(__file__).parents[1] / "docs" / "bundle-contract" / "bundle-v1.schema.json"


def load() -> dict:
    return json.loads(FIXTURE.read_text())


def test_synthetic_bundle_is_deterministic_and_crosswalk_complete() -> None:
    assert_crosswalk_complete()
    assert TABLE_CROSSWALK["adoption_sources"] == "exclude"
    first = Bundle.model_validate_json(FIXTURE.read_bytes())
    second = Bundle.model_validate_json(canonical_json(first))
    assert canonical_json(first) == canonical_json(second)
    schema = json.loads(SCHEMA.read_text())
    Draft202012Validator.check_schema(schema)
    assert not list(Draft202012Validator(schema).iter_errors(load()))


@pytest.mark.parametrize("version", [None, 2])
def test_format_version_is_required_const_one_in_python_and_schema(version: int | None) -> None:
    value = load()
    if version is None:
        del value["manifest"]["format_version"]
    else:
        value["manifest"]["format_version"] = version
    encoded = json.dumps(value)
    with pytest.raises(ValidationError):
        Bundle.model_validate_json(encoded)
    assert list(Draft202012Validator(json.loads(SCHEMA.read_text())).iter_errors(value))


@pytest.mark.parametrize(
    "mutate",
    [
        lambda value: value["config"]["runtime"].update(admin_token="secret"),
        lambda value: value["delivery"]["attempts"].__setitem__(
            0, {"state": "retired", "delivery_kind": "reminder"}
        ),
    ],
)
def test_published_schema_rejects_secret_and_unaccounted_retirement(mutate) -> None:
    value = load()
    mutate(value)
    with pytest.raises(ValidationError):
        Bundle.model_validate_json(json.dumps(value))
    assert list(Draft202012Validator(json.loads(SCHEMA.read_text())).iter_errors(value))


@pytest.mark.parametrize("kind", ["generic", "memory"])
def test_published_schema_closes_delivery_kinds(kind: str) -> None:
    value = load()
    value["delivery"]["attempts"][0]["delivery_kind"] = kind
    with pytest.raises(ValidationError):
        Bundle.model_validate_json(json.dumps(value))
    assert list(Draft202012Validator(json.loads(SCHEMA.read_text())).iter_errors(value))


@pytest.mark.parametrize("snowflake", ["0", "01", "+1", " 1"])
def test_published_schema_rejects_noncanonical_snowflakes(snowflake: str) -> None:
    value = load()
    value["delivery"]["attempts"][0]["message_id"] = snowflake
    with pytest.raises(ValidationError, match="pattern"):
        Bundle.model_validate_json(json.dumps(value))
    assert list(Draft202012Validator(json.loads(SCHEMA.read_text())).iter_errors(value))


def test_published_schema_requires_targetless_pre_journal_attestations() -> None:
    value = load()
    value["delivery"]["attempts"].append(
        {
            "state": "retired",
            "delivery_kind": "pre_journal_attestation",
            "targets": [
                {"binding_type": "reminder", "reminder_id": "00000000-0000-0000-0000-000000000006"}
            ],
            "retired_by": "system",
            "retired_reason": "invalid target claim",
        }
    )
    with pytest.raises(ValidationError):
        Bundle.model_validate_json(json.dumps(value))
    assert list(Draft202012Validator(json.loads(SCHEMA.read_text())).iter_errors(value))


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (lambda value: value.update(memory={}), "Extra inputs"),
        (
            lambda value: value["delivery"]["attempts"].append(
                {
                    "state": "indeterminate",
                }
            ),
            "union_tag_invalid",
        ),
        (lambda value: value["config"]["runtime"].update(admin_token="secret"), "not exportable"),
        (
            lambda value: value["schedule"]["members"][0].update(user_id="not-a-snowflake"),
            "pattern",
        ),
        (
            lambda value: value["schedule"]["runs"][0].update(datetime="2026-09-18T20:00:00"),
            "timezone-aware",
        ),
        (
            lambda value: value["delivery"].update(current_week_start="2026-09-18T00:00:00+08:00"),
            "current-boss-week",
        ),
    ],
)
def test_rejects_excluded_or_no_replay_states(mutate, message: str) -> None:
    value = load()
    mutate(value)
    with pytest.raises(ValidationError, match=message):
        Bundle.model_validate_json(json.dumps(value))


def test_explicitly_retired_attempt_allows_no_replay_export() -> None:
    value = load()
    value["delivery"]["attempts"].append(
        {
            "state": "retired",
            "delivery_kind": "reminder",
            "retired_by": "system",
            "retired_reason": "authoritative reconciliation unavailable",
        }
    )
    assert Bundle.model_validate_json(json.dumps(value)).delivery.attempts[-1].state == "retired"


def test_closed_legacy_producer_envelopes_preserve_known_shapes() -> None:
    value = load()
    amendment = value["schedule"]["amendments"][0]
    amendment.update(
        kind="split", payload={"bosses": ["SLotus"], "participants": ["100000000000000001"]}
    )
    interaction = value["history"]["chat_interactions"][0]
    interaction["tool_calls"] = [
        {
            "name": "propose_move",
            "round": 1,
            "arguments": "run=...",
            "output": "ok",
            "ms": 2,
            "outcome": "ok",
            "created": ["00000000-0000-0000-0000-000000000003"],
            "posted": ["00000000-0000-0000-0000-000000000003"],
        }
    ]
    interaction["model_rounds"] = [
        {"round": 1, "content": "yes", "thinking": None, "requested_tools": []}
    ]
    value["history"]["rescan_jobs"][0]["results"] = [
        {
            "channel_id": "100000000000000003",
            "channel_name": "synthetic",
            "asked": True,
            "window": "current",
            "since": "2026-09-17T00:00:00+08:00",
            "widened": False,
            "backfilled": 0,
            "stored": 1,
            "gated": 0,
            "bursts": 1,
            "extracted": 1,
            "proposals": 1,
            "dropped": 0,
            "stale": 0,
            "elapsed_ms": 2,
            "cancelled": False,
            "error": None,
            "summary": "synthetic",
            "proposed": [
                {
                    "kind": "split",
                    "bosses": ["SLotus"],
                    "confidence": 0.9,
                    "run_id": "00000000-0000-0000-0000-000000000002",
                }
            ],
        }
    ]
    Bundle.model_validate_json(json.dumps(value))
    value["history"]["chat_interactions"][0]["tool_calls"][0]["memory"] = "forbidden"
    with pytest.raises(ValidationError, match="Extra inputs"):
        Bundle.model_validate_json(json.dumps(value))


@pytest.mark.parametrize(
    "payload",
    [
        {
            "op": "remove",
            "fixed_run_id": "00000000-0000-0000-0000-000000000001",
            "weekly_when": "Fri 20:00",
        },
        {
            "op": "edit",
            "fixed_run_id": "00000000-0000-0000-0000-000000000001",
            "weekly_when": "Fri 20:00",
            "weekday": 5,
            "time": "21:00",
            "participants": ["100000000000000001"],
        },
    ],
)
def test_chat_fix_producer_payloads_are_portable(payload: dict) -> None:
    value = load()
    value["schedule"]["amendments"][0].update(kind="fix", payload=payload)
    Bundle.model_validate_json(json.dumps(value))


def test_extractor_fix_producer_payloads_are_portable() -> None:
    from datetime import date, time

    from bot.extract.pipeline import _fix_payload
    from bot.extract.resolve import Resolved

    validator = Draft202012Validator(json.loads(SCHEMA.read_text()))
    for resolved in (
        Resolved(),
        Resolved(day=date(2026, 9, 25)),
        Resolved(day=date(2026, 9, 25), clock=time(20, 0)),
    ):
        value = load()
        payload = {**_fix_payload(resolved), "also_mentioned": ["move"]}
        value["schedule"]["amendments"][0].update(kind="fix", payload=payload)
        bundle = Bundle.model_validate_json(json.dumps(value))
        validator.validate(value)
        assert Bundle.model_validate_json(canonical_json(bundle)) == bundle


def test_chat_fix_identity_cannot_fall_back_to_extractor_form() -> None:
    complete = {
        "op": "edit",
        "fixed_run_id": "00000000-0000-0000-0000-000000000001",
        "weekly_when": "Fri 20:00",
    }
    for missing in complete:
        value = load()
        payload = {key: item for key, item in complete.items() if key != missing}
        value["schedule"]["amendments"][0].update(kind="fix", payload=payload)
        with pytest.raises(ValidationError, match="fix payload"):
            Bundle.model_validate_json(json.dumps(value))
