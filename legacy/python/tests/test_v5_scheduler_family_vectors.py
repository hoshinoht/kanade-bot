"""Producer checks for the reminder and mutation v5 scheduler vector families."""

from __future__ import annotations

import ast
import asyncio
import json
from copy import deepcopy
from pathlib import Path
from types import SimpleNamespace

import pytest
from jsonschema import Draft202012Validator, FormatChecker, ValidationError

from scripts.v5_vectors.scheduler import generator, validation
from scripts.v5_vectors.scheduler.mutations import host as mutation_host
from scripts.v5_vectors.scheduler.mutations import replay as mutation_replay
from scripts.v5_vectors.scheduler.mutations import validation as mutation_validation
from scripts.v5_vectors.scheduler.reminders import replay as reminder_replay
from scripts.v5_vectors.scheduler.reminders import validation as reminder_validation

ROOT = Path(__file__).resolve().parents[3]
PYTHON_ROOT = ROOT / "legacy" / "python"
VECTOR_DIR = ROOT / "docs" / "v5" / "vectors" / "scheduler"
FAMILIES = {
    "reminders": (reminder_replay, reminder_validation),
    "mutations": (mutation_replay, mutation_validation),
}


def _document(family: str) -> dict:
    return json.loads((VECTOR_DIR / f"{family}.json").read_text(encoding="utf-8"))


def _case(family: str, case_id: str) -> dict:
    return next(case for case in _document(family)["cases"] if case["case_id"] == case_id)


def _input_only(family: str, case_id: str) -> dict:
    case = _case(family, case_id)
    return {"case_id": case["case_id"], "input": deepcopy(case["input"])}


def _validator(family: str) -> Draft202012Validator:
    schema = json.loads((VECTOR_DIR / f"{family}.schema.json").read_text(encoding="utf-8"))
    return Draft202012Validator(schema, format_checker=FormatChecker())


@pytest.mark.parametrize("family", FAMILIES)
def test_family_vectors_validate_and_replay_from_json(family):
    replay, gate = FAMILIES[family]
    document = _document(family)
    _validator(family).validate(document)
    gate.validate_document(document)
    for case in document["cases"]:
        assert replay.replay(json.loads(json.dumps(case))) == case["expected"]


@pytest.mark.parametrize("family", FAMILIES)
def test_family_replay_is_identical_for_two_clean_stores(family):
    replay, _ = FAMILIES[family]
    for case in _document(family)["cases"]:
        stripped = {"case_id": case["case_id"], "input": case["input"]}
        assert replay.replay(stripped) == replay.replay(stripped) == case["expected"]


def test_index_lists_every_generated_scheduler_family(tmp_path: Path):
    index = json.loads((VECTOR_DIR / "index.json").read_text(encoding="utf-8"))
    generator.write(tmp_path)
    assert {entry["vector"] for entry in index["families"]} == {
        path.name for path in tmp_path.iterdir()
    }
    for entry in index["families"]:
        schema = json.loads((VECTOR_DIR / entry["schema"]).read_text(encoding="utf-8"))
        document = json.loads((VECTOR_DIR / entry["vector"]).read_text(encoding="utf-8"))
        assert document["family"] == entry["family"]
        Draft202012Validator(schema, format_checker=FormatChecker()).validate(document)


def test_generator_is_byte_stable_for_every_family(tmp_path: Path):
    first, second = tmp_path / "first", tmp_path / "second"
    generator.write(first)
    generator.write(second)
    assert {p.name: p.read_bytes() for p in first.iterdir()} == {
        p.name: p.read_bytes() for p in second.iterdir()
    }
    assert generator.check(VECTOR_DIR)


@pytest.mark.parametrize("family", FAMILIES)
def test_family_check_detects_expected_value_tampering(tmp_path: Path, family):
    generator.write(tmp_path)
    path = tmp_path / f"{family}.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["cases"][0]["expected"]["steps"][0] = {"value": "tampered"}
    path.write_text(json.dumps(document, indent=2, ensure_ascii=False, sort_keys=True) + "\n")
    assert not generator.check(tmp_path)


@pytest.mark.parametrize("family", FAMILIES)
def test_provenance_names_real_test_functions(family):
    for reference in _document(family)["provenance"]["source_tests"]:
        path_text, name = reference.split("::", maxsplit=1)
        tree = ast.parse((PYTHON_ROOT / path_text).read_text(encoding="utf-8"))
        assert name in {
            node.name
            for node in ast.walk(tree)
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        }, reference


