"""Producer checks for the dispatch, mention and digest v5 scheduler vector families."""

from __future__ import annotations

import ast
import json
import logging
from copy import deepcopy
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator, FormatChecker, ValidationError

from bot.infrastructure import db
from bot.infrastructure.maintenance import coordinator
from bot.infrastructure.maintenance.delivery import journal
from scripts.v5_vectors.scheduler import delivery, generator, host_replay, validation
from scripts.v5_vectors.scheduler.digest import replay as digest_replay
from scripts.v5_vectors.scheduler.digest import validation as digest_validation
from scripts.v5_vectors.scheduler.dispatch import replay as dispatch_replay
from scripts.v5_vectors.scheduler.dispatch import validation as dispatch_validation
from scripts.v5_vectors.scheduler.mentions import replay as mention_replay
from scripts.v5_vectors.scheduler.mentions import validation as mention_validation

ROOT = Path(__file__).resolve().parents[3]
PYTHON_ROOT = ROOT / "legacy" / "python"
VECTOR_DIR = ROOT / "docs" / "v5" / "vectors" / "scheduler"
FAMILIES = {
    "dispatch": (dispatch_replay, dispatch_validation),
    "mentions": (mention_replay, mention_validation),
    "digest": (digest_replay, digest_validation),
}
DISPATCH_CASE = "classify-suppress-group-and-countdown"
MENTION_CASE = "audiences-per-post-kind"
DIGEST_CASE = "inclusion-excludes-cancelled-and-counts-at-risk"


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


def _repo_patch(family: str) -> tuple[object, str]:
    return (host_replay, "Repo") if family != "mentions" else (mention_replay, "Repo")


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


@pytest.mark.parametrize("family", FAMILIES)
def test_family_check_detects_expected_value_tampering(tmp_path: Path, family):
    generator.write(tmp_path)
    path = tmp_path / f"{family}.json"
    document = json.loads(path.read_text(encoding="utf-8"))
    document["cases"][0]["expected"]["steps"][0] = {"value": "tampered"}
    path.write_text(json.dumps(document, indent=2, ensure_ascii=False, sort_keys=True) + "\n")
    assert not generator.check(tmp_path)


def test_replay_restores_every_patched_alias_and_logging():
    originals = (journal.datetime, journal.uuid, coordinator.datetime, coordinator.uuid, db.uuid)
    disabled = logging.root.manager.disable
    dispatch_replay.replay(_input_only("dispatch", DISPATCH_CASE))
    assert (
        journal.datetime,
        journal.uuid,
        coordinator.datetime,
        coordinator.uuid,
        db.uuid,
    ) == originals
    assert logging.root.manager.disable == disabled


@pytest.mark.parametrize(
    "family,case_id,mutate",
    [
        pytest.param(
            "dispatch",
            DISPATCH_CASE,
            lambda case: case["input"]["steps"][7].update(kind="Day-Of"),
            id="dispatch-kind-pattern",
        ),
        pytest.param(
            "dispatch",
            DISPATCH_CASE,
            lambda case: case["input"].update(available_channel_ids=[222]),
            id="dispatch-channel-not-string",
        ),
        pytest.param(
            "dispatch",
            DISPATCH_CASE,
            lambda case: case["input"].pop("post_channel_id"),
            id="dispatch-missing-nullable-post-channel",
        ),
        pytest.param(
            "dispatch",
            DISPATCH_CASE,
            lambda case: case["input"]["steps"].append({"op": "tick"}),
            id="dispatch-unknown-operation",
        ),
        pytest.param(
            "dispatch",
            DISPATCH_CASE,
            lambda case: case["input"]["steps"].append({"op": "set_transport", "fail_sends": 1}),
            id="dispatch-transport-flag-type",
        ),
        pytest.param(
            "mentions",
            MENTION_CASE,
            lambda case: case["input"]["steps"][0].update(candidates="1,2"),
            id="mentions-candidates-type",
        ),
        pytest.param(
            "mentions",
            MENTION_CASE,
            lambda case: case["input"]["steps"][0].pop("candidates"),
            id="mentions-audience-missing-nullable",
        ),
        pytest.param(
            "mentions",
            MENTION_CASE,
            lambda case: case["input"]["members"][0].update(ping_level="loud"),
            id="mentions-member-level-domain",
        ),
        pytest.param(
            "mentions",
            MENTION_CASE,
            lambda case: case["input"]["steps"][4]["rsvps"].append(["1", "later"]),
            id="mentions-rsvp-state-domain",
        ),
        pytest.param(
            "mentions",
            MENTION_CASE,
            lambda case: case["input"].update(uuid_sequence=["x"]),
            id="mentions-no-uuid-input",
        ),
        pytest.param(
            "digest",
            DIGEST_CASE,
            lambda case: case["input"]["steps"].append(
                {"op": "set_config", "key": "paused", "value": "1"}
            ),
            id="digest-config-key-domain",
        ),
        pytest.param(
            "digest",
            DIGEST_CASE,
            lambda case: case["input"]["steps"][-1].update(week="next"),
            id="digest-unmodelled-argument",
        ),
        pytest.param(
            "digest",
            DIGEST_CASE,
            lambda case: case["input"].update(reset_time="24:00:00"),
            id="digest-reset-time-format",
        ),
    ],
)
def test_schema_gate_rejects_malformed_inputs_before_repo(monkeypatch, family, case_id, mutate):
    replay, _ = FAMILIES[family]
    case = _input_only(family, case_id)
    mutate(case)
    module, name = _repo_patch(family)
    monkeypatch.setattr(module, name, lambda *a, **k: pytest.fail("Repo must not open"))
    with pytest.raises(validation.ContractError, match="schema validation failed"):
        replay.replay(case)


