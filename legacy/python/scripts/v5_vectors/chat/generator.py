"""Generate deterministic chat vectors, schemas and index from the v4 oracle."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from . import contract
from .families import (
    authority,
    context,
    gate,
    loop,
    participants,
    propose,
    read_tools,
    sanitize,
    tool_schemas,
)

ROOT = Path(__file__).resolve().parents[5]
DEFAULT_OUTPUT = ROOT / "docs" / "v5" / "vectors" / "chat"
INDEX_VERSION = "v5-chat-index-v1"
FAMILIES: tuple[contract.Family, ...] = (
    gate.FAMILY,
    authority.FAMILY,
    participants.FAMILY,
    tool_schemas.FAMILY,
    read_tools.FAMILY,
    propose.FAMILY,
    sanitize.FAMILY,
    loop.FAMILY,
    context.FAMILY,
)


def _roundtrip(value: Any) -> Any:
    return json.loads(json.dumps(value, ensure_ascii=False, sort_keys=True))


def document(family: contract.Family) -> dict[str, Any]:
    """Serialize inputs, JSON-round-trip them, then fill ``expected`` from the oracle."""
    seeded = _roundtrip(family.document())
    for case in seeded["cases"]:
        case["expected"] = _roundtrip(
            family.replay({"case_id": case["case_id"], "input": case["input"]})
        )
    contract.validate_document(family, seeded)
    return seeded


def index() -> dict[str, Any]:
    return {
        "schema_version": INDEX_VERSION,
        "families": [
            {"family": f.name, "schema": f.schema_file, "vector": f.vector} for f in FAMILIES
        ],
    }


def documents() -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {"index.json": index()}
    for family in FAMILIES:
        out[family.schema_file] = family.schema()
        out[family.vector] = document(family)
    return out


def _bytes(document: dict[str, Any]) -> bytes:
    return (json.dumps(document, indent=2, ensure_ascii=False, sort_keys=True) + "\n").encode()


def write(output: Path = DEFAULT_OUTPUT) -> None:
    output.mkdir(parents=True, exist_ok=True)
    for name, doc in documents().items():
        (output / name).write_bytes(_bytes(doc))


def check(output: Path = DEFAULT_OUTPUT) -> bool:
    return all(
        (output / name).is_file() and (output / name).read_bytes() == _bytes(doc)
        for name, doc in documents().items()
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    if args.check:
        if not check(args.output):
            raise SystemExit("v5 chat vectors drift; run python -m scripts.v5_vectors.chat")
        return
    write(args.output)
