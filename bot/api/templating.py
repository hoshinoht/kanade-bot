"""The Jinja environment the portal renders through.

Kept apart from :mod:`bot.api.app` so a template can be rendered in a test
without building a whole application, and so every page shares one set of
filters -- the guild timezone in particular, which has to appear on every screen
(DESIGN.md §5) and must never be re-derived per template.
"""

from __future__ import annotations

from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING, Any

from fastapi.templating import Jinja2Templates

from ..ids import short_id
from ..timeutil import from_iso, utcnow
from ..weeks import WEEKDAY_NAMES

if TYPE_CHECKING:  # pragma: no cover
    from ..client import BossBot

#: Pinned so the page cannot change under us, with the integrity hash cdnjs
#: publishes for it. The portal reads and submits fine without htmx -- every
#: action is a real form -- so a blocked CDN degrades rather than breaks.
HTMX_SRC = "https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.10/htmx.min.js"
HTMX_SRI = (
    "sha512-5l65kMjGvrm7EATevCK/SFdMUfIUQfAsQLZY57t7tmsPf5"
    "ZtjrzU8wzSBykpRDhqnMwlKISCrxQTURFULsXdpQ=="
)

#: The nav, in the order the work actually happens: look at the week, fix the
#: baseline, answer what the extractor found, then the diagnostics.
NAV = [
    ("week", "/", "Week"),
    ("fixed", "/fixed", "Fixed"),
    ("inbox", "/inbox", "Inbox"),
    ("extractions", "/extractions", "Extractions"),
    ("members", "/members", "Members"),
    ("reminders", "/reminders", "Reminders"),
    ("config", "/config", "Config"),
]


def local_dt(value: Any, bot: BossBot, fmt: str = "%a %d %b %H:%M") -> str:
    """Render an ISO string or datetime in the guild timezone."""
    if value is None:
        return "—"
    moment = from_iso(value) if isinstance(value, str) else value
    if not isinstance(moment, datetime):  # pragma: no cover - defensive
        return str(value)
    return moment.astimezone(bot.tz).strftime(fmt)


def confidence_band(value: float | None) -> str:
    """Low / mid / high, so the inbox can colour a number without a chart."""
    if value is None:
        return "unknown"
    if value >= 0.85:
        return "high"
    if value >= 0.65:
        return "mid"
    return "low"


def build_templates(directory: Path, bot: BossBot) -> Jinja2Templates:
    templates = Jinja2Templates(directory=str(directory))
    env = templates.env
    env.trim_blocks = True
    env.lstrip_blocks = True
    env.filters["local_dt"] = lambda v, fmt="%a %d %b %H:%M": local_dt(v, bot, fmt)
    env.filters["short_id"] = short_id
    env.filters["confidence_band"] = confidence_band
    env.globals.update(
        {
            "tz_name": bot.settings.tz,
            "guild_id": bot.settings.guild_id,
            "nav": NAV,
            "weekday_names": WEEKDAY_NAMES,
            "htmx_src": HTMX_SRC,
            "htmx_sri": HTMX_SRI,
            "now": utcnow,
        }
    )
    return templates


__all__ = ["HTMX_SRC", "HTMX_SRI", "NAV", "build_templates", "confidence_band", "local_dt"]
