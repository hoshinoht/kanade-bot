"""Persisted behaviour plugins and their Discord role assignments."""

from __future__ import annotations

import json
import re
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

CONFIG_KEY = "chat_role_plugins"
SELECTABLE_CONFIG_KEY = "chat_selectable_plugins"
PERSONA_ROOT = Path(__file__).resolve().parent.parent / "personas"
DEFAULT_PLUGIN_DIR = PERSONA_ROOT / "behaviours" / "profiles"
PLUGIN_DIR = DEFAULT_PLUGIN_DIR
LEGACY_PLUGIN_DIR = PERSONA_ROOT / "behaviour-plugins"
MAX_ROLE_PLUGINS = 20
MAX_PLUGINS = 30
MAX_INSTRUCTIONS_CHARS = 4000
_PLUGIN_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,49}$")
NOT_A_PLUGIN = {"example.md", "README.md"}
RESERVED_NAMES = {"default", "example", "readme"}


@dataclass(frozen=True)
class RolePlugin:
    role_id: str
    plugin: str

    def as_dict(self) -> dict[str, str]:
        return {"role_id": self.role_id, "plugin": self.plugin}


@dataclass(frozen=True)
class Plugin:
    name: str
    instructions: str

    def as_dict(self) -> dict[str, str]:
        return {"name": self.name, "instructions": self.instructions}


@dataclass(frozen=True)
class ConfigIssue:
    index: int | None
    message: str


@dataclass(frozen=True)
class StyleResolution:
    """Resolved behaviour profile with safe projections."""

    selected: str | None
    effective: str
    source: str
    instructions: str
    role_id: str | None = None
    selected_available: bool = True

    def prompt_instructions(self) -> str:
        """The only projection allowed into a model prompt."""
        return self.instructions

    def member_view(self) -> dict[str, str | bool | None]:
        """Return the member-safe preference view."""
        return {"reply_style": self.selected, "available": self.selected_available}

    def admin_view(self) -> dict[str, str | bool | None]:
        return {
            "reply_style": self.selected,
            "reply_style_available": self.selected_available,
            "effective_style": self.effective,
            "style_source": self.source,
            "style_role_id": self.role_id,
        }


def _role_id(value: Any) -> str:
    role_id = str(value or "").strip()
    if not role_id.isdigit() or int(role_id) <= 0 or len(role_id) > 20:
        raise ValueError("role ids must be positive Discord snowflakes (digits only)")
    return role_id


def plugin_name(value: Any) -> str:
    """Return a safe filename stem for a behaviour plugin."""
    name = str(value or "").strip().lower()
    if name.endswith(".md"):
        name = name[:-3]
    if not _PLUGIN_NAME_RE.fullmatch(name):
        raise ValueError("plugin names use 1-50 lowercase letters, numbers, hyphens or underscores")
    if name in RESERVED_NAMES:
        raise ValueError(f"`{name}` is reserved and cannot be a behaviour plugin")
    return name


def available(directory: Path | None = None) -> list[str]:
    """Return available plugin names."""
    if directory is None:
        names = set(_available_in(PLUGIN_DIR))
        if PLUGIN_DIR == DEFAULT_PLUGIN_DIR:
            names.update(_available_in(LEGACY_PLUGIN_DIR))
        return sorted(names)
    return _available_in(directory)


def _available_in(directory: Path) -> list[str]:
    try:
        names = [
            path.stem
            for path in directory.iterdir()
            if path.is_file() and path.suffix == ".md" and path.name not in NOT_A_PLUGIN
        ]
    except OSError:
        return []
    return sorted(name for name in names if _PLUGIN_NAME_RE.fullmatch(name))


def read(name: str, directory: Path | None = None) -> Plugin | None:
    """Read a plugin only after filename membership validation."""
    try:
        safe = plugin_name(name)
    except ValueError:
        return None
    directories = [directory] if directory is not None else [PLUGIN_DIR]
    if directory is None and PLUGIN_DIR == DEFAULT_PLUGIN_DIR:
        directories.append(LEGACY_PLUGIN_DIR)
    for source in directories:
        if safe not in _available_in(source):
            continue
        try:
            instructions = (source / f"{safe}.md").read_text(encoding="utf-8").strip()
        except OSError:
            continue
        if instructions:
            return Plugin(safe, instructions)
    return None


