"""Message and embed text.

Pure string building, no Discord objects, so the wording is unit testable.
Participants are rendered as ``<@id>`` mentions; callers that must not ping
(e.g. ``/schedule``) pass ``allowed_mentions=discord.AllowedMentions.none()``.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from typing import Any
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

COLOUR_DAY_OF = 0x5865F2  # blurple
COLOUR_COUNTDOWN = 0xFEE75C  # yellow
COLOUR_ALL_SET = 0x57F287  # green


@dataclass
class Card:
    """A message plus an optional embed, as plain data so wording stays testable.

    ``content`` carries the ``<@id>`` mentions (mentions inside an embed never
    notify anyone); the embed carries the detail.
    """

    content: str
    title: str | None = None
    description: str | None = None
    fields: list[tuple[str, str]] = field(default_factory=list)
    footer: str | None = None
    colour: int = COLOUR_DAY_OF

    @property
    def has_embed(self) -> bool:
        return bool(self.title or self.description or self.fields or self.footer)


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


def boss_detail(bosses: list[str], table: Any | None) -> str:
    """One line per boss with its full in-game name and difficulty, if a table is given."""
    if table is None:
        return f"**{format_bosses(bosses)}**"
    return "\n".join(f"**{name}** · {table.describe(name)}" for name in bosses)


def status_line(run: dict, rsvps: dict[str, str]) -> str:
    label = STATUS_LABEL.get(run["status"], run["status"])
    return f"{label} · {rsvp_tally(run['participants'], rsvps)}"


def unconfirmed(run: dict, rsvps: dict[str, str]) -> list[str]:
    """Participants who have not said ✅ yet - the only people a countdown pings."""
    return [uid for uid in run["participants"] if rsvps.get(uid) != "yes"]


def everyone_on(runs: list[dict]) -> list[str]:
    seen: list[str] = []
    for run in runs:
        for uid in run["participants"]:
            if uid not in seen:
                seen.append(uid)
    return seen


def day_of_card(
    runs: list[dict],
    tz: ZoneInfo,
    rsvps_by_run: dict[str, dict[str, str]],
    table: Any | None = None,
    today: datetime | None = None,
) -> Card:
    """The grouped day-of ping: mentions up top, one embed field per run."""
    when = today or runs[0]["datetime"]
    content = f"📅 **Today — {local_day(when, tz)}**\n{format_participants(everyone_on(runs))}"
    fields: list[tuple[str, str]] = []
    for run in sorted(runs, key=lambda r: r["datetime"]):
        own_time = run["status"] == "otot"
        when_txt = "🕒 own time" if own_time else f"🕘 {local_time(run['datetime'], tz)}"
        name = f"{when_txt}  ·  {format_bosses(run['bosses'])}"
        value = "\n".join(
            [
                boss_detail(run["bosses"], table),
                status_line(run, rsvps_by_run.get(run["id"], {})),
                format_participants(run["participants"]),
            ]
        )
        fields.append((name, value))
    return Card(content=content, fields=fields, footer=REACT_HINT, colour=COLOUR_DAY_OF)


def countdown_card(
    run: dict, minutes: int, tz: ZoneInfo, rsvps: dict[str, str], table: Any | None = None
) -> Card:
    """A countdown pings only the people who haven't ✅'d; everyone else just sees it."""
    pending = unconfirmed(run, rsvps)
    who = format_participants(pending) if pending else "everyone's confirmed ✅"
    content = (
        f"⏰ **{format_bosses(run['bosses'])}** in {format_offset(minutes)} "
        f"({local_time(run['datetime'], tz)}) — {who}"
    )
    return Card(
        content=content,
        description=f"{boss_detail(run['bosses'], table)}\n{status_line(run, rsvps)}",
        footer=REACT_HINT if pending else None,
        colour=COLOUR_COUNTDOWN if pending else COLOUR_ALL_SET,
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
