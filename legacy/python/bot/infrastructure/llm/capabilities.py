"""Per-alias request capabilities published by Kanata's ``GET /v1/models``.

Kanata rejects (HTTP 400) any request field the alias's route cannot honor, so
request bodies are built from what each alias declares. An entry without the
optional ``kanata`` metadata gets :data:`MINIMAL`: model, messages and tools only.
"""

from __future__ import annotations

import logging
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, replace
from typing import Any, Literal

from .errors import ModelError

log = logging.getLogger(__name__)

TrustZone = Literal["local", "private_network", "external"]
TRUST_ZONES: tuple[str, ...] = ("local", "private_network", "external")

#: How long a successful listing is trusted before the next lookup refetches it.
CAPABILITY_TTL_S = 300.0
#: Upper bound for one capability fetch, so a slow gateway cannot stall a model call.
CAPABILITY_TIMEOUT_S = 5.0

#: Optional request fields a 400 ``error.param`` may name, and the capability each needs.
FIELD_CAPABILITY = {
    "response_format": "structured_output",
    "temperature": "sampling_controls",
    "seed": "sampling_controls",
    "top_p": "sampling_controls",
    "max_tokens": "sampling_controls",
    "reasoning_effort": "reasoning_control",
}


@dataclass(frozen=True)
class Capabilities:
    """Which optional request fields an alias accepts."""

    structured_output: bool = False
    sampling_controls: bool = False
    reasoning_control: bool = False
    function_tools: bool = True
    trust_zone: TrustZone | None = None
    #: Whether the gateway published metadata for this alias at all.
    declared: bool = False
    #: Allowed ``reasoning_effort`` values; ``None`` lets the model decide what it honours.
    reasoning_efforts: tuple[str, ...] | None = None

    def effort(self, level: str | None) -> str | None:
        """The ``reasoning_effort`` to send for ``level``, or ``None`` to omit it.

        With an explicit list, a level outside it (including ``"none"`` for off)
        is omitted so the alias's own default applies.
        """
        if not self.reasoning_control or not level:
            return None
        if self.reasoning_efforts is None or level in self.reasoning_efforts:
            return level
        return None


#: Absent or unreadable metadata: send nothing optional except tools.
MINIMAL = Capabilities()
#: Legacy contract for injected clients that cannot describe themselves (test doubles).
FULL = Capabilities(True, True, True, True, None, declared=False)


def _flag(meta: dict[str, Any], key: str, default: bool) -> bool:
    value = meta.get(key)
    return value if isinstance(value, bool) else default


def parse_entry(item: dict[str, Any]) -> Capabilities:
    meta = item.get("kanata")
    if not isinstance(meta, dict):
        return MINIMAL
    zone = meta.get("trust_zone")
    efforts = meta.get("reasoning_efforts")
    return Capabilities(
        structured_output=_flag(meta, "structured_output", False),
        sampling_controls=_flag(meta, "sampling_controls", False),
        reasoning_control=_flag(meta, "reasoning_control", False),
        function_tools=_flag(meta, "function_tools", True),
        trust_zone=zone if zone in TRUST_ZONES else None,
        declared=True,
        reasoning_efforts=tuple(e for e in efforts if isinstance(e, str))
        if isinstance(efforts, list)
        else None,
    )


def parse_models(data: Any) -> dict[str, Capabilities] | None:
    """Aliases in listing order, or ``None`` when the payload is not a model list."""
    items = data.get("data") if isinstance(data, dict) else None
    if not isinstance(items, list):
        return None
    out: dict[str, Capabilities] = {}
    for item in items:
        if isinstance(item, dict) and isinstance(item.get("id"), str) and item["id"]:
            out[item["id"]] = parse_entry(item)
    return out


class CapabilityCache:
    """TTL cache over one listing fetch; failures are never cached."""

    def __init__(
        self,
        fetch: Callable[[], Awaitable[dict[str, Capabilities]]],
        ttl: float = CAPABILITY_TTL_S,
        clock: Callable[[], float] = time.monotonic,
    ):
        self._fetch = fetch
        self._ttl = ttl
        self._clock = clock
        self._entries: dict[str, Capabilities] = {}
        self._fetched_at: float | None = None
        #: Capabilities a 400 disproved, per alias, until the stored deadline.
        self._revoked: dict[str, dict[str, float]] = {}

    def _fresh(self) -> bool:
        return self._fetched_at is not None and self._clock() - self._fetched_at < self._ttl

    async def refresh(self) -> dict[str, Capabilities]:
        """Refetch now. Raises :class:`ModelError` and keeps the old entries on failure."""
        entries = await self._fetch()
        self._entries, self._fetched_at = entries, self._clock()
        return entries

    def revoke(self, alias: str, capability: str) -> None:
        """Stop using ``capability`` for ``alias`` for one TTL, whatever the listing says."""
        self._revoked.setdefault(alias, {})[capability] = self._clock() + self._ttl

    def _restricted(self, alias: str, caps: Capabilities) -> Capabilities:
        now = self._clock()
        live = {k: v for k, v in self._revoked.get(alias, {}).items() if v > now}
        if not live:
            self._revoked.pop(alias, None)
            return caps
        self._revoked[alias] = live
        return replace(caps, **dict.fromkeys(live, False))

    async def lookup(self, alias: str) -> Capabilities:
        """The alias's profile; a miss, expiry or failure refetches, then falls back to minimal."""
        if self._fresh() and alias in self._entries:
            return self._restricted(alias, self._entries[alias])
        try:
            entries = await self.refresh()
        except ModelError as exc:
            # The call itself will surface a real outage; minimal is safe for every route.
            log.warning("model capabilities unavailable (%s); sending a minimal request", exc)
            return MINIMAL
        return self._restricted(alias, entries.get(alias, MINIMAL))


async def profile_of(client: Any, alias: str) -> Capabilities:
    """The client's profile for ``alias``; clients without a lookup keep the legacy full body."""
    lookup = getattr(client, "profile", None)
    if lookup is None:
        return FULL
    return await lookup(alias)
