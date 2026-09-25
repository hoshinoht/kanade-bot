"""Convert a private v4 persona directory into the v5 layout.

    uv run python -m scripts.convert_personas_v5 --source DIR --dest DIR [--dry-run] [--force]

Reads ``personas.yaml``, ``personas/<id>/`` and ``behaviours/`` through the v4
loaders and writes ``catalog.yaml``, ``bundles/<id>.yaml`` and
``profiles/<id>.yaml``. Text is kept as v4 reads it (UTF-8, universal
newlines, unstripped); staging is what v4 parses. The tracked ``kanade``
bundle is never written. Only file names are ever printed.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

from bot import behaviour_plugins
from bot.chat import persona_catalog
from bot.chat.persona import declared_voice
from bot.chat.progress import STAGING_KEYS, StagingConfigError, _validate_lines

SCHEMA_VERSION = 1
TRACKED_BUNDLE = persona_catalog.EXAMPLE_ID
EXAMPLE_PROFILE = "example"
COMPACT_NAME = "default-compact.md"
# v4 lookup order; the first directory holding a name wins.
PLUGIN_DIRS = ("behaviours", "behaviours/profiles", "behaviour-plugins")
STAGING_DIRS = ("behaviours/staging", "behaviours/profiles/staging")
_V5_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,49}$")
_HEADER = "# Private v5 persona file converted from v4 by scripts/convert_personas_v5.py.\n"


class ConversionError(ValueError):
    """A source problem that stops the conversion; names only, never content."""


@dataclass
class Plan:
    outputs: dict[str, dict[str, Any]] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)


def _read(path: Path) -> str:
    # Same decoding as v4 (`read_text`): CRLF becomes LF before the prompt sees it.
    return path.read_text(encoding="utf-8")


def _v5_alias_ok(alias: str) -> bool:
    return (
        0 < len(alias) <= 100
        and alias not in {".", ".."}
        and "/" not in alias
        and "\\" not in alias
        and not any(unicodedata.category(char) == "Cc" for char in alias)
    )


def _catalog(catalog: persona_catalog.PersonaCatalog, plan: Plan) -> dict[str, Any]:
    entries = []
    for descriptor in catalog.descriptors:
        aliases = [alias for alias in descriptor.aliases if _v5_alias_ok(alias)]
        for alias in descriptor.aliases:
            if alias not in aliases:
                plan.notes.append(f"catalog {descriptor.id}: dropped an alias rejected by v5")
        entries.append({"id": descriptor.id, "label": descriptor.label, "aliases": aliases})
    return {"schema_version": SCHEMA_VERSION, "default": catalog.default_id, "personas": entries}


def _bundle(
    source: Path, catalog: persona_catalog.PersonaCatalog, identifier: str, plan: Plan
) -> dict[str, Any]:
    try:
        bundle = persona_catalog.prepare_runtime(catalog, identifier).bundle
    except persona_catalog.PersonaCatalogError as exc:
        raise ConversionError(f"persona {identifier!r} does not load in v4") from exc
    behaviour: dict[str, str] = {}
    # v4 falls back from the behaviour's voice to the identity's before its default cue.
    voice = declared_voice(bundle.default_behaviour) or declared_voice(bundle.identity)
    if voice:
        behaviour["voice"] = voice
    else:
        plan.notes.append(f"bundle {identifier}: no declared voice (default cue applies)")
    behaviour["prompt"] = _read(bundle.source.default_behaviour_path)
    document: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "id": identifier,
        "identity": _read(bundle.source.identity_path),
        "behaviour": behaviour,
        "staging": {key: bundle.staging.for_state(key) for key in STAGING_KEYS},
    }
    compact = source / "personas" / identifier / COMPACT_NAME
    if compact.is_file() and not compact.is_symlink() and _read(compact).strip():
        document["compact"] = {"header_rewrite": _read(compact)}
    else:
        plan.notes.append(f"bundle {identifier}: no {COMPACT_NAME}")
    return document


def _profile_staging(source: Path, plan: Plan) -> dict[str, dict[str, str]]:
    found: dict[str, dict[str, str] | None] = {}
    for relative in STAGING_DIRS:
        try:
            paths = sorted((source / relative).iterdir())
        except OSError:
            continue
        for path in paths:
            if (
                path.name == "example.yaml"
                or not path.is_file()
                or path.suffix not in {".yaml", ".yml"}
                or path.stem in found
            ):
                continue
            try:
                found[path.stem] = _validate_lines(yaml.safe_load(_read(path)), path.stem)
            except (StagingConfigError, UnicodeError, yaml.YAMLError):
                # v4 logs and ignores an invalid override; v5 would reject the whole profile.
                found[path.stem] = None
                plan.notes.append(f"profile {path.stem}: invalid v4 staging ignored")
    return {name: lines for name, lines in found.items() if lines}


def _profiles(source: Path, plan: Plan) -> dict[str, dict[str, Any]]:
    staging = _profile_staging(source, plan)
    documents: dict[str, dict[str, Any]] = {}
    seen: set[str] = set()
    for relative in PLUGIN_DIRS:
        directory = source / relative
        for name in behaviour_plugins.available(directory):
            if name in seen:
                continue
            seen.add(name)
            try:
                behaviour_plugins.plugin_name(name)
            except ValueError:
                plan.notes.append(f"profile {name}: reserved v4 name skipped")
                continue
            plugin = behaviour_plugins.read(name, directory)
            if plugin is None:
                plan.notes.append(f"profile {name}: empty, skipped")
                continue
            identifier = name.replace("_", "-")
            if not _V5_ID_RE.fullmatch(identifier) or identifier == EXAMPLE_PROFILE:
                raise ConversionError(f"profile {name!r} has no valid v5 ID")
            if identifier in documents:
                raise ConversionError(f"profile {name!r} collides with v5 ID {identifier!r}")
            if identifier != name:
                plan.notes.append(f"profile {name}: renamed to {identifier}")
            document: dict[str, Any] = {
                "schema_version": SCHEMA_VERSION,
                "id": identifier,
                "label": name,
            }
            voice = declared_voice(plugin.instructions)
            if voice:
                document["voice"] = voice
            else:
                plan.notes.append(f"profile {name}: no declared voice")
            document["prompt"] = _read(directory / f"{name}.md")
            if name in staging:
                document["staging"] = staging[name]
            else:
                plan.notes.append(f"profile {name}: no staging override")
            documents[identifier] = document
    for name in sorted(set(staging) - seen):
        plan.notes.append(f"profile staging {name}: no matching profile, skipped")
    return documents


def build_plan(source: Path) -> Plan:
    if not (source / persona_catalog.MANIFEST_NAME).is_file():
        raise ConversionError(f"{persona_catalog.MANIFEST_NAME} is required")
    try:
        catalog = persona_catalog.load_catalog(source)
    except persona_catalog.PersonaCatalogError as exc:
        raise ConversionError("the v4 manifest is invalid") from exc
    plan = Plan()
    plan.outputs["catalog.yaml"] = _catalog(catalog, plan)
    for descriptor in catalog.descriptors:
        if descriptor.id == TRACKED_BUNDLE:
            plan.notes.append(f"bundle {TRACKED_BUNDLE}: tracked in v5, not converted")
            continue
        plan.outputs[f"bundles/{descriptor.id}.yaml"] = _bundle(
            source, catalog, descriptor.id, plan
        )
    for identifier, document in _profiles(source, plan).items():
        plan.outputs[f"profiles/{identifier}.yaml"] = document
    return plan


class _Dumper(yaml.SafeDumper):
    pass


def _represent_str(dumper: yaml.SafeDumper, value: str) -> yaml.Node:
    # PyYAML falls back to a quoted style when a literal block cannot hold the text.
    style = "|" if "\n" in value else None
    return dumper.represent_scalar("tag:yaml.org,2002:str", value, style=style)


_Dumper.add_representer(str, _represent_str)


def render(document: dict[str, Any]) -> str:
    text = _HEADER + yaml.dump(
        document,
        Dumper=_Dumper,
        sort_keys=False,
        allow_unicode=True,
        width=float("inf"),
        default_flow_style=False,
    )
    if yaml.safe_load(text) != document:
        raise ConversionError("rendered YAML does not round-trip")
    return text


def _write_private(path: Path, text: str) -> None:
    if not path.parent.exists():
        path.parent.mkdir(mode=0o700, parents=True)
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, prefix=".convert-", suffix=".tmp")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def convert(source: Path, dest: Path, *, force: bool = False, dry_run: bool = False) -> Plan:
    plan = build_plan(source)
    rendered = {name: render(document) for name, document in plan.outputs.items()}
    existing = sorted(name for name in rendered if (dest / name).exists())
    if dry_run:
        return plan
    if existing and not force:
        raise ConversionError(f"refusing to overwrite without --force: {', '.join(existing)}")
    for name, text in rendered.items():
        _write_private(dest / name, text)
    return plan


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--dest", type=Path, required=True)
    parser.add_argument("--force", action="store_true", help="overwrite existing outputs")
    parser.add_argument("--dry-run", action="store_true", help="print the plan, write nothing")
    args = parser.parse_args(argv)
    try:
        plan = convert(args.source, args.dest, force=args.force, dry_run=args.dry_run)
    except ConversionError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    verb = "would write" if args.dry_run else "wrote"
    for name in plan.outputs:
        exists = " (exists)" if args.dry_run and (args.dest / name).exists() else ""
        print(f"{verb} {name}{exists}")
    for note in plan.notes:
        print(f"note: {note}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
