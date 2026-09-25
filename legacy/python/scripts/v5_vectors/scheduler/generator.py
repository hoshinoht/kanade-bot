"""Generate deterministic stateful scheduler vectors from the v4 oracle."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

from . import cases, replay, validation
from .digest import cases as digest_cases
from .digest import replay as digest_replay
from .digest import validation as digest_validation
from .dispatch import cases as dispatch_cases
from .dispatch import replay as dispatch_replay
from .dispatch import validation as dispatch_validation
from .mentions import cases as mention_cases
from .mentions import replay as mention_replay
from .mentions import validation as mention_validation
from .mutations import cases as mutation_cases
from .mutations import replay as mutation_replay
from .mutations import validation as mutation_validation
from .reminders import cases as reminder_cases
from .reminders import replay as reminder_replay
from .reminders import validation as reminder_validation

ROOT = Path(__file__).resolve().parents[5]
DEFAULT_OUTPUT = ROOT / "docs" / "v5" / "vectors" / "scheduler"
#: (serialized cases, oracle replay, completed-document gate) per family.
FAMILIES = (
    (cases.documents, replay.replay, validation.validate_document),
    (reminder_cases.documents, reminder_replay.replay, reminder_validation.validate_document),
    (mutation_cases.documents, mutation_replay.replay, mutation_validation.validate_document),
    (dispatch_cases.documents, dispatch_replay.replay, dispatch_validation.validate_document),
    (mention_cases.documents, mention_replay.replay, mention_validation.validate_document),
    (digest_cases.documents, digest_replay.replay, digest_validation.validate_document),
)


def documents() -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    for serialized, oracle_replay, validate in FAMILIES:
        seeded = json.loads(json.dumps(serialized(), ensure_ascii=False, sort_keys=True))
        for name, document in seeded.items():
            for case in document["cases"]:
                case["expected"] = oracle_replay(case)
            validate(document)
            out[name] = document
    return out


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
