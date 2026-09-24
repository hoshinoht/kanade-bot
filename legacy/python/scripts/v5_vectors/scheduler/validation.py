"""Canonical schema and cross-step constraints for scheduler vectors."""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from jsonschema import Draft202012Validator, FormatChecker, SchemaError, ValidationError

SCHEMA_PATH = (
    Path(__file__).resolve().parents[5] / "docs" / "v5" / "vectors" / "scheduler" / "schema.json"
)


class ContractError(ValueError):
    """A malformed vector contract, detected before an oracle operation."""


def _fail(case_id: str, message: str) -> None:
    raise ContractError(f"{case_id}: {message}")


def _load_schema() -> dict[str, Any]:
    try:
        schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ContractError(
            f"scheduler schema could not be loaded from {SCHEMA_PATH}: {exc}"
        ) from None
    if not isinstance(schema, dict):
        raise ContractError(f"scheduler schema at {SCHEMA_PATH} must be a JSON object")
    try:
        Draft202012Validator.check_schema(schema)
    except SchemaError as exc:
        raise ContractError(
            f"scheduler schema at {SCHEMA_PATH} is invalid: {exc.message}"
        ) from None
    return schema


def _schema_validator(target: str | None) -> Draft202012Validator:
    schema = _load_schema()
    if target is not None:
        try:
            schema = {
                "$schema": schema["$schema"],
                "$defs": schema["$defs"],
                "$ref": f"#/$defs/{target}",
            }
        except KeyError as exc:
            raise ContractError(
                f"scheduler schema at {SCHEMA_PATH} is missing {exc.args[0]!r}"
            ) from None
    try:
        return Draft202012Validator(schema, format_checker=FormatChecker())
    except SchemaError as exc:
        raise ContractError(
            f"scheduler schema at {SCHEMA_PATH} is invalid: {exc.message}"
        ) from None


def _schema_path(error: ValidationError) -> str:
    path = "$"
    for part in error.absolute_path:
        path += f"[{part}]" if isinstance(part, int) else f".{part}"
    return path


def _schema_context(instance: object) -> str:
    if isinstance(instance, dict) and "case_id" in instance:
        return f"{instance['case_id']}"
    return "scheduler document"


def _validate_schema(instance: object, target: str | None) -> None:
    try:
        _schema_validator(target).validate(instance)
    except ValidationError as exc:
        raise ContractError(
            f"{_schema_context(instance)}: schema validation failed at "
            f"{_schema_path(exc)} [{exc.validator}]: {exc.message}"
        ) from None


def _aware(case_id: str, field: str, value: object) -> None:
    try:
        parsed = datetime.fromisoformat(str(value))
    except ValueError:
        _fail(case_id, f"{field} must be an aware ISO datetime")
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        _fail(case_id, f"{field} must be an aware ISO datetime")


def _run_reference(case_id: str, key: str, runs: set[str], possible: set[str]) -> None:
    if key not in runs | possible:
        _fail(case_id, f"undefined run key {key!r}")


def _validate_input_semantics(case: dict[str, Any]) -> None:
    """Check parseable inputs and static references before opening a repository.

    Materialized keys are only *possible* here. Replay records them as usable
    only after the v4 oracle actually returns a run for that fixed/week pair.
    """
    case_id, input_ = case["case_id"], case["input"]
    _aware(case_id, "clock", input_["clock"])
    _aware(case_id, "week_start", input_["week_start"])
    try:
        ZoneInfo(input_["timezone"])
    except ZoneInfoNotFoundError:
        _fail(case_id, f"unknown timezone {input_['timezone']!r}")

    fixed, runs, possible = set(), set(), set()
    for step in input_["steps"]:
        op = step["op"]
        if op == "add_fixed":
            key = step["fixed_key"]
            if key in fixed:
                _fail(case_id, f"add_fixed key {key!r} is not unique")
            fixed.add(key)
        elif op == "create_run":
            key = step["run_key"]
            if key in runs:
                _fail(case_id, f"create_run key {key!r} is not unique")
            _aware(case_id, "create_run at", step["at"])
            runs.add(key)
        elif op == "materialise":
            start = step.get("week_start", input_["week_start"])
            _aware(case_id, "materialise week_start", start)
            possible.update(f"{key}@{start}" for key in fixed)
        elif op in {"set_status", "set_rsvp", "reaction"}:
            _run_reference(case_id, step["run_key"], runs, possible)
        elif op in {"edit_fixed", "retire_fixed"}:
            if step["fixed_key"] not in fixed:
                _fail(case_id, f"undefined fixed key {step['fixed_key']!r}")
            for start in step["week_starts"]:
                _aware(case_id, f"{op} week_start", start)
            if op == "edit_fixed":
                if set(step["fields"]) != set(step["changed"]):
                    _fail(case_id, "edit_fixed fields and changed must match exactly")
            else:
                fixed.remove(step["fixed_key"])
        else:
            _fail(case_id, f"unknown operation {op!r}")


