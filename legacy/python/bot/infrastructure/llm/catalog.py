"""The live alias list for the portal's model picker."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from .capabilities import Capabilities
from .client import KanataClient

if TYPE_CHECKING:
    from bot.infrastructure.config import Settings

#: The picker waits this long; a save that cannot verify its alias is refused.
CATALOG_TIMEOUT_S = 3.0


async def model_catalog(settings: Settings, **client_kwargs: Any) -> dict[str, Capabilities]:
    """Fetch ``GET /v1/models`` with the server-side key. Raises :class:`ModelError`."""
    client = KanataClient.from_settings(settings, CATALOG_TIMEOUT_S, **client_kwargs)
    try:
        return await client.model_entries()
    finally:
        await client.close()
