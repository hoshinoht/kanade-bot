"""Persisted behaviour plugins and their Discord role assignments."""

from __future__ import annotations

import json
import re
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

CONFIG_KEY = "chat_role_plugins"
PLUGIN_DIR = Path(__file__).resolve().parent.parent / "personas" / "behaviour-plugins"
MAX_ROLE_PLUGINS = 20
MAX_PLUGINS = 30
MAX_INSTRUCTIONS_CHARS = 4000
_PLUGIN_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,49}$")
NOT_A_PLUGIN = {"example.md", "README.md"}


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
    return name


def available(directory: Path | None = None) -> list[str]:
    """Plugin names currently stored on the writable persona bind mount."""
    directory = PLUGIN_DIR if directory is None else directory
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
    """Read one existing plugin by membership, never by a submitted path."""
    directory = PLUGIN_DIR if directory is None else directory
    try:
        safe = plugin_name(name)
    except ValueError:
        return None
    if safe not in available(directory):
        return None
    try:
        instructions = (directory / f"{safe}.md").read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return Plugin(safe, instructions) if instructions else None


def list_plugins(directory: Path | None = None) -> list[Plugin]:
    directory = PLUGIN_DIR if directory is None else directory
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
    """Read stored assignments defensively; a damaged row applies no plugins."""
    if not raw:
        return []
    try:
        value = json.loads(raw)
        if not isinstance(value, list):
            return []
        return validate(value)
    except (TypeError, ValueError, json.JSONDecodeError):
        return []


def overlay(
    assignments: Iterable[RolePlugin],
    role_ids: Iterable[int | str],
    directory: Path | None = None,
) -> str:
    """Combine every matching, readable plugin in assignment order."""
    held = {str(role_id) for role_id in role_ids}
    selected = [read(item.plugin, directory) for item in assignments if item.role_id in held]
    return "\n\n".join(plugin.instructions for plugin in selected if plugin is not None)