def list_plugins(directory: Path | None = None) -> list[Plugin]:
    return [found for name in available(directory) if (found := read(name, directory)) is not None]


def write(name: Any, instructions: Any, directory: Path | None = None) -> Plugin:
    """Create or replace a plugin atomically on the persona bind mount."""
    directory = PLUGIN_DIR if directory is None else directory
    safe = plugin_name(name)
    text = str(instructions or "").strip()
    if not text:
        raise ValueError("a behaviour plugin needs instructions")
    if len(text) > MAX_INSTRUCTIONS_CHARS:
        raise ValueError(
            f"plugin instructions may contain at most {MAX_INSTRUCTIONS_CHARS} characters"
        )
    existing = available(directory)
    if safe not in existing and len(existing) >= MAX_PLUGINS:
        raise ValueError(f"at most {MAX_PLUGINS} behaviour plugins may be stored")
    try:
        directory.mkdir(parents=True, exist_ok=True)
        destination = directory / f"{safe}.md"
        temporary = directory / f".{safe}.tmp"
        temporary.write_text(text + "\n", encoding="utf-8")
        temporary.replace(destination)
    except OSError as exc:
        raise ValueError(f"could not store behaviour plugin `{safe}`: {exc}") from None
    return Plugin(safe, text)


def delete(name: Any, directory: Path | None = None) -> None:
    directory = PLUGIN_DIR if directory is None else directory
    safe = plugin_name(name)
    if safe not in available(directory):
        raise ValueError(f"behaviour plugin `{safe}` does not exist")
    try:
        (directory / f"{safe}.md").unlink()
    except OSError as exc:
        raise ValueError(f"could not delete behaviour plugin `{safe}`: {exc}") from None


def validate(items: Iterable[Mapping[str, Any]]) -> list[RolePlugin]:
    """Validate role-to-plugin assignments and preserve their order."""
    assignments: list[RolePlugin] = []
    seen: set[str] = set()
    for item in items:
        if not isinstance(item, Mapping):
            raise ValueError("each role assignment must contain role_id and plugin")
        role_id = _role_id(item.get("role_id"))
        plugin = plugin_name(item.get("plugin"))
        if role_id in seen:
            raise ValueError(f"role {role_id} is listed more than once")
        seen.add(role_id)
        assignments.append(RolePlugin(role_id, plugin))
    if len(assignments) > MAX_ROLE_PLUGINS:
        raise ValueError(f"at most {MAX_ROLE_PLUGINS} role plugins may be configured")
    return assignments


def encode(value: Any) -> str:
    assignments = validate(value or [])
    return json.dumps([item.as_dict() for item in assignments], separators=(",", ":"))


def seed_value(raw: str) -> str:
    """Convert ``ROLE_ID=plugin,...`` from the deployment environment to JSON."""
    entries: list[dict[str, str]] = []
    for part in (raw or "").split(","):
        item = part.strip()
        if not item:
            continue
        role_id, separator, plugin = item.partition("=")
        if not separator:
            raise ValueError("CHAT_ROLE_PLUGINS entries must look like ROLE_ID=plugin")
        entries.append({"role_id": role_id, "plugin": plugin})
    return encode(entries)


def decode(raw: str | None) -> list[RolePlugin]:
    """Read every individually valid assignment; one bad entry cannot erase the rest."""
    assignments, _issues = decode_with_issues(raw)
    return assignments


