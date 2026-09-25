"""Generate public v4 persona prompt-assembly oracle vectors."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import yaml
from jsonschema import Draft202012Validator, FormatChecker

from . import cases, replay

ROOT = Path(__file__).resolve().parents[5]
DEFAULT_OUTPUT = ROOT / "docs" / "v5" / "vectors" / "persona"
SCHEMA_PATH = DEFAULT_OUTPUT / "schema.json"
V5_BUNDLE = ROOT / "config" / "personas" / "bundles" / f"{cases.BUNDLE_ID}.yaml"
VECTORS = "persona.json"


class ContractError(ValueError):
    """The tracked v5 bundle or a vector document breaks the contract."""


def check_v5_bundle(templates: dict[str, Any]) -> None:
    """The tracked v5 Kanade bundle must carry the v4 templates byte-for-byte."""
    bundle = yaml.safe_load(V5_BUNDLE.read_text(encoding="utf-8"))
    # `compact` and `nudges` are v5-only with no v4 oracle; the Rust tests pin them.
    bundle.pop("compact", None)
    bundle.pop("nudges", None)
    expected = {
        "schema_version": 1,
        "id": templates["id"],
        "identity": templates["identity"],
        "behaviour": {"prompt": templates["behaviour_prompt"]},
        "staging": templates["staging"],
    }
    if bundle != expected:
        raise ContractError(f"{V5_BUNDLE.relative_to(ROOT)} differs from the v4 templates")


def documents() -> dict[str, dict[str, Any]]:
    templates = replay.bundle_templates()
    check_v5_bundle(templates)
    document = {
        "schema_version": "v5-persona-v1",
        "family": "persona",
        "provenance": {
            "oracle": "legacy/python bot.chat.persona, bot.chat.persona_catalog, bot.chat.progress",
            "functions": [
                "persona_catalog.load_example_bundle",
                "persona.clock_header",
                "persona.runtime_line",
                "persona.focus_line",
                "persona.component_system_prompt",
                "persona.component_voice_reminder",
                "progress.parse_profile_staging",
            ],
            "templates": [
                f"legacy/python/config/personas/personas/kanade/{name}"
                for name in replay.TEMPLATE_FILES.values()
            ],
            "source_tests": ["tests/test_chat_persona.py", "tests/test_persona_catalog.py"],
            "inventory_surfaces": ["chat.persona-assembly", "chat.persona-catalog"],
        },
        "bundles": [templates],
        "cases": cases.cases(),
    }
    seeded = json.loads(json.dumps(document, ensure_ascii=False, sort_keys=True))
    for case in seeded["cases"]:
        case["expected"] = replay.replay(case)
    validate(seeded)
    return {VECTORS: seeded}


def validate(document: dict[str, Any]) -> None:
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    Draft202012Validator.check_schema(schema)
    errors = sorted(
        Draft202012Validator(schema, format_checker=FormatChecker()).iter_errors(document),
        key=lambda error: list(error.absolute_path),
    )
    if errors:
        raise ContractError(f"persona vectors violate the schema: {errors[0].message}")
    bundle_ids = {bundle["id"] for bundle in document["bundles"]}
    case_ids = [case["case_id"] for case in document["cases"]]
    if len(set(case_ids)) != len(case_ids):
        raise ContractError("persona vector case IDs must be unique")
    for case in document["cases"]:
        if case["input"]["bundle_id"] not in bundle_ids:
            raise ContractError(f"{case['case_id']}: unknown bundle_id")


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
            raise SystemExit("v5 persona vectors drift; run python -m scripts.v5_vectors.persona")
        return
    write(args.output)
