"""Input gate for mention vectors: schema first, then static checks."""

from __future__ import annotations

from typing import Any

from .. import contract

SCHEMA = contract.FamilySchema("mentions.schema.json")


def semantics(case: dict[str, Any]) -> None:
    contract.references(case, set(), ())


def validate_input_case(case: object) -> None:
    SCHEMA.validate(case, "replayCase", contract.context(case))
    semantics(case)  # type: ignore[arg-type]


def validate_document(document: dict[str, Any]) -> None:
    contract.validate_document(SCHEMA, document, semantics)
