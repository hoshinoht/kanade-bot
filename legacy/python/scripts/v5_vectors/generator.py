"""Produce deterministic pure-domain compatibility vectors from the v4 oracle."""

from __future__ import annotations

import argparse
import json
from datetime import datetime, time
from pathlib import Path
from typing import Any

from bot.domain.bosses import BossParseError
from bot.domain.ids import IdAmbiguous, IdNotFound, IdTooShort

from . import cases, replay

ROOT = Path(__file__).resolve().parents[4]
DEFAULT_OUTPUT = ROOT / "docs" / "v5" / "vectors" / "domain"
ERROR_TYPES = {
    "ValueError": ValueError,
    "BossParseError": BossParseError,
    "IdAmbiguous": IdAmbiguous,
    "IdNotFound": IdNotFound,
    "IdTooShort": IdTooShort,
}


def _normalise(value: Any) -> Any:
    if isinstance(value, (datetime, time)):
        return value.isoformat()
    if isinstance(value, tuple):
        return [_normalise(item) for item in value]
    if isinstance(value, list):
        return [_normalise(item) for item in value]
    if isinstance(value, dict):
        return {key: _normalise(item) for key, item in value.items()}
    return value


def _expected(case: dict[str, Any], fixtures: dict[str, Any]) -> dict[str, Any]:
    error_type = case.get("error_type")
    if error_type is None:
        return {"value": _normalise(replay.dispatch(case, fixtures))}
    expected_class = ERROR_TYPES[error_type]
    try:
        replay.dispatch(case, fixtures)
    except expected_class as exc:
        if type(exc) is not expected_class:
            raise
        return {"error": {"type": error_type, "message": str(exc)}}
    raise AssertionError(f"{case['case_id']} expected {error_type} but succeeded")


def documents() -> dict[str, dict[str, Any]]:
    """Replay documents after JSON round-tripping their serialized fixtures."""
    seeded = json.loads(json.dumps(cases.documents(), ensure_ascii=False, sort_keys=True))
    for document in seeded.values():
        fixtures = document.get("fixtures", {})
        for case in document["cases"]:
            case["expected"] = _expected(case, fixtures)
    return seeded


def _bytes(document: dict[str, Any]) -> bytes:
    return (json.dumps(document, indent=2, ensure_ascii=False, sort_keys=True) + "\n").encode()


def write(output: Path = DEFAULT_OUTPUT) -> None:
    output.mkdir(parents=True, exist_ok=True)
    for name, document in documents().items():
        (output / name).write_bytes(_bytes(document))


def check(output: Path = DEFAULT_OUTPUT) -> bool:
    return all(
        (output / name).read_bytes() == _bytes(document) for name, document in documents().items()
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    if args.check:
        if not check(args.output):
            raise SystemExit("v5 domain vectors drift; run python -m scripts.v5_vectors")
        return
    write(args.output)
