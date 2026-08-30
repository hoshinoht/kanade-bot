"""Message and embed text.

Pure string building, no Discord objects, so the wording is unit testable.
Participants are rendered as ``<@id>`` mentions; callers that must not ping
(e.g. ``/schedule``) pass ``allowed_mentions=discord.AllowedMentions.none()``.
"""

from __future__ import annotations

from datetime import datetime
from zoneinfo import ZoneInfo

from .ids import short_id
from .rsvp import EMOJI_NO, EMOJI_YES
from .util import mention
from .weeks import WEEKDAY_NAMES

STATUS_LABEL: dict[str, str] = {
    "planned": "⚠️ unconfirmed",
    "confirmed": "✅ confirmed",
    "at_risk": "❗ at risk",
    "otot": "🕒 own time",
    "done": "🏁 done",
    "cancelled": "🚫 cancelled",
}

REACT_HINT = f"React {EMOJI_YES} if you're on, {EMOJI_NO} if not."


def format_bosses(bosses: list[str]) -> str:
    return " + ".join(bosses) if bosses else "(no bosses)"


def format_participants(participants: list[str]) -> str:
    return " ".join(mention(uid) for uid in participants) if participants else "(nobody)"


def format_offset(minutes: int) -> str:
    """``60`` -> ``1h``, ``90`` -> ``1h30m``, ``15`` -> ``15m``."""
    hours, mins = divmod(int(minutes), 60)
    if hours and mins:
        return f"{hours}h{mins}m"
    if hours:
        return f"{hours}h"
    return f"{mins}m"


def local_time(run_at: datetime, tz: ZoneInfo) -> str:
    return run_at.astimezone(tz).strftime("%H:%M")


def local_day(run_at: datetime, tz: ZoneInfo) -> str:
    local = run_at.astimezone(tz)
    return f"{WEEKDAY_NAMES[local.weekday()]} {local.day:02d} {local.strftime('%b')}"


def rsvp_tally(participants: list[str], rsvps: dict[str, str]) -> str:
    yes = sum(1 for uid in participants if rsvps.get(uid) == "yes")
    no = sum(1 for uid in participants if rsvps.get(uid) == "no")
    text = f"{yes}/{len(participants)} {EMOJI_YES}"
    if no:
        text += f" · {no} {EMOJI_NO}"
    return text


def day_of_line(run: dict, tz: ZoneInfo) -> str:
    """One line of the grouped morning ping, tagging only that run's people."""
    bosses = format_bosses(run["bosses"])
    who = format_participants(run["participants"])
    if run["status"] == "otot":
        return f"• **{bosses}** — 🕒 own time — {who}"
    line = f"• **{bosses}** @ {local_time(run['datetime'], tz)} — {who}"
    if run["status"] == "planned":
        line += " ⚠️ unconfirmed"
    elif run["status"] == "at_risk":
        line += " ❗ at risk"
    return line


def day_of_message(runs: list[dict], tz: ZoneInfo, today: datetime | None = None) -> str:
    """The grouped day-of ping: one message, one line per run."""
    when = today or (runs[0]["datetime"] if runs else None)
    header = "📅 **Today**" if when is None else f"📅 **Today — {local_day(when, tz)}**"
    body = "\n".join(day_of_line(run, tz) for run in runs)
    return f"{header}\n{body}\n{REACT_HINT}"


def countdown_message(run: dict, minutes: int, tz: ZoneInfo) -> str:
    return (
        f"⏰ **{format_bosses(run['bosses'])}** in {format_offset(minutes)} "
        f"({local_time(run['datetime'], tz)}) — {format_participants(run['participants'])}\n"
        f"{REACT_HINT}"
    )


def decline_notice(run: dict, who_declined: str, display_name: str, tz: ZoneInfo) -> str:
    """Posted as a reply when someone reacts ❌."""
    others = [uid for uid in run["participants"] if uid != str(who_declined)]
    tag = format_participants(others) if others else ""
    return (
        f"{tag} {display_name} can't make **{format_bosses(run['bosses'])}** "
        f"({local_day(run['datetime'], tz)} {local_time(run['datetime'], tz)}) — "
        f"reschedule? `/amend run_id:{short_id(run['id'])} to:...`"
    ).strip()


def schedule_line(run: dict, tz: ZoneInfo, rsvps: dict[str, str]) -> str:
    local = run["datetime"].astimezone(tz)
    when = "own time" if run["status"] == "otot" else local.strftime("%H:%M")
    parts = [
        f"`#{short_id(run['id'])}`",
        when,
        f"**{format_bosses(run['bosses'])}**",
        format_participants(run["participants"]),
        STATUS_LABEL.get(run["status"], run["status"]),
        rsvp_tally(run["participants"], rsvps),
    ]
    return " · ".join(parts)


def group_by_day(runs: list[dict], tz: ZoneInfo) -> list[tuple[str, list[dict]]]:
    """Group runs into ``(day heading, runs)`` pairs, in chronological order."""
    groups: dict[str, list[dict]] = {}
    order: list[str] = []
    for run in sorted(runs, key=lambda r: r["datetime"]):
        key = local_day(run["datetime"], tz)
        if key not in groups:
            groups[key] = []
            order.append(key)
        groups[key].append(run)
    return [(key, groups[key]) for key in order]


def amend_notice(run: dict, old_at: datetime, tz: ZoneInfo) -> str:
    return (
        f"🔁 **{format_bosses(run['bosses'])}** moved: "
        f"~~{local_day(old_at, tz)} {local_time(old_at, tz)}~~ → "
        f"**{local_day(run['datetime'], tz)} {local_time(run['datetime'], tz)}** — "
        f"{format_participants(run['participants'])}\n{REACT_HINT}"
    )


def fixed_run_line(fixed: dict, table: object | None = None) -> str:
    day = WEEKDAY_NAMES[fixed["weekday"]]
    where = f" · <#{fixed['channel_id']}>" if fixed.get("channel_id") else ""
    line = (
        f"`#{short_id(fixed['id'])}` **{format_bosses(fixed['bosses'])}** · "
        f"{day} {fixed['time']} · "
        f"{format_participants(fixed['participants'])} · owner {mention(fixed['owner_id'])}"
        f"{where}"
    )
    if table is not None:
        # Full in-game names, so "HFA" is unambiguous to someone new to the party.
        line += f"\n   ↳ {table.describe_all(fixed['bosses'])}"
    return line
