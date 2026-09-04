"""Datetime helpers.

Everything is stored as ISO-8601 UTC (``...+00:00``); conversion to the guild
timezone happens only at the edges (formatting, parsing user input).
"""

from __future__ import annotations

from datetime import UTC, datetime
from zoneinfo import ZoneInfo


def utcnow() -> datetime:
    """Timezone-aware "now" in UTC."""
    return datetime.now(UTC)


def to_iso(dt: datetime) -> str:
    """Serialise an aware datetime to an ISO-8601 UTC string."""
    if dt.tzinfo is None:
        raise ValueError("refusing to serialise a naive datetime")
    return dt.astimezone(UTC).isoformat()


def from_iso(value: str) -> datetime:
    """Parse an ISO-8601 string back into an aware UTC datetime."""
    dt = datetime.fromisoformat(value)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=UTC)
    return dt.astimezone(UTC)


def to_local(dt: datetime, tz: ZoneInfo) -> datetime:
    """Convert an aware datetime into the guild timezone."""
    return dt.astimezone(tz)


def local_naive(dt: datetime, tz: ZoneInfo) -> datetime:
    """Wall-clock time in the guild timezone, without tzinfo."""
    return dt.astimezone(tz).replace(tzinfo=None)
