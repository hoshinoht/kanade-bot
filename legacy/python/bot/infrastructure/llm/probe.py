"""Authenticated reachability check for `/debug status` and live-test gating."""

from __future__ import annotations

import time
from typing import TYPE_CHECKING, Any

from .client import KanataClient
from .errors import ModelError

if TYPE_CHECKING:
    from bot.infrastructure.config import Settings


async def gateway_status(
    settings: Settings, timeout: float = 3.0, **client_kwargs: Any
) -> tuple[bool, str]:
    """``GET /v1/models``, then whether each configured alias is listed."""
    started = time.monotonic()
    try:
        client = KanataClient.from_settings(settings, timeout, **client_kwargs)
    except ModelError as exc:
        return False, str(exc)
    try:
        listed = set(await client.models())
    except ModelError as exc:
        return False, str(exc)
    finally:
        await client.close()
    detail = [f"reachable ({(time.monotonic() - started) * 1000:.0f} ms)"]
    for label, alias in (
        ("extract", settings.extract_model),
        ("chat", settings.chat_pilot_model),
    ):
        if alias:
            detail.append(f"{label} `{alias}` {'listed' if alias in listed else 'NOT listed'}")
    return True, " · ".join(detail)
