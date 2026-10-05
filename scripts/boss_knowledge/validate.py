#!/usr/bin/env python3
"""Validate boss/knowledge/*.yaml against schema.json and guard against copied guide prose.

Needs PyYAML and jsonschema:
    uv run --no-project --with pyyaml --with jsonschema scripts/boss_knowledge/validate.py
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import date
from pathlib import Path

try:
    import jsonschema
    import yaml
except ImportError as exc:  # pragma: no cover - environment hint
    sys.exit(
        f"missing dependency ({exc.name}); run via: "
        f"uv run --no-project --with pyyaml --with jsonschema {__file__}"
    )

ROOT = Path(__file__).resolve().parents[2]
KNOWLEDGE = ROOT / "boss" / "knowledge"
CACHE = ROOT / "data" / "research" / "boss-guides"
CATALOG = ROOT / "legacy" / "python" / "boss" / "bosses.yaml"
LETTER_NAMES = {"e": "Easy", "n": "Normal", "h": "Hard", "c": "Chaos", "x": "Extreme"}
# Shown on the Bosses info page only; never scheduler (catalog) difficulties.
INFO_ONLY = {"Champion", "Destiny"}
DEFAULT_NGRAM = 12
REPORT_FLOOR = 6
_WORD = re.compile(r"[a-z0-9%]+")


class _UniqueKeyLoader(yaml.SafeLoader):
    pass


def _unique_mapping(loader: _UniqueKeyLoader, node: yaml.MappingNode, deep: bool = False) -> dict:
    mapping: dict = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in mapping:
            raise yaml.constructor.ConstructorError(
                None, None, f"duplicate key {key!r}", key_node.start_mark
            )
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


_UniqueKeyLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _unique_mapping)


def words(text: str) -> list[str]:
    return _WORD.findall(text.lower().replace("\u2019", "'").replace("'", ""))


def strings(value: object, path: str = ""):
    if isinstance(value, str):
        yield path, value
    elif isinstance(value, dict):
        for key, item in value.items():
            yield from strings(item, f"{path}.{key}" if path else str(key))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from strings(item, f"{path}[{index}]")


def grams(tokens: list[str], n: int) -> set[tuple[str, ...]]:
    return {tuple(tokens[i : i + n]) for i in range(len(tokens) - n + 1)}


def longest_shared_run(tokens: list[str], small: set[tuple[str, ...]]) -> int:
    """Approximate longest run of words shared with the guide, via chained REPORT_FLOOR-grams."""
    best = run = 0
    for i in range(len(tokens) - REPORT_FLOOR + 1):
        run = run + 1 if tuple(tokens[i : i + REPORT_FLOOR]) in small else 0
        best = max(best, run)
    return best + REPORT_FLOOR - 1 if best else 0


def load_guides(cache: Path, n: int) -> dict[str, tuple[set, set]]:
    guides = {}
    for path in sorted(cache.glob("*.txt")):
        tokens = words(path.read_text(encoding="utf-8"))
        guides[path.stem] = (grams(tokens, n), grams(tokens, REPORT_FLOOR))
    return guides


def check_dates(doc: dict, errors: list[str]) -> None:
    today = date.today()
    for index, source in enumerate(doc.get("sources", [])):
        for field in ("fetched", "updated"):
            if isinstance(source, dict) and field in source:
                try:
                    value = date.fromisoformat(source[field])
                except (TypeError, ValueError):
                    errors.append(f"sources[{index}].{field} is not a real date")
                    continue
                if value > today:
                    errors.append(f"sources[{index}].{field} is in the future")


def check_semantics(path: Path, doc: dict, catalog: dict, errors: list[str]) -> None:
    boss = doc.get("boss")
    if isinstance(boss, str) and path.stem != boss.lower():
        errors.append(f"file stem must be {boss.lower()!r}")
    urls = [s.get("url") for s in doc.get("sources", []) if isinstance(s, dict)]
    if len(urls) != len(set(urls)):
        errors.append("duplicate source url")
    names = [d.get("name") for d in doc.get("difficulties", []) if isinstance(d, dict)]
    if len(names) != len(set(names)):
        errors.append("duplicate difficulty name")
    for d in doc.get("difficulties", []):
        phases = [h.get("phase") for h in d.get("hp", []) if isinstance(h, dict)]
        if len(phases) != len(set(phases)):
            errors.append(f"difficulties[{d.get('name')}] duplicate hp phase")
    strategies = [s.get("name") for s in doc.get("strategies", []) if isinstance(s, dict)]
    if len(strategies) != len(set(strategies)):
        errors.append("duplicate strategy name")
    aliases = [a.casefold() for a in doc.get("event", {}).get("aliases", []) if isinstance(a, str)]
    if len(aliases) != len(set(aliases)):
        errors.append("duplicate event alias")
    if boss in catalog and "event" in doc:
        errors.append(f"{boss!r} is a catalog boss, so it must not declare `event`")
    clashes = sorted({a for a in aliases} & {key.casefold() for key in catalog})
    if clashes:
        errors.append(f"event aliases collide with catalog keys: {', '.join(clashes)}")
    if not catalog:
        return
    if boss in catalog:
        allowed = {LETTER_NAMES[letter] for letter in catalog[boss]} | INFO_ONLY
        extra = sorted(set(names) - allowed)
        if extra:
            errors.append(f"difficulties not in catalog for {boss}: {', '.join(extra)}")
        notes = set(doc.get("difficulty_notes", {})) - set(catalog[boss])
        if notes:
            errors.append(f"difficulty_notes letters not in catalog: {', '.join(sorted(notes))}")
    elif "event" not in doc:
        errors.append(f"{boss!r} is not a catalog boss, so it must declare `event`")


def load_catalog(path: Path) -> dict[str, list[str]]:
    if not path.is_file():
        return {}
    raw = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    return {
        key: list(value.get("difficulties", [])) for key, value in raw.get("bosses", {}).items()
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--dir", type=Path, default=KNOWLEDGE)
    parser.add_argument(
        "--cache", type=Path, default=CACHE, help="cached guide text for the anti-copy guard"
    )
    parser.add_argument(
        "--catalog", type=Path, default=CATALOG, help="boss catalog for difficulty cross-checks"
    )
    parser.add_argument(
        "--ngram", type=int, default=DEFAULT_NGRAM, help="consecutive shared words that fail"
    )
    args = parser.parse_args(argv)

    schema = json.loads((args.dir / "schema.json").read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator.check_schema(schema)
    doc_validator = jsonschema.Draft202012Validator(schema)
    meta_validator = jsonschema.Draft202012Validator(
        {"$defs": schema["$defs"], "$ref": "#/$defs/meta"}
    )
    catalog = load_catalog(args.catalog)
    guides = load_guides(args.cache, args.ngram) if args.cache.is_dir() else {}
    if not guides:
        print(f"WARN anti-copy guard skipped: no cached guides in {args.cache} (run fetch.py)")

    failures = 0
    files = sorted(p for p in args.dir.iterdir() if p.suffix in {".yaml", ".yml"})
    for path in files:
        errors: list[str] = []
        try:
            doc = yaml.load(path.read_text(encoding="utf-8"), Loader=_UniqueKeyLoader)
        except yaml.YAMLError as exc:
            print(f"FAIL {path.name}: invalid YAML: {exc}")
            failures += 1
            continue
        validator = meta_validator if path.name == "_meta.yaml" else doc_validator
        for error in sorted(validator.iter_errors(doc), key=lambda e: list(e.absolute_path)):
            where = ".".join(map(str, error.absolute_path)) or "<root>"
            errors.append(f"{where}: {error.message}")
        if path.name != "_meta.yaml" and isinstance(doc, dict):
            check_dates(doc, errors)
            check_semantics(path, doc, catalog, errors)

        longest = (0, "", "")
        for field, text in strings(doc):
            tokens = words(text)
            for guide, (big, small) in guides.items():
                if grams(tokens, args.ngram) & big:
                    errors.append(
                        f"{field}: >= {args.ngram} consecutive words copied from {guide}.txt"
                    )
                run = longest_shared_run(tokens, small)
                if run > longest[0]:
                    longest = (run, field, guide)

        overlap = (
            f" (longest shared run ~{longest[0]} words: {longest[1]} vs {longest[2]}.txt)"
            if longest[0]
            else ""
        )
        if errors:
            failures += 1
            print(f"FAIL {path.name}{overlap}")
            for error in errors:
                print(f"     - {error}")
        else:
            print(f"ok   {path.name}{overlap}")
    print(
        f"{len(files)} file(s), {failures} failing; "
        f"anti-copy threshold {args.ngram} words, {len(guides)} guide(s) cached"
    )
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
