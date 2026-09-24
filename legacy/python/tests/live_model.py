"""Gate for `live_model` tests: skip unless a real gateway is explicitly configured."""

from __future__ import annotations

import os

import pytest

GATEWAY_KEYS = ("KANATA_BASE_URL", "KANATA_API_KEY_FILE")


def require_live_gateway(*alias_keys: str) -> dict[str, str]:
    """The process-environment values for the gateway and aliases, or skip."""
    wanted = (*GATEWAY_KEYS, *alias_keys)
    missing = [key for key in wanted if not os.environ.get(key, "").strip()]
    if missing:
        pytest.skip(f"live model gateway not configured: set {', '.join(missing)}")
    return {key: os.environ[key].strip() for key in wanted}
