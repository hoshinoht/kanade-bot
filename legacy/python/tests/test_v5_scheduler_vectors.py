"""Producer checks for the stateful v5 scheduler compatibility vectors."""

from __future__ import annotations

import json
from copy import deepcopy
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator, FormatChecker, ValidationError

from scripts.v5_vectors.scheduler import generator, replay, validation

ROOT = Path(__file__).resolve().parents[3]
VECTOR_DIR = ROOT / "docs" / "v5" / "vectors" / "scheduler"


def _document() -> dict:
    return json.loads((VECTOR_DIR / "scheduler.json").read_text())


def _input_only_case(index: int = 0) -> dict:
    case = _document()["cases"][index]
    return {"case_id": case["case_id"], "input": deepcopy(case["input"])}


def _validator() -> Draft202012Validator:
    return Draft202012Validator(
        json.loads((VECTOR_DIR / "schema.json").read_text()), format_checker=FormatChecker()
    )


def test_scheduler_vectors_validate_and_replay_from_json():
    document = _document()
    _validator().validate(document)
    validation.validate_document(document)
    for case in document["cases"]:
        parsed = json.loads(json.dumps(case))
        assert replay.replay(parsed) == case["expected"]


def test_scheduler_replay_is_identical_for_two_clean_stores():
    for case in _document()["cases"]:
        assert replay.replay(case) == replay.replay(case)


def test_scheduler_replay_accepts_input_only_case_before_expected_attachment():
    case = _input_only_case()
    expected = _document()["cases"][0]["expected"]
    assert replay.replay(case) == expected


@pytest.mark.parametrize(
    "mutate",
    [
        pytest.param(
            lambda case: case["input"]["countdowns"].__setitem__(0, "60"), id="countdown-string"
        ),
        pytest.param(
            lambda case: case["input"]["countdowns"].__setitem__(0, -1), id="countdown-negative"
        ),
        pytest.param(
            lambda case: case["input"]["countdowns"].__setitem__(0, 1.5), id="countdown-float"
        ),
        pytest.param(
            lambda case: case["input"]["countdowns"].__setitem__(0, True), id="countdown-bool"
        ),
        pytest.param(lambda case: case["input"].update(uuid_sequence=[]), id="uuid-empty"),
        pytest.param(lambda case: case["input"].update(uuid_sequence=[1]), id="uuid-nonstrings"),
        pytest.param(lambda case: case["input"].update(reset_weekday=-1), id="weekday-negative"),
        pytest.param(lambda case: case["input"].update(reset_weekday=7), id="weekday-seven"),
        pytest.param(lambda case: case["input"].update(reset_weekday="3"), id="weekday-string"),
        pytest.param(lambda case: case["input"].update(reset_weekday=True), id="weekday-bool"),
        pytest.param(lambda case: case.pop("case_id"), id="missing-case-id"),
        pytest.param(lambda case: case["input"].pop("clock"), id="missing-input-field"),
        pytest.param(lambda case: case.update(unexpected=True), id="unknown-root-field"),
        pytest.param(
            lambda case: case["input"]["steps"][0].update(invented=True),
            id="unknown-operation-field",
        ),
        pytest.param(
            lambda case: case["input"]["steps"][0].pop("channel_id"),
            id="missing-operation-field",
        ),
        pytest.param(lambda case: case.update(expected={"steps": []}), id="bad-expected"),
    ],
)
def test_scheduler_replay_schema_gate_rejects_malformed_cases_before_repo(monkeypatch, mutate):
    case = _input_only_case()
    mutate(case)
    opened = False

    def reject_repo(*args, **kwargs):
        nonlocal opened
        opened = True
        raise AssertionError("Repo must not open")

    monkeypatch.setattr(replay, "Repo", reject_repo)
    with pytest.raises(validation.ContractError, match="schema validation failed"):
        replay.replay(case)
    assert not opened


@pytest.mark.parametrize("case", [[], "wrong-root"], ids=["array-root", "string-root"])
def test_scheduler_replay_schema_gate_rejects_wrong_root_before_repo(monkeypatch, case):
    monkeypatch.setattr(replay, "Repo", lambda *args, **kwargs: pytest.fail("Repo must not open"))
    with pytest.raises(validation.ContractError, match="schema validation failed"):
        replay.replay(case)


def test_scheduler_replay_reports_short_uuid_sequence_as_ephemeral_resource_failure():
    case = _input_only_case()
    case["input"]["uuid_sequence"] = [case["input"]["uuid_sequence"][0]]
    with pytest.raises(validation.ContractError, match="uuid_sequence exhausted.*ephemeral"):
        replay.replay(case)


def test_scheduler_generator_is_byte_stable_and_checked_in(tmp_path: Path):
    first, second = tmp_path / "first", tmp_path / "second"
    generator.write(first)
    generator.write(second)
    assert (first / "scheduler.json").read_bytes() == (second / "scheduler.json").read_bytes()
    assert generator.check(VECTOR_DIR)


