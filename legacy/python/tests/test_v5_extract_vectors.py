"""Producer checks for the v5 extraction vector families."""

from __future__ import annotations

import importlib
import json
from copy import deepcopy
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator, FormatChecker

from scripts.v5_vectors.extract import contract, generator

ROOT = Path(__file__).resolve().parents[3]
PYTHON_ROOT = ROOT / "legacy" / "python"
VECTOR_DIR = ROOT / "docs" / "v5" / "vectors" / "extract"
FAMILIES = {family.name: family for family in generator.FAMILIES}


def _document(name: str) -> dict:
    return json.loads((VECTOR_DIR / f"{name}.json").read_text(encoding="utf-8"))


def _input_only(case: dict) -> dict:
    return {"case_id": case["case_id"], "input": deepcopy(case["input"])}


@pytest.mark.parametrize("name", FAMILIES)
def test_checked_in_vectors_validate_and_replay_from_json(name):
    family = FAMILIES[name]
    document = _document(name)
    schema = json.loads((VECTOR_DIR / family.schema_file).read_text(encoding="utf-8"))
    Draft202012Validator(schema, format_checker=FormatChecker()).validate(document)
    contract.validate_document(family, document)
    for case in document["cases"]:
        replayed = json.loads(json.dumps(family.replay(_input_only(case))))
        assert replayed == case["expected"], case["case_id"]


@pytest.mark.parametrize("name", FAMILIES)
def test_replay_is_identical_across_two_clean_runs(name):
    family = FAMILIES[name]
    for case in _document(name)["cases"]:
        first = json.dumps(family.replay(_input_only(case)), sort_keys=True)
        second = json.dumps(family.replay(_input_only(case)), sort_keys=True)
        assert first == second, case["case_id"]


def test_generator_is_byte_stable_and_matches_the_checked_in_files(tmp_path: Path):
    first, second = tmp_path / "first", tmp_path / "second"
    generator.write(first)
    generator.write(second)
    written = {p.name: p.read_bytes() for p in first.iterdir()}
    assert written == {p.name: p.read_bytes() for p in second.iterdir()}
    assert generator.check(VECTOR_DIR)
    assert set(written) <= {p.name for p in VECTOR_DIR.iterdir()}


def test_index_lists_every_family_schema_and_vector():
    index = json.loads((VECTOR_DIR / "index.json").read_text(encoding="utf-8"))
    entries = {entry["family"]: entry for entry in index["families"]}
    assert set(entries) == set(FAMILIES)
    for name, entry in entries.items():
        assert (VECTOR_DIR / entry["schema"]).is_file()
        assert _document(name)["family"] == name == entry["vector"].removesuffix(".json")


@pytest.mark.parametrize("name", FAMILIES)
def test_check_detects_expected_value_tampering(tmp_path: Path, name):
    generator.write(tmp_path)
    path = tmp_path / f"{name}.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["cases"][0]["expected"]["steps"][0] = {"value": "tampered"}
    path.write_text(json.dumps(document, indent=2, ensure_ascii=False, sort_keys=True) + "\n")
    assert not generator.check(tmp_path)


def test_check_detects_schema_tampering(tmp_path: Path):
    generator.write(tmp_path)
    path = tmp_path / "gate.schema.json"
    schema = json.loads(path.read_text(encoding="utf-8"))
    schema["title"] = "tampered"
    path.write_text(json.dumps(schema, indent=2, sort_keys=True) + "\n")
    assert not generator.check(tmp_path)


@pytest.mark.parametrize(
    ("name", "mutate"),
    [
        pytest.param(
            "gate",
            lambda case: case["input"]["steps"][0].update(texts="not a list"),
            id="unknown-step-field",
        ),
        pytest.param(
            "resolve",
            lambda case: case["input"]["steps"][0].update(op="guess"),
            id="unknown-op",
        ),
        pytest.param(
            "plan", lambda case: case["input"]["steps"][0].pop("now"), id="missing-pinned-now"
        ),
        pytest.param(
            "commit",
            lambda case: case["input"].update(clock="2026-08-27T01:00:00"),
            id="naive-clock",
        ),
        pytest.param(
            "parse",
            lambda case: case["input"]["steps"][0].update(raw=None),
            id="non-string-response",
        ),
    ],
)
def test_input_gate_refuses_malformed_cases_before_replay(name, mutate):
    case = _input_only(_document(name)["cases"][0])
    mutate(case)
    with pytest.raises(contract.ContractError, match="schema validation failed"):
        FAMILIES[name].replay(case)


def test_document_gate_pairs_errors_with_declarations():
    family = FAMILIES["window"]
    document = _document("window")
    case = next(c for c in document["cases"] if c["case_id"] == "rescan-window-bounds")
    step = next(s for s in case["input"]["steps"] if "error_type" in s)
    step.pop("error_type")
    with pytest.raises(contract.ContractError, match="undeclared error type"):
        contract.validate_document(family, document)


@pytest.mark.parametrize("name", FAMILIES)
def test_provenance_points_at_real_code_and_tests(name):
    provenance = _document(name)["provenance"]
    for reference in provenance["source_tests"]:
        assert (PYTHON_ROOT / reference).is_file(), reference
    for dotted in provenance["functions"]:
        module_name, _, attribute = dotted.rpartition(".")
        while module_name:
            try:
                target = importlib.import_module(module_name)
                break
            except ModuleNotFoundError:
                module_name, _, outer = module_name.rpartition(".")
                attribute = f"{outer}.{attribute}"
        for part in attribute.split("."):
            target = getattr(target, part)