def decode_with_issues(raw: str | None) -> tuple[list[RolePlugin], list[ConfigIssue]]:
    """Parse stored role assignments and retain per-entry diagnostics."""
    if not raw:
        return [], []
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as exc:
        return [], [ConfigIssue(None, f"invalid JSON: {exc.msg}")]
    if not isinstance(value, list):
        return [], [ConfigIssue(None, "assignments must be a list")]
    assignments: list[RolePlugin] = []
    issues: list[ConfigIssue] = []
    seen: set[str] = set()
    for index, item in enumerate(value):
        try:
            parsed = validate([item])[0]
            if parsed.role_id in seen:
                raise ValueError(f"role {parsed.role_id} is listed more than once")
        except (IndexError, TypeError, ValueError) as exc:
            issues.append(ConfigIssue(index, str(exc) or "invalid role assignment"))
            continue
        seen.add(parsed.role_id)
        assignments.append(parsed)
    if len(assignments) > MAX_ROLE_PLUGINS:
        issues.append(
            ConfigIssue(None, f"at most {MAX_ROLE_PLUGINS} role plugins may be configured")
        )
        assignments = assignments[:MAX_ROLE_PLUGINS]
    return assignments, issues


def assignment_diagnostics(
    raw: str | None, directory: Path | None = None
) -> tuple[list[RolePlugin], list[ConfigIssue]]:
    """Return valid assignments and configuration issues."""
    assignments, issues = decode_with_issues(raw)
    issues = list(issues)
    for index, assignment in enumerate(assignments):
        if read(assignment.plugin, directory) is None:
            issues.append(
                ConfigIssue(
                    index,
                    f"role {assignment.role_id}: profile `{assignment.plugin}` is unreadable",
                )
            )
    return assignments, issues


def encode_catalog(names: Iterable[Any]) -> str:
    """Validate and encode the ordered member-selectable profile catalog."""
    kept: list[str] = []
    for value in names:
        name = plugin_name(value)
        if name not in kept:
            kept.append(name)
    if len(kept) > MAX_PLUGINS:
        raise ValueError(f"at most {MAX_PLUGINS} behaviour plugins may be selectable")
    return json.dumps(kept, separators=(",", ":"))


def decode_catalog(raw: str | None) -> list[str]:
    """Read a catalog defensively while preserving valid ordered names."""
    if not raw:
        return []
    try:
        value = json.loads(raw)
    except json.JSONDecodeError:
        return []
    if not isinstance(value, list):
        return []
    kept: list[str] = []
    for item in value:
        try:
            name = plugin_name(item)
        except (TypeError, ValueError):
            continue
        if name not in kept:
            kept.append(name)
    return kept[:MAX_PLUGINS]


def resolve(
    *,
    selected: str | None,
    selectable: Iterable[str],
    assignments: Iterable[RolePlugin],
    role_ids: Iterable[int | str],
    default_instructions: str,
    directory: Path | None = None,
) -> StyleResolution:
    """Resolve an effective profile without exposing source metadata to prompts."""
    held = {str(role_id) for role_id in role_ids}
    for assignment in assignments:
        if assignment.role_id not in held:
            continue
        plugin = read(assignment.plugin, directory)
        if plugin is not None:
            return StyleResolution(
                selected=selected,
                effective=plugin.name,
                source="role",
                instructions=plugin.instructions,
                role_id=assignment.role_id,
                selected_available=_selection_available(selected, selectable, directory),
            )
    available = _selection_available(selected, selectable, directory)
    plugin = read(selected, directory) if selected and available else None
    if plugin is not None:
        return StyleResolution(selected, plugin.name, "member", plugin.instructions)
    return StyleResolution(
        selected=selected,
        effective="default",
        source="default",
        instructions=default_instructions,
        selected_available=available,
    )


def _selection_available(
    selected: str | None, selectable: Iterable[str], directory: Path | None
) -> bool:
    if selected is None:
        return True
    return selected in set(selectable) and read(selected, directory) is not None


def overlay(
    assignments: Iterable[RolePlugin],
    role_ids: Iterable[int | str],
    directory: Path | None = None,
) -> str:
    """Compatibility helper: return the first readable matching role profile."""
    held = {str(role_id) for role_id in role_ids}
    for item in assignments:
        if item.role_id in held and (plugin := read(item.plugin, directory)) is not None:
            return plugin.instructions
    return ""