@pytest.mark.parametrize(
    "mutate",
    [
        lambda document: document["cases"][0]["input"]["steps"][0].update(invented=True),
        lambda document: document["cases"][3]["input"]["steps"][0].update(status="unknown"),
        lambda document: document["cases"][1]["input"]["steps"][1].update(source="portal"),
        lambda document: document["cases"][0]["input"].update(reset_time="29:00:00"),
        lambda document: document["cases"][0]["input"]["steps"][0].update(time="29:00"),
        lambda document: document["cases"][0]["expected"]["final_state"]["fixed_runs"][0].update(
            weekday=7
        ),
        lambda document: document["cases"][0]["expected"]["final_state"]["fixed_runs"][0].update(
            time="29:00"
        ),
    ],
)
def test_scheduler_schema_rejects_unknown_keys_and_domains(mutate):
    document = _document()
    mutate(document)
    with pytest.raises(ValidationError):
        _validator().validate(document)


@pytest.mark.parametrize(
    "mutate",
    [
        lambda document: document["cases"][2]["input"]["steps"][4].update(fixed_key="missing"),
        lambda document: document["cases"][3]["input"]["steps"][1].update(run_key="missing"),
        lambda document: document["cases"][2]["input"]["steps"][3].update(
            run_key="weekly@2026-09-10T00:00:00+08:00"
        ),
        lambda document: document["cases"][2]["input"]["steps"][4]["changed"].pop(),
        lambda document: document["cases"][0]["expected"]["steps"].pop(),
        lambda document: document["cases"][4]["expected"]["steps"][1].clear(),
        lambda document: document["cases"][4]["input"]["steps"][1].pop("error_type"),
        lambda document: document["cases"][0]["input"]["steps"][0].update(owner_id=""),
        lambda document: document["cases"][2]["expected"]["steps"][3].update(value="planned"),
    ],
)
def test_scheduler_semantic_gate_rejects_cross_step_contract_defects(mutate):
    document = _document()
    mutate(document)
    with pytest.raises(ValueError):
        validation.validate_document(document)


def test_replay_rejects_static_invalid_references_before_opening_repo(monkeypatch):
    case = _document()["cases"][3]
    case["input"]["steps"][1]["run_key"] = "missing"
    opened = 0

    def reject_repo(*args, **kwargs):
        nonlocal opened
        opened += 1
        raise AssertionError("Repo must not open")

    monkeypatch.setattr(replay, "Repo", reject_repo)
    with pytest.raises(validation.ContractError, match="undefined run key"):
        replay.replay(case)
    assert opened == 0


@pytest.mark.parametrize(
    "field,value", [("clock", "2026-08-27T01:00:00"), ("ping_time", "29:00:00")]
)
def test_replay_rejects_invalid_time_inputs_before_opening_repo(monkeypatch, field, value):
    case = _document()["cases"][0]
    case["input"][field] = value
    monkeypatch.setattr(replay, "Repo", lambda *args: pytest.fail("Repo must not open"))
    with pytest.raises(validation.ContractError):
        replay.replay(case)


def test_replay_rejects_a_skipped_materialized_run_before_the_use():
    case = _document()["cases"][0]
    case["input"]["clock"] = "2026-09-01T12:00:00+08:00"
    case["input"]["steps"].append(
        {
            "op": "set_status",
            "run_key": "weekly@2026-08-27T00:00:00+08:00",
            "status": "done",
        }
    )
    with pytest.raises(validation.ContractError, match="materialization did not produce"):
        replay.replay(case)


def test_replay_rejects_a_retired_fixed_run_as_a_future_materialized_reference():
    case = _document()["cases"][0]
    case["input"]["steps"] = [
        case["input"]["steps"][0],
        {
            "op": "retire_fixed",
            "fixed_key": "weekly",
            "week_starts": ["2026-08-27T00:00:00+08:00"],
        },
        {"op": "materialise"},
        {
            "op": "set_status",
            "run_key": "weekly@2026-08-27T00:00:00+08:00",
            "status": "done",
        },
    ]
    with pytest.raises(validation.ContractError, match="undefined run key"):
        replay.replay(case)


def test_scheduler_snapshot_retains_direct_rsvp_source_and_time():
    adopted = next(
        case for case in _document()["cases"] if case["case_id"].startswith("adopt-manual")
    )
    rows = adopted["expected"]["final_state"]["rsvps"]
    assert {(row["user_id"], row["source"], row["at"]) for row in rows} == {
        ("1", "chat", "2026-08-26T17:00:00+00:00"),
        ("9", "slash", "2026-08-26T17:00:00+00:00"),
    }


def test_scheduler_check_detects_fixture_tampering(tmp_path: Path):
    generator.write(tmp_path)
    path = tmp_path / "scheduler.json"
    path.write_bytes(
        path.read_bytes().replace(b"materialise-repeat-idempotence", b"tampered-case-id")
    )
    assert not generator.check(tmp_path)
