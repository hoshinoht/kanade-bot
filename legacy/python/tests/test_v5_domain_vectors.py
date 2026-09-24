from __future__ import annotations

import ast
import json
from copy import deepcopy
from pathlib import Path

import jsonschema
import pytest

from scripts.v5_vectors import generator, replay

VECTOR_DIR = Path(__file__).resolve().parents[3] / "docs" / "v5" / "vectors" / "domain"
PYTHON_ROOT = Path(__file__).resolve().parents[1]


def _schema() -> dict:
    return json.loads((VECTOR_DIR / "schema.json").read_text(encoding="utf-8"))


def test_checked_in_domain_vectors_validate_against_the_versioned_schema():
    schema = _schema()
    index = json.loads((VECTOR_DIR / "index.json").read_text(encoding="utf-8"))
    for name in index["vectors"]:
        jsonschema.validate(json.loads((VECTOR_DIR / name).read_text(encoding="utf-8")), schema)


def test_every_checked_in_case_replays_from_its_serialized_input_and_fixtures():
    index = json.loads((VECTOR_DIR / "index.json").read_text(encoding="utf-8"))
    for name in index["vectors"]:
        document = json.loads((VECTOR_DIR / name).read_text(encoding="utf-8"))
        for case in document["cases"]:
            assert generator._expected(case, document.get("fixtures", {})) == case["expected"]


def test_catalog_replays_after_sorted_json_object_reserialization():
    document = json.loads((VECTOR_DIR / "bosses.json").read_text(encoding="utf-8"))
    reordered = json.loads(json.dumps(document, sort_keys=True))
    for case in reordered["cases"]:
        assert generator._expected(case, reordered["fixtures"]) == case["expected"]


def test_provenance_test_references_name_real_test_functions():
    index = json.loads((VECTOR_DIR / "index.json").read_text(encoding="utf-8"))
    for name in index["vectors"]:
        document = json.loads((VECTOR_DIR / name).read_text(encoding="utf-8"))
        for reference in document["provenance"]["source_tests"]:
            path_text, function_name = reference.split("::", maxsplit=1)
            tree = ast.parse((PYTHON_ROOT / path_text).read_text(encoding="utf-8"))
            assert function_name in {
                node.name for node in ast.walk(tree) if isinstance(node, ast.FunctionDef)
            }


def test_generator_is_byte_stable_and_matches_checked_in_oracle_vectors(tmp_path: Path):
    first = tmp_path / "first"
    second = tmp_path / "second"
    generator.write(first)
    generator.write(second)
    assert {path.name: path.read_bytes() for path in first.iterdir()} == {
        path.name: path.read_bytes() for path in second.iterdir()
    }
    assert {path.name: path.read_bytes() for path in first.iterdir()} == {
        path.name: path.read_bytes()
        for path in VECTOR_DIR.glob("*.json")
        if path.name not in {"schema.json", "index.json"}
    }


def test_check_detects_an_expected_value_tamper(tmp_path: Path):
    generator.write(tmp_path)
    path = tmp_path / "ids.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["cases"][0]["expected"]["value"] = "tampered"
    path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    assert not generator.check(tmp_path)


@pytest.mark.parametrize(
    "mutate",
    [
        lambda document: document.update(family="unknown"),
        lambda document: document["cases"][0].update(op="unknown"),
        lambda document: document["cases"][0]["input"].pop("value"),
        lambda document: document["cases"][0]["input"].update(value=1),
        lambda document: document["cases"][0]["input"].update(unexpected="x"),
        lambda document: document["cases"][0]["expected"].update(
            error={"type": "x", "message": "x"}
        ),
        lambda document: document["cases"][0]["expected"].update(unexpected=True),
    ],
)
def test_schema_rejects_wrong_family_operation_input_and_result_shapes(mutate):
    document = json.loads((VECTOR_DIR / "ids.json").read_text(encoding="utf-8"))
    mutate(document)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(document, _schema())


@pytest.mark.parametrize(
    "mutate",
    [
        lambda catalog: catalog["difficulties"].__setitem__(
            1, deepcopy(catalog["difficulties"][0])
        ),
        lambda catalog: catalog["difficulties"][0].update(unexpected=True),
    ],
)
def test_schema_rejects_duplicate_or_unknown_ordered_catalog_entries(mutate):
    document = json.loads((VECTOR_DIR / "bosses.json").read_text(encoding="utf-8"))
    catalog = document["fixtures"]["catalogs"]["synthetic-bosses-v1"]
    mutate(catalog)
    with pytest.raises(jsonschema.ValidationError):
        jsonschema.validate(document, _schema())


@pytest.mark.parametrize("declared", sorted(generator.ERROR_TYPES))
def test_schema_binds_declared_and_expected_error_types(declared):
    validator = jsonschema.Draft202012Validator(_schema())
    document = next(
        document
        for document in generator.documents().values()
        if any(case.get("error_type") == declared for case in document["cases"])
    )
    case = next(case for case in document["cases"] if case.get("error_type") == declared)
    document["cases"] = [case]
    validator.validate(document)
    for mismatched in (*sorted(generator.ERROR_TYPES), "UnexpectedError"):
        if mismatched == declared:
            continue
        case["expected"]["error"]["type"] = mismatched
        with pytest.raises(jsonschema.ValidationError):
            validator.validate(document)


def test_positive_case_does_not_swallow_an_unexpected_value_error(monkeypatch):
    original = replay.dispatch

    def fail_positive(case, fixtures):
        if case["case_id"] == "ids.canonical.decorated":
            raise ValueError("unexpected")
        return original(case, fixtures)

    monkeypatch.setattr(replay, "dispatch", fail_positive)
    with pytest.raises(ValueError, match="unexpected"):
        generator.documents()


def test_negative_case_must_raise_its_declared_exception(monkeypatch):
    original = replay.dispatch

    def succeed_negative(case, fixtures):
        if case["case_id"] == "ids.resolve.not-found":
            return "unexpected success"
        return original(case, fixtures)

    monkeypatch.setattr(replay, "dispatch", succeed_negative)
    with pytest.raises(AssertionError, match="expected IdNotFound but succeeded"):
        generator.documents()
