"""Strict, local boss-guide knowledge documents.

The knowledge base only validates and renders the checked-in documents.  It
never fills gaps with generated mechanics or fetches material from a source.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from types import MappingProxyType
from urllib.parse import urlparse

import yaml

from bot.domain.bosses import BossReference, BossTable

_REQUIRED_FIELDS = {"boss", "summary", "core", "danger", "tips", "sources"}
_OPTIONAL_FIELDS = {"difficulty_notes", "notes"}
_META_FIELDS = {"schema_version", "researched_as_of", "intended_use"}
_MAX_TEXT_LENGTH = 500
_MAX_DOCUMENT_LENGTH = 4000


class BossKnowledgeError(ValueError):
    """Raised when strict boss knowledge documents are malformed."""


class _UniqueKeyLoader(yaml.SafeLoader):
    """Safe YAML loader which turns duplicate mappings into a clear error."""


def _construct_unique_mapping(loader: _UniqueKeyLoader, node: yaml.MappingNode, deep: bool = False):
    mapping: dict[object, object] = {}
    for key_node, value_node in node.value:
        if not isinstance(key_node, yaml.ScalarNode):
            raise BossKnowledgeError("mapping keys must be hashable scalars")
        try:
            key = loader.construct_object(key_node, deep=deep)
            hash(key)
        except TypeError as exc:
            raise BossKnowledgeError("mapping keys must be hashable scalars") from exc
        if key in mapping:
            raise BossKnowledgeError(f"duplicate YAML key {key!r}")
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


_UniqueKeyLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _construct_unique_mapping
)


def _load_yaml(path: Path) -> object:
    try:
        return yaml.load(path.read_text(encoding="utf-8"), Loader=_UniqueKeyLoader)
    except FileNotFoundError as exc:
        raise BossKnowledgeError(f"required knowledge file is missing: {path.name}") from exc
    except BossKnowledgeError as exc:
        raise BossKnowledgeError(f"{path.name}: {exc}") from exc
    except yaml.YAMLError as exc:
        raise BossKnowledgeError(f"{path.name}: invalid YAML: {exc}") from exc


def _mapping(value: object, label: str) -> dict:
    if not isinstance(value, dict):
        raise BossKnowledgeError(f"{label} must be a map")
    return value


def _text(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise BossKnowledgeError(f"{label} must be text")
    value = value.strip()
    if not value:
        raise BossKnowledgeError(f"{label} must not be empty")
    if len(value) > _MAX_TEXT_LENGTH:
        raise BossKnowledgeError(f"{label} must be at most {_MAX_TEXT_LENGTH} characters")
    return value


def _bullets(value: object, label: str, minimum: int, maximum: int) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise BossKnowledgeError(f"{label} must be a list")
    if not minimum <= len(value) <= maximum:
        raise BossKnowledgeError(f"{label} must contain {minimum}..{maximum} items")
    return tuple(_text(item, f"{label}[{index}]") for index, item in enumerate(value, start=1))


def _sources(value: object, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not 1 <= len(value) <= 10:
        raise BossKnowledgeError(f"{label} must contain 1..10 items")
    sources: list[str] = []
    for index, source in enumerate(value, start=1):
        if not isinstance(source, str):
            raise BossKnowledgeError(f"{label}[{index}] must be text")
        source = source.strip()
        if not source:
            raise BossKnowledgeError(f"{label}[{index}] must not be empty")
        sources.append(source)
    for source in sources:
        parsed = urlparse(source)
        if len(source) > 2048 or parsed.scheme != "https" or not parsed.netloc:
            raise BossKnowledgeError(
                f"{label} must contain HTTPS URL(s) no longer than 2048 characters"
            )
    return tuple(sources)


@dataclass(frozen=True)
class BossKnowledgeMeta:
    """Metadata shared by the complete knowledge collection."""

    schema_version: int
    researched_as_of: date
    intended_use: str | None = None


@dataclass(frozen=True)
class BossKnowledge:
    """Validated, source-backed knowledge for one canonical boss."""

    boss: str
    summary: str
    core: tuple[str, ...]
    danger: tuple[str, ...]
    tips: tuple[str, ...]
    sources: tuple[str, ...]
    difficulty_notes: Mapping[str, str]
    notes: tuple[str, ...]


@dataclass(frozen=True)
class BossKnowledgeBase:
    """An immutable complete set of per-boss knowledge documents."""

    table: BossTable
    meta: BossKnowledgeMeta
    documents: Mapping[str, BossKnowledge]

    @classmethod
    def load(cls, path: str | Path, table: BossTable) -> BossKnowledgeBase:
        directory = Path(path)
        if not directory.is_dir():
            raise BossKnowledgeError(f"knowledge directory does not exist: {directory}")
        meta = _parse_meta(directory / "_meta.yaml")
        files = [
            item
            for item in directory.iterdir()
            if item.is_file() and item.suffix in {".yaml", ".yml"}
        ]
        knowledge_files = [item for item in files if item.name != "_meta.yaml"]

        expected = {short.lower(): short for short in table.bosses}
        seen = {item.stem for item in knowledge_files}
        duplicate_ids = sorted(
            stem for stem in seen if sum(item.stem == stem for item in knowledge_files) != 1
        )
        missing = sorted(set(expected) - seen)
        extra = sorted(seen - set(expected))
        uppercase = sorted(item.name for item in knowledge_files if item.stem != item.stem.lower())
        if missing or extra or uppercase or duplicate_ids:
            details = []
            if missing:
                details.append(f"missing: {', '.join(missing)}")
            if extra:
                details.append(f"extra: {', '.join(extra)}")
            if uppercase:
                details.append(f"filenames must be lowercase: {', '.join(uppercase)}")
            if duplicate_ids:
                details.append(f"duplicate ids: {', '.join(duplicate_ids)}")
            raise BossKnowledgeError("knowledge coverage invalid (" + "; ".join(details) + ")")

        documents: dict[str, BossKnowledge] = {}
        for item in sorted(knowledge_files):
            short = expected[item.stem]
            documents[short] = _parse_document(item, short, table)
        return cls(table=table, meta=meta, documents=MappingProxyType(documents))

    def get(self, short: str) -> BossKnowledge:
        try:
            return self.documents[short]
        except KeyError as exc:
            raise BossKnowledgeError(f"no knowledge document for {short!r}") from exc

    def render(self, reference: BossReference | str, difficulty: str | None = None) -> str:
        """Return local Markdown for one boss, with no generated or fetched facts."""
        if isinstance(reference, BossReference):
            if difficulty is not None and difficulty != reference.difficulty:
                raise BossKnowledgeError("conflicting requested difficulties")
            short, difficulty = reference.short, reference.difficulty
        else:
            short = reference
        document = self.get(short)
        boss = self.table.bosses.get(short)
        if boss is None:
            raise BossKnowledgeError(f"{short!r} is not in the boss catalog")
        if difficulty is not None:
            difficulty = difficulty.lower()
            if difficulty not in boss.difficulties:
                raise BossKnowledgeError(f"{boss.full} does not support difficulty {difficulty!r}")

        lines = [
            f"# {boss.full} ({boss.short})",
            f"_Researched as of {self.meta.researched_as_of.isoformat()}._",
            "",
            document.summary,
        ]
        for heading, bullets in (
            ("Core", document.core),
            ("Danger", document.danger),
            ("Tips", document.tips),
        ):
            lines.extend(("", f"## {heading}", *(f"- {bullet}" for bullet in bullets)))
        selected = (
            ((difficulty, document.difficulty_notes[difficulty]),)
            if difficulty is not None and difficulty in document.difficulty_notes
            else tuple(document.difficulty_notes.items())
            if difficulty is None
            else ()
        )
        if selected:
            lines.extend(("", "## Difficulty notes"))
            for letter, note in selected:
                lines.extend((f"### {self.table.difficulty_name(letter)}", note))
        if document.notes:
            lines.extend(("", "## Notes", *(f"- {note}" for note in document.notes)))
        lines.extend(("", "## Sources", *(f"- {source}" for source in document.sources)))
        return "\n".join(lines)

    markdown = render
    retrieve = render


def _parse_meta(path: Path) -> BossKnowledgeMeta:
    raw = _mapping(_load_yaml(path), path.name)
    unknown = set(raw) - _META_FIELDS
    missing = {"schema_version", "researched_as_of"} - set(raw)
    if unknown or missing:
        raise BossKnowledgeError(
            f"{path.name} fields invalid (missing: {', '.join(sorted(missing)) or 'none'}; "
            f"unknown: {', '.join(sorted(map(str, unknown))) or 'none'})"
        )
    version = raw["schema_version"]
    if isinstance(version, bool) or not isinstance(version, int) or version != 1:
        raise BossKnowledgeError("_meta.yaml schema_version must be 1")
    researched = raw["researched_as_of"]
    if not isinstance(researched, str):
        raise BossKnowledgeError("_meta.yaml researched_as_of must be an ISO date")
    try:
        researched_date = date.fromisoformat(researched)
    except ValueError as exc:
        raise BossKnowledgeError("_meta.yaml researched_as_of must be an ISO date") from exc
    if researched_date > date.today():
        raise BossKnowledgeError("_meta.yaml researched_as_of must not be in the future")
    intended_use = (
        _text(raw["intended_use"], "_meta.yaml intended_use") if "intended_use" in raw else None
    )
    return BossKnowledgeMeta(1, researched_date, intended_use)


def _parse_document(path: Path, short: str, table: BossTable) -> BossKnowledge:
    raw = _mapping(_load_yaml(path), path.name)
    unknown = set(raw) - _REQUIRED_FIELDS - _OPTIONAL_FIELDS
    missing = _REQUIRED_FIELDS - set(raw)
    if unknown or missing:
        raise BossKnowledgeError(
            f"{path.name} fields invalid (missing: {', '.join(sorted(missing)) or 'none'}; "
            f"unknown: {', '.join(sorted(map(str, unknown))) or 'none'})"
        )
    boss = _text(raw["boss"], f"{path.name} boss")
    if boss != short:
        raise BossKnowledgeError(f"{path.name} boss must be canonical {short!r}, not {boss!r}")
    difficulty_notes: dict[str, str] = {}
    if "difficulty_notes" in raw:
        notes = _mapping(raw["difficulty_notes"], f"{path.name} difficulty_notes")
        for letter, note in notes.items():
            if not isinstance(letter, str) or letter not in table.bosses[short].difficulties:
                raise BossKnowledgeError(f"{path.name} has unsupported difficulty note {letter!r}")
            difficulty_notes[letter] = _text(note, f"{path.name} difficulty_notes.{letter}")
    document = BossKnowledge(
        boss=boss,
        summary=_text(raw["summary"], f"{path.name} summary"),
        core=_bullets(raw["core"], f"{path.name} core", 1, 8),
        danger=_bullets(raw["danger"], f"{path.name} danger", 1, 8),
        tips=_bullets(raw["tips"], f"{path.name} tips", 1, 8),
        sources=_sources(raw["sources"], f"{path.name} sources"),
        difficulty_notes=MappingProxyType(difficulty_notes),
        notes=_bullets(raw.get("notes", []), f"{path.name} notes", 0, 8),
    )
    text = "".join(
        (
            document.boss,
            document.summary,
            *document.core,
            *document.danger,
            *document.tips,
            *document.difficulty_notes.values(),
            *document.notes,
            *document.sources,
        )
    )
    if len(text) > _MAX_DOCUMENT_LENGTH:
        raise BossKnowledgeError(
            f"{path.name} normalized document exceeds {_MAX_DOCUMENT_LENGTH} characters"
        )
    return document


def load_boss_knowledge(path: str | Path, table: BossTable) -> BossKnowledgeBase:
    """Load a complete strict knowledge directory."""
    return BossKnowledgeBase.load(path, table)
