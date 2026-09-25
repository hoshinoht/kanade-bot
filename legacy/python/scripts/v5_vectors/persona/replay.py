"""Replay one persona case through the v4 prompt assembly oracle."""

from __future__ import annotations

from dataclasses import asdict
from datetime import datetime
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

import yaml

from bot.chat import persona, persona_catalog, progress

#: Tracked public v4 templates only; private deployment files are never read.
TEMPLATE_DIR = Path(persona.PERSONA_DIR) / "personas" / persona_catalog.EXAMPLE_ID
TEMPLATE_FILES = {
    "identity": "identity.md",
    "behaviour_prompt": "default.md",
    "staging": "staging.yaml",
}


def bundle_templates() -> dict[str, Any]:
    """Raw template bytes, exactly as the v5 bundle must carry them."""
    raw = {
        key: (TEMPLATE_DIR / name).read_text(encoding="utf-8")
        for key, name in TEMPLATE_FILES.items()
    }
    return {
        "id": persona_catalog.EXAMPLE_ID,
        "identity": raw["identity"],
        "behaviour_prompt": raw["behaviour_prompt"],
        "staging": yaml.safe_load(raw["staging"]),
    }


def replay(case: dict[str, Any]) -> dict[str, Any]:
    data = case["input"]
    if data["bundle_id"] != persona_catalog.EXAMPLE_ID:
        raise ValueError(f"{case['case_id']}: only the tracked public bundle is replayable")
    bundle = persona_catalog.load_example_bundle(persona.PERSONA_DIR)
    clock = data["clock"]
    header = persona.clock_header(
        datetime.fromisoformat(clock["now"]),
        ZoneInfo(clock["timezone"]),
        datetime.fromisoformat(clock["week_start"]),
    )
    runtime = persona.runtime_line(data["model"])
    focus = persona.focus_line(data["focus_card"])
    profile = data["profile"]
    # v4 strips stored plugin instructions before using them as the active profile.
    active = profile["markdown"].strip() if profile else ""
    staging = bundle.staging
    if profile and profile["staging"] is not None:
        staging = progress.parse_profile_staging(profile["staging"], bundle.staging)
    components = persona.PromptComponents(
        identity=bundle.identity,
        default_behaviour=bundle.default_behaviour,
        active_profile=active,
    )
    return {
        "header": header,
        "runtime": runtime,
        "focus": focus,
        "system_prompt": persona.component_system_prompt(components, header, runtime, focus),
        "voice_reminder": persona.component_voice_reminder(
            bundle.default_behaviour, active, bundle.identity
        ),
        "staging": asdict(staging),
    }
