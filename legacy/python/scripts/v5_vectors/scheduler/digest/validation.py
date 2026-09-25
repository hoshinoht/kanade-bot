"""Input gate for digest vectors: schema first, then static references."""

from __future__ import annotations

from typing import Any

from .. import contract

SCHEMA = contract.FamilySchema("digest.schema.json")


def semantics(case: dict[str, Any]) -> None:
    contract.references(case, {"set_rsvp", "set_status"}, ("at", "week_start", "clock"))


def validate_input_case(case: object) -> None:
    SCHEMA.validate(case, "replayCase", contract.context(case))
    semantics(case)  # type: ignore[arg-type]


def validate_document(document: dict[str, Any]) -> None:
    contract.validate_document(SCHEMA, document, semantics)