REMINDER_CASE = "rebuild-deletes-sent-rows-and-duplicate-add-is-ignored"
MUTATION_CASE = "swap-errors-and-rsvp-recompute"


@pytest.mark.parametrize(
    "family,case_id,mutate",
    [
        pytest.param(
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"][1].update(countdowns=["60"]),
            id="reminders-countdown-string",
        ),
        pytest.param(
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"][1].update(rebuild="no"),
            id="reminders-rebuild-string",
        ),
        pytest.param(
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"][1].update(ping_time="9:00"),
            id="reminders-ping-time-format",
        ),
        pytest.param(
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"][2].update(kind="morning"),
            id="reminders-unknown-kind",
        ),
        pytest.param(
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"][2].pop("sent_at"),
            id="reminders-missing-nullable-field",
        ),
        pytest.param(
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"].append({"op": "tick"}),
            id="reminders-unknown-operation",
        ),
        pytest.param(
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"][0].update(status="archived"),
            id="reminders-run-status-domain",
        ),
        pytest.param(
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"].update(ping_time="09:00:00"),
            id="reminders-unknown-input-field",
        ),
        pytest.param(
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["steps"][4].pop("mark"),
            id="mutations-missing-mark",
        ),
        pytest.param(
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["steps"][4].update(error_type="ValueError"),
            id="mutations-undeclared-error-class",
        ),
        pytest.param(
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["members"][0].update(ping_level="loud"),
            id="mutations-ping-level-domain",
        ),
        pytest.param(
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["catalog"]["bosses"][0].update(portrait="x.png"),
            id="mutations-catalog-extra-key",
        ),
        pytest.param(
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["steps"].append(
                {
                    "op": "set_status",
                    "run_key": "weekly@2026-08-27T00:00:00+08:00",
                    "status": "at_risk",
                    "announce": True,
                    "mark": True,
                }
            ),
            id="mutations-unsettable-status-without-error",
        ),
        pytest.param(
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["steps"].append(
                {"op": "update_fixed", "fixed_key": "weekly", "changes": {"weekday": 2}}
            ),
            id="mutations-unknown-fixed-change",
        ),
        pytest.param(
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"].update(watched_channel_ids=[222]),
            id="mutations-channel-not-string",
        ),
    ],
)
def test_family_schema_gate_rejects_malformed_inputs_before_repo(
    monkeypatch, family, case_id, mutate
):
    replay, _ = FAMILIES[family]
    case = _input_only(family, case_id)
    mutate(case)
    monkeypatch.setattr(replay, "Repo", lambda *a, **k: pytest.fail("Repo must not open"))
    with pytest.raises(validation.ContractError, match="schema validation failed"):
        replay.replay(case)


@pytest.mark.parametrize(
    "family,case_id,mutate,message",
    [
        (
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"][1].update(run_key="missing"),
            "undefined run key",
        ),
        (
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"].append(deepcopy(case["input"]["steps"][0])),
            "not unique",
        ),
        (
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"].update(timezone="Mars/Olympus"),
            "unknown timezone",
        ),
        (
            "reminders",
            REMINDER_CASE,
            lambda case: case["input"]["steps"][2].update(fire_at="2026-08-31T08:00:00"),
            "aware ISO datetime",
        ),
        (
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["steps"][4].update(run_key="weekly@not-a-week"),
            "aware ISO datetime",
        ),
        (
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["steps"].append(
                {"op": "update_fixed", "fixed_key": "missing", "changes": {"note": "x"}}
            ),
            "undefined fixed key",
        ),
        (
            "mutations",
            MUTATION_CASE,
            lambda case: case["input"]["members"].append(deepcopy(case["input"]["members"][0])),
            "unique",
        ),
    ],
)
def test_family_semantic_gate_rejects_bad_references_before_repo(
    monkeypatch, family, case_id, mutate, message
):
    replay, _ = FAMILIES[family]
    case = _input_only(family, case_id)
    mutate(case)
    monkeypatch.setattr(replay, "Repo", lambda *a, **k: pytest.fail("Repo must not open"))
    with pytest.raises(validation.ContractError, match=message):
        replay.replay(case)


