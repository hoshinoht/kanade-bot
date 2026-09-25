"""Input gate for mutation vectors: schema first, then static references."""

from __future__ import annotations

from typing import Any

from .. import contract

SCHEMA = contract.FamilySchema("mutations.schema.json")
RUN_OPS = {"set_rsvp", "set_status", "amend", "swap"}


def semantics(case: dict[str, Any]) -> None:
    case_id, input_ = case["case_id"], case["input"]
    contract.aware(case_id, "clock", input_["clock"])
    contract.zone(case_id, input_["timezone"])
    members = [member["user_id"] for member in input_["members"]]
    if len(set(members)) != len(members):
        contract.fail(case_id, "member user IDs must be unique")
    fixed: set[str] = set()
    runs: set[str] = set()
    for step in input_["steps"]:
        op = step["op"]
        for field in ("at", "clock", "week_start"):
            if field in step:
                contract.aware(case_id, f"{op} {field}", step[field])
        if op == "add_fixed":
            if step["fixed_key"] in fixed:
                contract.fail(case_id, f"add_fixed key {step['fixed_key']!r} is not unique")
            fixed.add(step["fixed_key"])
        elif op == "create_run":
            if step["run_key"] in runs:
                contract.fail(case_id, f"create_run key {step['run_key']!r} is not unique")
            runs.add(step["run_key"])
        elif op in RUN_OPS:
            contract.run_reference(case_id, step["run_key"], runs, fixed)
        elif op == "update_fixed" and step["fixed_key"] not in fixed:
            contract.fail(case_id, f"undefined fixed key {step['fixed_key']!r}")


def validate_input_case(case: object) -> None:
    SCHEMA.validate(case, "replayCase", contract.context(case))
    semantics(case)  # type: ignore[arg-type]


def validate_document(document: dict[str, Any]) -> None:
    contract.validate_document(SCHEMA, document, semantics)
