"""Shared compact schedule and proposal display formatting."""

from __future__ import annotations

from typing import Any

from bot.agent import formatting
from bot.domain.ids import short_id

from ...api import service
from .clock import utcnow


def is_over(run: dict) -> bool:
    """Whether a run is finished or in the past, using the facade clock seam."""
    return run["status"] == "done" or run["datetime"] <= utcnow()


def channel_reference(bot: Any, channel_id: Any) -> str | None:
    """A clickable Discord channel reference, only for a visible channel."""
    if service.channel_name(bot, channel_id) is None:
        return None
    try:
        canonical_id = int(channel_id)
    except (TypeError, ValueError):  # pragma: no cover - channel_name already rejected it
        return None
    return f"<#{canonical_id}>"


def run_line(bot: Any, run: dict, with_channel: bool = False) -> str:
    """Render one compact schedule line."""
    local = run["datetime"].astimezone(bot.tz)
    rsvps = bot.repo.get_rsvps(run["id"])
    yes = sum(1 for uid in run["participants"] if rsvps.get(uid) == "yes")
    # Only on a guild-wide listing, and only when the bot can actually see the
    # channel. An unresolved `<#id>` would be worse than saying nothing.
    where = channel_reference(bot, run["channel_id"]) if with_channel else None
    return (
        f"`[{short_id(run['id'])}]` *{local.strftime('%a %d %b %H:%M')}* "
        f"**{formatting.boss_labels(run['bosses'])}** "
        f"(`{run['status']}`, `{yes}/{len(run['participants'])} yes`)"
        + (f" · {where}" if where else "")
        + (" — *already happened*" if is_over(run) else "")
    )


def run_detail(bot: Any, run: dict) -> str:
    """Render the full read-model view for a single run."""
    view = service.run_view(bot, run)
    people = ", ".join(f"{p['name']} (`{p['rsvp'] or 'no answer'}`)" for p in view["participants"])
    return (
        f"**Run `[{view['short_id']}]`**\n\n"
        f"**{formatting.boss_labels(view['bosses'])}** · "
        f"*{view['local_day']} {view['local_time']}* · `{view['status']}`\n"
        f"On it: {people or '*nobody*'}."
    )