@pytest.mark.parametrize(
    "family,case_id,mutate,message",
    [
        (
            "dispatch",
            DISPATCH_CASE,
            lambda case: case["input"]["steps"][7].update(run_key="missing"),
            "undefined run key",
        ),
        (
            "dispatch",
            DISPATCH_CASE,
            lambda case: case["input"]["steps"][7].update(fire_at="2026-08-31T09:00:00"),
            "aware ISO datetime",
        ),
        (
            "mentions",
            MENTION_CASE,
            lambda case: case["input"]["members"].append(deepcopy(case["input"]["members"][0])),
            "unique",
        ),
        (
            "digest",
            DIGEST_CASE,
            lambda case: case["input"]["steps"][6].update(run_key="star@not-a-week"),
            "aware ISO datetime",
        ),
        (
            "digest",
            DIGEST_CASE,
            lambda case: case["input"].update(timezone="Mars/Olympus"),
            "unknown timezone",
        ),
    ],
)
def test_semantic_gate_rejects_bad_references_before_repo(
    monkeypatch, family, case_id, mutate, message
):
    replay, _ = FAMILIES[family]
    case = _input_only(family, case_id)
    mutate(case)
    module, name = _repo_patch(family)
    monkeypatch.setattr(module, name, lambda *a, **k: pytest.fail("Repo must not open"))
    with pytest.raises(validation.ContractError, match=message):
        replay.replay(case)


@pytest.mark.parametrize(
    "family,case_id,step,value",
    [
        ("dispatch", DISPATCH_CASE, 16, [{"id": "x"}]),
        ("dispatch", DISPATCH_CASE, 17, []),
        ("mentions", MENTION_CASE, 0, ["2"]),
        ("mentions", "allow-list-gate-and-digest-empty-list", 0, {"users": ["1"]}),
        ("digest", DIGEST_CASE, 12, 700000000000000001),
        ("digest", "previous-digest-retired-but-kept-as-log", 4, -1),
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
    "family,case_id",
    [("dispatch", DISPATCH_CASE), ("digest", DIGEST_CASE)],
)
def test_short_uuid_sequence_is_an_ephemeral_contract_failure(family, case_id):
    replay, _ = FAMILIES[family]
    case = _input_only(family, case_id)
    case["input"]["uuid_sequence"] = case["input"]["uuid_sequence"][:2]
    with pytest.raises(validation.ContractError, match="uuid_sequence exhausted.*ephemeral"):
        replay.replay(case)


def test_mentions_consume_no_uuid():
    for case in _document("mentions")["cases"]:
        assert "uuid_sequence" not in case["input"]


def test_an_undeclared_mention_error_is_not_captured():
    case = _input_only("mentions", "level-changes-and-refusals")
    del case["input"]["steps"][6]["error_type"]
    with pytest.raises(KeyError, match="404"):
        mention_replay.replay(case)


def test_digest_inclusion_rejects_an_unrecognised_summary():
    embed = {"description": "**2** things", "fields": [{"name": "Mon", "value": "`#0000000a`"}]}
    with pytest.raises(validation.ContractError, match="unrecognised digest summary"):
        delivery.digest_inclusion([embed])


def test_digest_intents_never_mention_anyone():
    for case in _document("digest")["cases"]:
        for intent in case["expected"]["final_state"]["side_effects"]:
            assert intent["mentions"] == [] and intent["effect_kind"] == "digest"


def test_unavailable_home_channel_reminders_are_lost_as_recorded_v4_quirk():
    """Pinned v4 behaviour, escalated rather than fixed: fallback sends never bind."""
    case = _case("dispatch", "unavailable-home-falls-back-then-binding-refuses")
    state = case["expected"]["final_state"]
    assert {intent["outcome"] for intent in state["side_effects"]} == {
        "raised:DeliveryBindingError"
    }
    assert {intent["channel_id"] for intent in state["side_effects"]} == {"555"}
    assert all(row["message_id"] is None for row in state["reminders"])
    assert all(row["sent_at"] is not None for row in state["reminders"])


def test_dispatch_schema_rejects_an_intent_without_outcome():
    document = _document("dispatch")
    document["cases"][0]["expected"]["final_state"]["side_effects"][0].pop("outcome")
    with pytest.raises(ValidationError):
        _validator("dispatch").validate(document)
