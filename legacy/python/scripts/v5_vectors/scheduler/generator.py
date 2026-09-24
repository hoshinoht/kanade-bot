"""Generate deterministic stateful scheduler vectors from the v4 oracle."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from . import cases, replay, validation

ROOT = Path(__file__).resolve().parents[5]
DEFAULT_OUTPUT = ROOT / "docs" / "v5" / "vectors" / "scheduler"


def documents() -> dict[str, dict[str, Any]]:
    seeded = json.loads(json.dumps(cases.documents(), ensure_ascii=False, sort_keys=True))
    for document in seeded.values():
        for case in document["cases"]:
            case["expected"] = replay.replay(case)
        validation.validate_document(document)
    return seeded


def _bytes(document: dict[str, Any]) -> bytes:
    return (json.dumps(document, indent=2, ensure_ascii=False, sort_keys=True) + "\n").encode()


def write(output: Path = DEFAULT_OUTPUT) -> None:
    output.mkdir(parents=True, exist_ok=True)
    for name, document in documents().items():
        (output / name).write_bytes(_bytes(document))


def check(output: Path = DEFAULT_OUTPUT) -> bool:
    return all(
        (output / name).is_file() and (output / name).read_bytes() == _bytes(document)
        for name, document in documents().items()
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    if args.check:
        if not check(args.output):
            raise SystemExit(
                "v5 scheduler vectors drift; run python -m scripts.v5_vectors.scheduler"
            )
        return
    write(args.output)
