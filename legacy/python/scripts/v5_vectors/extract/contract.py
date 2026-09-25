"""Schema builders, the family shape, and the schema-first gate for extraction vectors.

Each family's Draft 2020-12 schema is authored here as data and written beside
its vectors; ``--check`` holds both to their checked-in bytes. Replay validates
a case's input against ``$defs/replayCase`` before calling the oracle, and the
document gate pairs every success value with ``$defs/result_<op>``.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker, ValidationError

DRAFT = "https://json-schema.org/draft/2020-12/schema"


class ContractError(ValueError):
    """A vector document or case breaks the extraction contract."""


def closed(props: dict[str, Any], optional: tuple[str, ...] = ()) -> dict[str, Any]:
    return {
        "type": "object",
        "properties": props,
        "required": [key for key in props if key not in optional],
        "additionalProperties": False,
    }


def arr(items: dict[str, Any], min_items: int = 0) -> dict[str, Any]:
    out: dict[str, Any] = {"type": "array", "items": items}
    if min_items:
        out["minItems"] = min_items
    return out


def nullable(schema: dict[str, Any]) -> dict[str, Any]:
    return {"anyOf": [schema, {"type": "null"}]}


def ref(name: str) -> dict[str, str]:
    return {"$ref": f"#/$defs/{name}"}


STR = {"type": "string"}
NSTR = {"type": ["string", "null"]}
BOOL = {"type": "boolean"}
INT = {"type": "integer"}
NUM = {"type": "number"}
#: ``format`` alone is not enforced without optional validators, so the offset is a pattern.
INSTANT = {
    "type": "string",
    "format": "date-time",
    "pattern": r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$",
}
NINSTANT = nullable(INSTANT)
DATE = {"type": "string", "format": "date", "pattern": r"^\d{4}-\d{2}-\d{2}$"}
NDATE = nullable(DATE)
CLOCK = {"type": "string", "pattern": "^([01][0-9]|2[0-3]):[0-5][0-9](:[0-5][0-9])?$"}
NCLOCK = nullable(CLOCK)
STRS = arr(STR)
ANY: dict[str, Any] = {}
CASE_ID = {"type": "string", "pattern": "^[a-z0-9][a-z0-9-]*$"}
PROVENANCE = closed(
    {"oracle": STR, "functions": STRS, "source_tests": STRS, "inventory_surfaces": STRS}
)


@dataclass(frozen=True)
class Op:
    """One step operation: its input fields, success type, and declarable errors."""

    fields: dict[str, Any]
    result: dict[str, Any]
    errors: tuple[str, ...] = ()
    optional: tuple[str, ...] = ()


@dataclass(frozen=True)
class Family:
    name: str
    version: str
    provenance: dict[str, Any]
    context: dict[str, Any]
    ops: dict[str, Op]
    cases: Callable[[], list[dict[str, Any]]]
    replay: Callable[[dict[str, Any]], dict[str, Any]]
    defs: dict[str, Any] = field(default_factory=dict)
    state: dict[str, Any] | None = None

    @property
    def vector(self) -> str:
        return f"{self.name}.json"

    @property
    def schema_file(self) -> str:
        return f"{self.name}.schema.json"

    def schema(self) -> dict[str, Any]:
        defs = dict(self.defs)
        steps = []
        for op, spec in self.ops.items():
            props: dict[str, Any] = {"op": {"const": op}, **spec.fields}
            optional = spec.optional
            if spec.errors:
                props["error_type"] = {"enum": list(spec.errors)}
                optional = (*optional, "error_type")
            defs[f"step_{op}"] = closed(props, optional)
            defs[f"result_{op}"] = spec.result
            steps.append(ref(f"step_{op}"))
        defs["step"] = {"oneOf": steps}
        defs["input"] = closed({**self.context, "steps": arr(ref("step"), 1)})
        defs["outcome"] = {
            "oneOf": [
                closed({"value": ANY}),
                closed({"error": closed({"type": STR, "message": STR})}),
            ]
        }
        expected: dict[str, Any] = {"steps": arr(ref("outcome"), 1)}
        if self.state is not None:
            expected["final_state"] = self.state
        defs["expected"] = closed(expected)
        defs["replayCase"] = closed({"case_id": CASE_ID, "input": ref("input")})
        defs["case"] = closed(
            {"case_id": CASE_ID, "input": ref("input"), "expected": ref("expected")}
        )
        return {
            "$schema": DRAFT,
            "title": f"Kanade v5 extraction vectors: {self.name}",
            **closed(
                {
                    "schema_version": {"const": self.version},
                    "family": {"const": self.name},
                    "provenance": PROVENANCE,
                    "cases": arr(ref("case"), 1),
                }
            ),
            "$defs": defs,
        }

    def document(self) -> dict[str, Any]:
        return {
            "schema_version": self.version,
            "family": self.name,
            "provenance": self.provenance,
            "cases": self.cases(),
        }


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


def run_step(
    case_id: str,
    index: int,
    step: dict[str, Any],
    call: Callable[[], Any],
    errors: dict[str, type[Exception]] | None = None,
) -> dict[str, Any]:
    """Run one oracle call; only the step's declared exact error class is captured."""
    errors = errors or {"ValueError": ValueError}
    expected = step.get("error_type")
    try:
        value = call()
    except tuple(errors.values()) as exc:
        if expected is None or type(exc) is not errors[expected]:
            raise
        return {"error": {"type": expected, "message": str(exc)}}
    if expected is not None:
        raise ContractError(f"{case_id} step {index}: expected {expected} but it succeeded")
    return {"value": value}


def replay_steps(
    family: Family,
    case: dict[str, Any],
    handlers: dict[str, Callable[[dict[str, Any]], Any]],
    errors: dict[str, type[Exception]] | None = None,
) -> dict[str, Any]:
    """Schema-gate the input, then run each step through its handler."""
    validate_input(family, case)
    results = []
    for index, step in enumerate(case["input"]["steps"]):
        handler = handlers[step["op"]]
        results.append(
            run_step(case["case_id"], index, step, lambda: handler(step), errors)  # noqa: B023
        )
    return {"steps": results}
