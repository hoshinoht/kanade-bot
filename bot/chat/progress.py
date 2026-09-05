"""Silent staging copy for the chatbot indicator."""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .strategy import route_strategy_intent

log = logging.getLogger(__name__)

STAGING_SCHEDULE = "Checking your runs…"
STAGING_GUIDE = "Reading checked-in notes…"
STAGING_WRITE = "Drafting the proposal card…"
STAGING_GENERIC = "Thinking…"

STAGING_GUIDE_NAMED = "Reading checked-in notes for {boss}…"

STAGING_KEYS = ("schedule", "guide", "guide_named", "write", "generic")

LEGACY_KEYS = {"guide-named": "guide_named"}

_WRITE_HINT_RE = re.compile(
    r"\b(move|moves|moving|cancel|otot|rsvp|weekly|fixed|proposal|amend|reschedule)\b",
    re.IGNORECASE,
)
_SCHEDULE_HINT_RE = re.compile(
    r"\b(what'?s on|what is on|when is|schedule|tonight|today|tomorrow|"
    r"this week|next week|my runs|for me|monday|tuesday|wednesday|thursday|"
    r"friday|saturday|sunday|mon|tue|wed|thu|fri|sat|sun)\b",
    re.IGNORECASE,
)


class StagingConfigError(ValueError):
    pass


@dataclass(frozen=True)
class StagingLines:
    schedule: str
    guide: str
    guide_named: str
    write: str
    generic: str

    def for_state(self, state: str) -> str:
        return getattr(self, state)


DEFAULT_LINES = StagingLines(
    schedule=STAGING_SCHEDULE,
    guide=STAGING_GUIDE,
    guide_named=STAGING_GUIDE_NAMED,
    write=STAGING_WRITE,
    generic=STAGING_GENERIC,
)


def _normalize_key(key: Any) -> str:
    name = str(key or "").strip().lower().replace("-", "_")
    return LEGACY_KEYS.get(name, name)


def _validate_lines(mapping: Any, where: str) -> dict[str, str]:
    if not isinstance(mapping, dict):
        raise StagingConfigError(f"{where} must be a mapping")
    out: dict[str, str] = {}
    for raw_key, value in mapping.items():
        key = _normalize_key(raw_key)
        if key not in STAGING_KEYS:
            raise StagingConfigError(f"{where} has unknown staging key {raw_key!r}")
        if not isinstance(value, str) or not value.strip():
            raise StagingConfigError(f"{where}.{raw_key} must be a non-empty string")
        out[key] = value.strip()
    return out


def parse_staging_config(data: Any) -> tuple[StagingLines, dict[str, StagingLines]]:
    if not isinstance(data, dict):
        raise StagingConfigError("staging config must be a mapping")
    unknown = {k for k in data if k not in ("default", "profiles")}
    if unknown:
        raise StagingConfigError(f"unknown top-level keys: {sorted(unknown)}")
    if "default" not in data:
        raise StagingConfigError("staging config needs a 'default' mapping")
    default_values = _validate_lines(data["default"], "default")
    missing = [k for k in STAGING_KEYS if k not in default_values]
    if missing:
        raise StagingConfigError(f"default is missing staging keys: {missing}")
    default = StagingLines(**{k: default_values[k] for k in STAGING_KEYS})
    profiles: dict[str, StagingLines] = {}
    raw_profiles = data.get("profiles", {})
    if raw_profiles is None:
        raw_profiles = {}
    if not isinstance(raw_profiles, dict):
        raise StagingConfigError("profiles must be a mapping")
    for name, override in raw_profiles.items():
        if not isinstance(name, str) or not name.strip():
            raise StagingConfigError("profile names must be non-empty strings")
        merged = {**default_values, **_validate_lines(override, f"profiles.{name}")}
        profiles[name.strip()] = StagingLines(**{k: merged[k] for k in STAGING_KEYS})
    return default, profiles


def load_staging_config(path: str | Path | None) -> tuple[StagingLines, dict[str, StagingLines]]:
    if not path:
        return DEFAULT_LINES, {}
    try:
        text = Path(path).read_text(encoding="utf-8")
    except OSError:
        return DEFAULT_LINES, {}
    try:
        import yaml
    except ImportError:
        return DEFAULT_LINES, {}
    try:
        data = yaml.safe_load(text)
    except Exception as exc:  # noqa: BLE001
        raise StagingConfigError(f"invalid staging YAML at {path}: {exc}") from exc
    if data is None:
        return DEFAULT_LINES, {}
    if isinstance(data, dict) and "default" not in data and "profiles" not in data:
        base = {k: getattr(DEFAULT_LINES, k) for k in STAGING_KEYS}
        base.update(_validate_lines(data, "default"))
        default = StagingLines(**{k: base[k] for k in STAGING_KEYS})
        return default, {}
    return parse_staging_config(data)