@pytest.mark.parametrize(
    "family,case_id,step,value",
    [
        ("reminders", "is-stale-strict-grace-boundaries", 0, "false"),
        ("reminders", "reconcile-day-of-and-reschedule-unposted", 10, -1),
        ("reminders", REMINDER_CASE, 1, ["morning"]),
        ("reminders", REMINDER_CASE, 4, "done"),
        ("mutations", MUTATION_CASE, 1, []),
        ("mutations", MUTATION_CASE, 4, {"id": "x", "status": "planned"}),
    ],
)
def test_completed_gate_types_each_result_by_its_operation(family, case_id, step, value):
    _, gate = FAMILIES[family]
    document = _document(family)
    case = next(item for item in document["cases"] if item["case_id"] == case_id)
    case["expected"]["steps"][step] = {"value": value}
    with pytest.raises(validation.ContractError):
        gate.validate_document(document)


@pytest.mark.parametrize(
    "family,case_id,mutate",
    [
        ("reminders", REMINDER_CASE, lambda case: case["expected"]["steps"].pop()),
        (
            "reminders",
            "negative-unknown-run-status-rejected-by-repo",
            lambda case: case["input"]["steps"][1].update(status="otot") or None,
        ),
        (
            "mutations",
            MUTATION_CASE,
            lambda case: (
                case["expected"]["steps"][7].update(error={"type": "BadRequest", "message": "x"})
                or case["input"]["steps"][7].pop("error_type")
            ),
        ),
    ],
)
def test_completed_gate_rejects_cross_step_contract_defects(family, case_id, mutate):
    _, gate = FAMILIES[family]
    document = _document(family)
    mutate(next(item for item in document["cases"] if item["case_id"] == case_id))
    with pytest.raises(ValueError):
        gate.validate_document(document)


@pytest.mark.parametrize("family", FAMILIES)
def test_short_uuid_sequence_is_an_ephemeral_contract_failure(family):
    replay, _ = FAMILIES[family]
    case_id = REMINDER_CASE if family == "reminders" else MUTATION_CASE
    case = _input_only(family, case_id)
    case["input"]["uuid_sequence"] = case["input"]["uuid_sequence"][:2]
    with pytest.raises(validation.ContractError, match="uuid_sequence exhausted.*ephemeral"):
        replay.replay(case)


def test_an_undeclared_error_is_not_captured():
    case = _input_only("mutations", MUTATION_CASE)
    del case["input"]["steps"][7]["error_type"]
    with pytest.raises(Exception, match="not on this run") as caught:
        mutation_replay.replay(case)
    assert type(caught.value).__name__ == "BadRequest"


def test_host_records_unmodelled_post_arguments():
    host = mutation_host.Host.__new__(mutation_host.Host)
    host.effects, host.unexpected = [], []
    asyncio.run(
        host.post_plain(
            SimpleNamespace(id="222"),
            "text",
            ["1"],
            effect_kind="k",
            effect_context=(),
            silent=True,
        )
    )
    assert host.unexpected == ["post_plain received unmodelled arguments ['silent']"]


def test_spring_forward_gap_ping_lands_at_or_after_run_start_as_recorded_v4_quirk():
    """Pinned v4 behaviour, escalated rather than fixed: a day-of ping inside a DST gap."""
    state = _case("reminders", "dst-spring-forward-gap-ping-and-fixed-slots")["expected"][
        "final_state"
    ]
    starts = {run["id"]: run["datetime"] for run in state["runs"]}
    day_of = {
        row["run_id"]: row["fire_at"] for row in state["reminders"] if row["kind"] == "day_of"
    }
    assert {run_id: (starts[run_id], day_of[run_id]) for run_id in starts} == {
        "00000000-0000-4000-8001-000000000003": (
            "2026-03-08T07:30:00+00:00",
            "2026-03-08T07:30:00+00:00",
        ),
        "00000000-0000-4000-8001-000000000004": (
            "2026-03-08T07:00:00+00:00",
            "2026-03-08T07:30:00+00:00",
        ),
    }


def test_mutation_json_schema_rejects_effect_without_kind():
    document = _document("mutations")
    document["cases"][0]["expected"]["final_state"]["side_effects"][0].pop("effect_kind")
    with pytest.raises(ValidationError):
        _validator("mutations").validate(document)