def validate_input_case(case: dict[str, Any]) -> None:
    """Public pre-write gate for a replayable input case."""
    _validate_schema(case, "replayCase")
    _validate_input_semantics(case)


def _validate_completed_semantics(case: dict[str, Any]) -> None:
    case_id, expected, steps = case["case_id"], case["expected"], case["input"]["steps"]
    if len(expected["steps"]) != len(steps):
        _fail(case_id, "expected steps must match input steps")
    for step, result in zip(steps, expected["steps"], strict=True):
        if set(result) == {"error"}:
            if step.get("error_type") != result["error"]["type"]:
                _fail(case_id, "error type must match the step declaration")
        elif set(result) == {"value"}:
            if "error_type" in step:
                _fail(case_id, "a failing step cannot have a success value")
            _validate_value(case_id, step, result["value"])
        else:
            _fail(case_id, "each expected step needs exactly one value or error")
    _validate_final_state(case_id, expected["final_state"])


def validate_case(case: dict[str, Any]) -> None:
    """Validate a completed vector after its oracle expectations are attached."""
    _validate_schema(case, "case")
    _validate_input_semantics(case)
    _validate_completed_semantics(case)


def _validate_value(case_id: str, step: dict[str, Any], value: Any) -> None:
    op = step["op"]
    if op in {"add_fixed", "create_run"} and not isinstance(value, str):
        _fail(case_id, f"{op} result must be an ID")
    if op == "materialise" and not (
        isinstance(value, list) and all(isinstance(item, str) for item in value)
    ):
        _fail(case_id, "materialise result must be an ordered ID array")
    if op in {"set_status", "set_rsvp"} and value != step.get("status", step.get("state")):
        _fail(case_id, f"{op} result must equal the requested value")
    if op in {"edit_fixed", "retire_fixed"} and not isinstance(value, int):
        _fail(case_id, f"{op} result must be a count")
    if op == "reaction" and (
        not isinstance(value, dict)
        or set(value) != {"run_id", "applied", "state", "old_status", "new_status"}
    ):
        _fail(case_id, "reaction result must be a typed reaction result")


def _validate_final_state(case_id: str, state: dict[str, Any]) -> None:
    runs, fixed = state["runs"], state["fixed_runs"]
    run_ids, fixed_ids = {row["id"] for row in runs}, {row["id"] for row in fixed}
    if len(run_ids) != len(runs) or len(fixed_ids) != len(fixed):
        _fail(case_id, "final run and fixed IDs must be unique")
    if any(
        row["fixed_run_id"] is not None
        and row["fixed_run_id"] not in fixed_ids
        and row["status"] not in {"done", "cancelled"}
        for row in runs
    ):
        _fail(case_id, "a live final run references an absent fixed run")
    if any(row["run_id"] not in run_ids for row in state["reminders"] + state["rsvps"]):
        _fail(case_id, "a final reminder or RSVP references an absent run")


def validate_document(document: dict[str, Any]) -> None:
    _validate_schema(document, None)
    seen = set()
    for case in document["cases"]:
        if case["case_id"] in seen:
            _fail(case["case_id"], "duplicate case ID")
        seen.add(case["case_id"])
        _validate_input_semantics(case)
        _validate_completed_semantics(case)