def load_profile_dir(
    profiles_dir: str | Path | None,
    default: StagingLines,
    profiles: dict[str, StagingLines] | None = None,
) -> dict[str, StagingLines]:
    merged: dict[str, StagingLines] = dict(profiles or {})
    if not profiles_dir:
        return merged
    try:
        entries = sorted(Path(profiles_dir).iterdir())
    except OSError:
        return merged
    try:
        import yaml
    except ImportError:
        return merged
    base = {k: getattr(default, k) for k in STAGING_KEYS}
    for entry in entries:
        if not entry.is_file() or entry.suffix not in {".yaml", ".yml"}:
            continue
        if entry.stem == "example":
            continue
        try:
            data = yaml.safe_load(entry.read_text(encoding="utf-8")) or {}
        except Exception as exc:  # noqa: BLE001
            raise StagingConfigError(f"invalid staging YAML at {entry}: {exc}") from exc
        override = _validate_lines(data, f"profiles.{entry.stem}")
        values = {**base, **override}
        merged[entry.stem] = StagingLines(**{k: values[k] for k in STAGING_KEYS})
    return merged


def load_staging_split(
    default_path: str | Path | None,
    profiles_dir: str | Path | None = None,
) -> tuple[StagingLines, dict[str, StagingLines]]:
    default, profiles = load_staging_config(default_path)
    return default, load_profile_dir(profiles_dir, default, profiles)


def staging_linkage(
    profiles: dict[str, StagingLines], available: list[str] | set[str]
) -> tuple[list[str], list[str]]:
    known = set(available or [])
    orphans = sorted(name for name in profiles if name not in known)
    missing = sorted(name for name in known if name not in profiles)
    return orphans, missing


def load_profile_staging(
    config: tuple[StagingLines, dict[str, StagingLines]] | dict,
    profile: str | None,
) -> StagingLines:
    if isinstance(config, dict):
        default, profiles = parse_staging_config(config)
    else:
        default, profiles = config
    if profile and profile in profiles:
        return profiles[profile]
    return default


def placeholder_for(
    text: str,
    bosses: Any,
    bot_user_id: str | None = None,
    self_role_id: str | None = None,
    staging: dict[str, str] | StagingLines | None = None,
) -> str:
    """Pick the silent staging line. Pure, no LLM/DB/Discord."""
    if isinstance(staging, StagingLines):
        table = {
            "schedule": staging.schedule,
            "guide": staging.guide,
            "guide_named": staging.guide_named,
            "write": staging.write,
            "generic": staging.generic,
        }
    else:
        table = dict(staging or {})
        if "guide-named" in table and "guide_named" not in table:
            table["guide_named"] = table["guide-named"]
    generic = table.get("generic", STAGING_GENERIC)
    cleaned = (text or "").strip()
    if not cleaned:
        return generic
    try:
        intent = route_strategy_intent(cleaned, bosses)
    except Exception:  # noqa: BLE001
        intent = None
    if intent is not None and intent.kind == "resolved" and intent.references:
        names = ", ".join(r.short for r in intent.references[:3])
        template = table.get("guide_named", STAGING_GUIDE_NAMED)
        if names:
            try:
                return template.format(boss=names)
            except Exception:  # noqa: BLE001
                return template
        return table.get("guide", STAGING_GUIDE)
    if intent is not None and intent.kind == "unresolved":
        return table.get("guide", STAGING_GUIDE)
    if _WRITE_HINT_RE.search(cleaned):
        return table.get("write", STAGING_WRITE)
    if _SCHEDULE_HINT_RE.search(cleaned):
        return table.get("schedule", STAGING_SCHEDULE)
    return generic


__all__ = [
    "STAGING_GENERIC",
    "STAGING_GUIDE",
    "STAGING_GUIDE_NAMED",
    "STAGING_KEYS",
    "STAGING_SCHEDULE",
    "STAGING_WRITE",
    "DEFAULT_LINES",
    "StagingConfigError",
    "StagingLines",
    "load_profile_staging",
    "load_profile_dir",
    "load_staging_config",
    "load_staging_file",
    "load_staging_split",
    "parse_staging_config",
    "placeholder_for",
    "staging_linkage",
]


def load_staging_file(path: str | Path | None) -> dict[str, str]:
    if not path:
        return {}
    try:
        default, _ = load_staging_config(path)
    except StagingConfigError:
        log.warning("ignoring invalid staging presets at %s", path)
        return {}
    return {
        "schedule": default.schedule,
        "guide": default.guide,
        "guide_named": default.guide_named,
        "guide-named": default.guide_named,
        "write": default.write,
        "generic": default.generic,
    }
