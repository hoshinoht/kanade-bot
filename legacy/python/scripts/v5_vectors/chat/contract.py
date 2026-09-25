"""The chat family shape and its schema-first gate.

Schema builders and ``Op`` are the extraction lane's; the gate is re-declared
here because its validator cache is keyed by family name and chat reuses names
(``gate``) that extraction already owns.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker, ValidationError

from ..extract import contract as base
from ..extract.contract import (  # noqa: F401 - re-exported for the families
    ANY,
    BOOL,
    DRAFT,
    INSTANT,
    INT,
    NINSTANT,
    NSTR,
    NUM,
    STR,
    STRS,
    ContractError,
    Op,
    arr,
    closed,
    nullable,
    ref,
    run_step,
)


class Family(base.Family):
    def schema(self) -> dict[str, Any]:
        return {**super().schema(), "title": f"Kanade v5 chat vectors: {self.name}"}


_VALIDATORS: dict[tuple[str, str | None], Draft202012Validator] = {}


def _validator(family: Family, target: str | None) -> Draft202012Validator:
    key = (family.name, target)
    if key not in _VALIDATORS:
        schema = family.schema()
        Draft202012Validator.check_schema(schema)
        if target is not None:
            schema = {"$schema": DRAFT, "$defs": schema["$defs"], "$ref": f"#/$defs/{target}"}
        _VALIDATORS[key] = Draft202012Validator(schema, format_checker=FormatChecker())
    return _VALIDATORS[key]


def validate(family: Family, instance: object, target: str | None, context: str) -> None:
    try:
        _validator(family, target).validate(instance)
    except ValidationError as exc:
        path = "$" + "".join(
            f"[{part}]" if isinstance(part, int) else f".{part}" for part in exc.absolute_path
        )
        raise ContractError(
            f"{context}: schema validation failed at {path} [{exc.validator}]: {exc.message}"
        ) from None


def validate_input(family: Family, case: object) -> None:
    context = case.get("case_id", "vector case") if isinstance(case, dict) else "vector case"
    validate(family, case, "replayCase", str(context))


def validate_document(family: Family, document: dict[str, Any]) -> None:
    """Whole-document schema, unique case IDs, and per-step outcome pairing."""
    validate(family, document, None, f"{family.name} document")
    seen: set[str] = set()
    for case in document["cases"]:
        case_id = case["case_id"]
        if case_id in seen:
            raise ContractError(f"{case_id}: duplicate case ID")
        seen.add(case_id)
        steps, results = case["input"]["steps"], case["expected"]["steps"]
        if len(steps) != len(results):
            raise ContractError(f"{case_id}: expected steps must match input steps")
        for index, (step, result) in enumerate(zip(steps, results, strict=True)):
            if "error" in result:
                if step.get("error_type") != result["error"]["type"]:
                    raise ContractError(f"{case_id} step {index}: undeclared error type")
            else:
                if "error_type" in step:
                    raise ContractError(f"{case_id} step {index}: declared error did not occur")
                validate(family, result["value"], f"result_{step['op']}", f"{case_id} step {index}")


def run_steps(
    case: dict[str, Any],
    handlers: dict[str, Callable[[dict[str, Any]], Any]],
    errors: dict[str, type[Exception]] | None = None,
) -> dict[str, Any]:
    """Run each (already gated) step through its handler."""
    results = []
    for index, step in enumerate(case["input"]["steps"]):
        handler = handlers[step["op"]]
        results.append(
            run_step(case["case_id"], index, step, lambda: handler(step), errors)  # noqa: B023
        )
    return {"steps": results}


def replay_steps(
    family: Family,
    case: dict[str, Any],
    handlers: dict[str, Callable[[dict[str, Any]], Any]],
    errors: dict[str, type[Exception]] | None = None,
) -> dict[str, Any]:
    """Schema-gate the input, then run each step through its handler."""
    validate_input(family, case)
    return run_steps(case, handlers, errors)
