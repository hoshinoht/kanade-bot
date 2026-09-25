"""Schema-first gates shared by the reminder and mutation vector families."""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from jsonschema import Draft202012Validator, FormatChecker, SchemaError, ValidationError

from .validation import ContractError

VECTOR_DIR = Path(__file__).resolve().parents[5] / "docs" / "v5" / "vectors" / "scheduler"


def fail(case_id: str, message: str) -> None:
    raise ContractError(f"{case_id}: {message}")


class FamilySchema:
    """One family's checked-in Draft 2020-12 schema.

    Every operation's success value is typed by ``$defs/result_<op>``; the
    document schema accepts any of them and :meth:`check_value` pairs them.
    """

    def __init__(self, filename: str):
        self.path = VECTOR_DIR / filename
        self._validators: dict[str | None, Draft202012Validator] = {}

    def _schema(self) -> dict[str, Any]:
        try:
            schema = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            raise ContractError(f"schema could not be loaded from {self.path}: {exc}") from None
        if not isinstance(schema, dict):
            raise ContractError(f"schema at {self.path} must be a JSON object")
        try:
            Draft202012Validator.check_schema(schema)
        except SchemaError as exc:
            raise ContractError(f"schema at {self.path} is invalid: {exc.message}") from None
        return schema

    def _validator(self, target: str | None) -> Draft202012Validator:
        if target not in self._validators:
            self._validators[target] = self._build(target)
        return self._validators[target]

    def _build(self, target: str | None) -> Draft202012Validator:
        schema = self._schema()
        if target is not None:
            if target not in schema.get("$defs", {}):
                raise ContractError(f"schema at {self.path} is missing $defs/{target}")
            schema = {
                "$schema": schema["$schema"],
                "$defs": schema["$defs"],
                "$ref": f"#/$defs/{target}",
            }
        return Draft202012Validator(schema, format_checker=FormatChecker())

    def validate(self, instance: object, target: str | None, context: str) -> None:
        try:
            self._validator(target).validate(instance)
        except ValidationError as exc:
            path = "$" + "".join(
                f"[{part}]" if isinstance(part, int) else f".{part}" for part in exc.absolute_path
            )
            raise ContractError(
                f"{context}: schema validation failed at {path} [{exc.validator}]: {exc.message}"
            ) from None

    def check_value(self, case_id: str, op: str, value: Any) -> None:
        self.validate(value, f"result_{op}", f"{case_id} {op} result")


def context(instance: object) -> str:
    if isinstance(instance, dict) and isinstance(instance.get("case_id"), str):
        return instance["case_id"]
    return "vector case"


def aware(case_id: str, field: str, value: object) -> None:
    try:
        parsed = datetime.fromisoformat(str(value))
    except ValueError:
        parsed = None
    if parsed is None or parsed.utcoffset() is None:
        fail(case_id, f"{field} must be an aware ISO datetime")


def zone(case_id: str, name: str) -> None:
    try:
        ZoneInfo(name)
    except (ZoneInfoNotFoundError, ValueError):
        fail(case_id, f"unknown timezone {name!r}")


def run_reference(case_id: str, key: str, runs: set[str], fixed_ever: set[str]) -> None:
    """A run key is a created run or a possible ``<fixed_key>@<week start>`` materialization.

    Materialized keys stay *possible* statically; replay accepts one only after
    the v4 oracle actually produced that run.
    """
    if key in runs:
        return
    fixed, sep, start = key.partition("@")
    if sep and fixed in fixed_ever:
        aware(case_id, "materialized run key", start)
        return
    fail(case_id, f"undefined run key {key!r}")


def completed(schema: FamilySchema, case: dict[str, Any]) -> None:
    """Expected-step cardinality, error pairing, and per-operation result typing."""
    case_id, expected, steps = case["case_id"], case["expected"], case["input"]["steps"]
    if len(expected["steps"]) != len(steps):
        fail(case_id, "expected steps must match input steps")
    for step, result in zip(steps, expected["steps"], strict=True):
        if set(result) == {"error"}:
            if step.get("error_type") != result["error"]["type"]:
                fail(case_id, "error type must match the step declaration")
        elif set(result) == {"value"}:
            if "error_type" in step:
                fail(case_id, "a failing step cannot have a success value")
            schema.check_value(case_id, step["op"], result["value"])
        else:
            fail(case_id, "each expected step needs exactly one value or error")
    state = expected["final_state"]
    if "runs" not in state:
        return
    run_ids = {row["id"] for row in state["runs"]}
    if len(run_ids) != len(state["runs"]):
        fail(case_id, "final run IDs must be unique")
    if any(row["run_id"] not in run_ids for row in state["reminders"] + state["rsvps"]):
        fail(case_id, "a final reminder or RSVP references an absent run")


def validate_document(schema: FamilySchema, document: dict[str, Any], semantics: Any) -> None:
    schema.validate(document, None, "vector document")
    seen: set[str] = set()
    for case in document["cases"]:
        if case["case_id"] in seen:
            fail(case["case_id"], "duplicate case ID")
        seen.add(case["case_id"])
        semantics(case)
        completed(schema, case)


def references(case: dict[str, Any], run_ops: set[str], instant_fields: tuple[str, ...]) -> None:
    """Aware instants, unique keys, and run/fixed references declared before use."""
    case_id, input_ = case["case_id"], case["input"]
    aware(case_id, "clock", input_["clock"])
    zone(case_id, input_["timezone"])
    members = [member["user_id"] for member in input_.get("members", [])]
    if len(set(members)) != len(members):
        fail(case_id, "member user IDs must be unique")
    fixed: set[str] = set()
    runs: set[str] = set()
    for step in input_["steps"]:
        op = step["op"]
        for field in instant_fields:
            if step.get(field) is not None:
                aware(case_id, f"{op} {field}", step[field])
        if op == "add_fixed":
            if step["fixed_key"] in fixed:
                fail(case_id, f"add_fixed key {step['fixed_key']!r} is not unique")
            fixed.add(step["fixed_key"])
        elif op == "create_run":
            if step["run_key"] in runs:
                fail(case_id, f"create_run key {step['run_key']!r} is not unique")
            runs.add(step["run_key"])
        elif op in run_ops:
            run_reference(case_id, step["run_key"], runs, fixed)
