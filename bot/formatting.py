"""Message and embed text.

Pure string building, no Discord objects, so the wording is unit testable.

Who appears as a ``<@id>`` mention and who appears as a plain name is decided by
the :class:`Audience` a caller passes in -- built by :func:`bot.pings.audience`
from the mention policy in DESIGN.md §3. Without one (older call sites, and the
unit tests that only care about wording) everybody is rendered as a mention, and
``allowed_mentions`` is still the thing that decides who is actually notified.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
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
    #: An image to attach and use as the embed's thumbnail -- a boss portrait
    #: from ``config/portraits``. ``None`` (the usual case) means no attachment
    #: at all, so a guild that ships no portraits sees exactly what it did before.
    thumbnail_path: Path | None = None
    #: The user ids this card may actually notify, already resolved against the
    #: mention policy (:mod:`bot.pings`). ``BossBot._post`` turns it into the
    #: message's ``allowed_mentions``, so a card cannot ping anyone its own
    #: wording does not account for.
    mention_users: list[str] = field(default_factory=list)

    @property
    def has_embed(self) -> bool:
        return bool(
            self.title or self.description or self.fields or self.footer or self.thumbnail_path
        )


@dataclass(frozen=True)
class Audience:
    """Who a message names, and which of them it is allowed to notify.

    ``mentioned`` is the resolved list from :func:`bot.pings.resolve_mentions`;
    everyone else is rendered from ``names``. A person with no name on file
    falls back to a ``<@id>``, which Discord still renders as their display
    name -- and, absent from ``allowed_mentions``, still does not notify them.
    """

    names: Mapping[str, str] = field(default_factory=dict)
    mentioned: tuple[str, ...] = ()

    def renders_as_mention(self, user_id: int | str) -> bool:
        return str(user_id) in self.mentioned

    def name_for(self, user_id: int | str) -> str:
        uid = str(user_id)
        if uid in self.mentioned:
            return mention(uid)
        return self.names.get(uid) or mention(uid)

    def render(self, user_ids: Iterable[int | str]) -> str:
        people = [self.name_for(uid) for uid in user_ids]
        return " ".join(people) if people else "(nobody)"


def format_bosses(bosses: list[str]) -> str:
    return " + ".join(bosses) if bosses else "(no bosses)"


def format_participants(participants: list[str], who: Audience | None = None) -> str:
    """The people on a run: mentions for whoever ``who`` says may be notified.

    With no ``who`` everyone is a mention, which is what the card unit tests and
    any caller that has not been given an audience expect.
    """
    if who is not None:
        return who.render(participants)
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


def lead_portrait(bosses: list[str], table: Any | None) -> Path | None:
    """The portrait for the first boss on a card, if the guild has one.

    One thumbnail per message, and the first boss is the one the message is
    named after -- "HStar + HFA" is the star run.
    """
    if table is None or not bosses:
        return None
    getter = getattr(table, "portrait_for", None)
    return getter(bosses[0]) if getter else None


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
    who: Audience | None = None,
) -> Card:
    """The grouped day-of ping: mentions up top, one embed field per run."""
    when = today or runs[0]["datetime"]
    content = f"📅 **Today — {local_day(when, tz)}**\n{format_participants(everyone_on(runs), who)}"
    fields: list[tuple[str, str]] = []
    for run in sorted(runs, key=lambda r: r["datetime"]):
        own_time = run["status"] == "otot"
        when_txt = "🕒 own time" if own_time else f"🕘 {local_time(run['datetime'], tz)}"
        name = f"{when_txt}  ·  {format_bosses(run['bosses'])}"
        value = "\n".join(
            [
                boss_detail(run["bosses"], table),
                status_line(run, rsvps_by_run.get(run["id"], {})),
                format_participants(run["participants"], who),
            ]
        )
        fields.append((name, value))
    lead = sorted(runs, key=lambda r: r["datetime"])[0]
    return Card(
        content=content,
        fields=fields,
        footer=REACT_HINT,
        colour=COLOUR_DAY_OF,
        thumbnail_path=lead_portrait(lead["bosses"], table),
        mention_users=list(who.mentioned) if who else [],
    )


def countdown_card(
    run: dict,
    minutes: int,
    tz: ZoneInfo,
    rsvps: dict[str, str],
    table: Any | None = None,
    who: Audience | None = None,
) -> Card:
    """A countdown pings only the people who haven't ✅'d; everyone else just sees it."""
    pending = unconfirmed(run, rsvps)
    waiting = format_participants(pending, who) if pending else "everyone's confirmed ✅"
    content = (
        f"⏰ **{format_bosses(run['bosses'])}** in {format_offset(minutes)} "
        f"({local_time(run['datetime'], tz)}) — {waiting}"
    )
    return Card(
        content=content,
        description=f"{boss_detail(run['bosses'], table)}\n{status_line(run, rsvps)}",
        footer=REACT_HINT if pending else None,
        colour=COLOUR_COUNTDOWN if pending else COLOUR_ALL_SET,
        thumbnail_path=lead_portrait(run["bosses"], table),
        mention_users=list(who.mentioned) if who else [],
    )


def decline_notice(
    run: dict,
    who_declined: str,
    display_name: str,
    tz: ZoneInfo,
    who: Audience | None = None,
) -> str:
    """Posted as a reply when someone reacts ❌."""
    others = [uid for uid in run["participants"] if uid != str(who_declined)]
    tag = format_participants(others, who) if others else ""
    return (
        f"{tag} {display_name} can't make **{format_bosses(run['bosses'])}** "
        f"({local_day(run['datetime'], tz)} {local_time(run['datetime'], tz)}) — "
        f"reschedule? `/amend run_id:{short_id(run['id'])} to:...`"
    ).strip()


def schedule_line(run: dict, tz: ZoneInfo, rsvps: dict[str, str], delta: str = "") -> str:
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
    if delta:
        # A one-week stand-in is invisible otherwise: the run's party differs
        # from the timing it came from, and that is easy to forget.
        parts.append(f"_{delta}_")
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


def amend_notice(run: dict, old_at: datetime, tz: ZoneInfo, who: Audience | None = None) -> str:
    return (
        f"🔁 **{format_bosses(run['bosses'])}** moved: "
        f"~~{local_day(old_at, tz)} {local_time(old_at, tz)}~~ → "
        f"**{local_day(run['datetime'], tz)} {local_time(run['datetime'], tz)}** — "
        f"{format_participants(run['participants'], who)}\n{REACT_HINT}"
    )


#: Appended to every notice a portal/CLI/API action posts. The party sees the
#: same wording as a chat-driven change, plus who really made it -- a run that
#: moves with nobody in the channel having said anything is otherwise baffling.
VIA_PORTAL = "_(via portal)_"


def via_portal(content: str) -> str:
    return f"{content}\n{VIA_PORTAL}"


def otot_notice(run: dict, tz: ZoneInfo, who: Audience | None = None) -> str:
    """Posted when a run is switched to own time outside Discord."""
    return (
        f"🕒 **{format_bosses(run['bosses'])}** is own-time this week "
        f"({local_day(run['datetime'], tz)}) — it stays in the morning ping, "
        f"but there are no countdowns. {format_participants(run['participants'], who)}"
    )


def restore_notice(run: dict, tz: ZoneInfo, who: Audience | None = None) -> str:
    """Posted when a cancelled or own-time run goes back on the schedule."""
    return (
        f"🔁 **{format_bosses(run['bosses'])}** is back on the schedule "
        f"({local_day(run['datetime'], tz)} {local_time(run['datetime'], tz)}) — "
        f"{format_participants(run['participants'], who)}\n{REACT_HINT}"
    )


def swap_notice(
    run: dict, out: list[str], joined: list[str], tz: ZoneInfo, who: Audience | None = None
) -> str:
    """Posted when a run's party changes for one week only."""
    parts = []
    if out:
        parts.append(f"{format_participants(out, who)} out")
    if joined:
        parts.append(f"{format_participants(joined, who)} in")
    return (
        f"🔁 **{format_bosses(run['bosses'])}** "
        f"({local_day(run['datetime'], tz)} {local_time(run['datetime'], tz)}): "
        f"{' · '.join(parts)} this week — the weekly timing is unchanged."
    )


def roster_delta(out: list[str], joined: list[str]) -> str:
    """``"this week: -MY +kanon"`` for a schedule line, or ``""`` when unchanged."""
    if not out and not joined:
        return ""
    bits = [f"−{name}" for name in out] + [f"+{name}" for name in joined]
    return "this week: " + " ".join(bits)


def done_notice(run: dict, who: Audience | None = None) -> str:
    return (
        f"🏁 **{format_bosses(run['bosses'])}** cleared — "
        f"{format_participants(run['participants'], who)}"
    )


def cancel_notice(run: dict, tz: ZoneInfo, who: Audience | None = None) -> str:
    return (
        f"🚫 **{format_bosses(run['bosses'])}** "
        f"({local_day(run['datetime'], tz)}) is cancelled — "
        f"{format_participants(run['participants'], who)}"
    )


#: How a fixed-timing change reads in the party's channel.
FIXED_VERBS = {
    "added": "📌 Weekly timing added",
    "changed": "📌 Weekly timing changed",
    "removed": "🗑️ Weekly timing removed",
}


def fixed_notice(fixed: dict, verb: str, who: Audience | None = None) -> str:
    """One line for a baseline timing created, edited or deleted elsewhere."""
    day = WEEKDAY_NAMES[fixed["weekday"]]
    return (
        f"{FIXED_VERBS.get(verb, verb)}: **{format_bosses(fixed['bosses'])}** · "
        f"{day} {fixed['time']} · {format_participants(fixed['participants'], who)}"
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


# ---------------------------------------------------------------------------
# extractor proposal cards (DESIGN.md §2.3, §2b.3)
# ---------------------------------------------------------------------------

COLOUR_PROPOSAL = 0xEB459E  # fuchsia
COLOUR_SUGGESTION = 0xFAA61A  # orange -- something is still unanswered
COLOUR_FIXED = 0x9B59B6  # purple -- a recurring timing

CONFIRM_HINT = f"React {EMOJI_YES} to confirm, {EMOJI_NO} to reject, or use `/amend` to edit."

#: Header per card kind: (emoji + title, colour).
PROPOSAL_HEADERS: dict[str, tuple[str, int]] = {
    "proposal": ("📋 Proposed change", COLOUR_PROPOSAL),
    "suggestion": ("💡 Suggested amendment", COLOUR_SUGGESTION),
    "fix": ("📌 New fixed timing", COLOUR_FIXED),
}

#: What each amendment kind reads as on a card.
KIND_VERB: dict[str, str] = {
    "move": "move",
    "add": "new run",
    "cancel": "cancel",
    "split": "split",
    "otot": "own time",
    "sub": "stand-in",
    "fix": "fixed timing",
    "rsvp": "answer",
}

TBD = "**TBD**"


def when_text(amendment: dict, tz: ZoneInfo) -> str:
    """The new day/time, falling back to the words that were actually written.

    Never invents a value: a day with no time reads "Wed 02 Sep — time **TBD**",
    and nothing at all reads **TBD**, which is what DESIGN.md §2b.1 requires.
    """
    new_at = amendment.get("new_datetime")
    if new_at is not None:
        return f"**{local_day(new_at, tz)} {local_time(new_at, tz)}**"
    day_ref, time_ref = amendment.get("day_ref"), amendment.get("time_ref")
    if day_ref and time_ref:
        return f"**{day_ref} {time_ref}** (couldn't read that as a date)"
    if day_ref:
        return f"**{day_ref}** — time {TBD}"
    if time_ref:
        return f"**{time_ref}** — day {TBD}"
    return TBD


def proposal_line(
    amendment: dict, run: dict | None, tz: ZoneInfo, who: Audience | None = None
) -> tuple[str, str]:
    """``(field name, field value)`` for one amendment on a card.

    When a run matched, the heading names **that run's** bosses, not the
    amendment's: the `#id` beside them points at the run, and showing one run's
    id next to another's bosses is how a reader ✅s the wrong night.
    """
    bosses = format_bosses(run["bosses"] if run is not None else amendment["bosses"])
    verb = KIND_VERB.get(amendment["kind"], amendment["kind"])
    name = f"{verb} · {bosses}"
    if run is not None:
        name += f" · `#{short_id(run['id'])}`"

    lines: list[str] = []
    kind = amendment["kind"]
    if kind in ("move", "add", "split", "fix"):
        old = (
            f"~~{local_day(run['datetime'], tz)} {local_time(run['datetime'], tz)}~~ → "
            if run is not None and kind == "move"
            else ""
        )
        lines.append(old + when_text(amendment, tz))
    elif kind == "cancel":
        lines.append("**off this week**")
    elif kind == "otot":
        lines.append("**own time** — stays on the schedule, no countdown pings")
    elif kind == "sub":
        payload = amendment.get("payload") or {}
        out = [str(u) for u in payload.get("remove", [])] or amendment["participants"]
        joining = [str(u) for u in payload.get("add", [])]
        if joining:
            lines.append(
                f"{format_participants(out, who)} out · {format_participants(joining, who)} in"
            )
        else:
            lines.append(f"{format_participants(out, who)} out · **temp needed**")
    else:  # pragma: no cover - rsvp never reaches a card
        lines.append(when_text(amendment, tz))

    people = amendment["participants"] or (run["participants"] if run else [])
    if people and kind != "sub":
        lines.append(format_participants(people, who))
    if amendment.get("summary"):
        lines.append(f"_{amendment['summary']}_")
    also = amendment.get("also_mentioned") or []
    if also:
        # The burst said more than one thing about this run. One ✅ applies the
        # winner; the rest are named so nothing is silently dropped.
        spoken = ", ".join(KIND_VERB.get(kind, kind) for kind in also)
        lines.append(f"(also mentioned: {spoken})")
    return name, "\n".join(lines)


def card_kind(amendments: list[dict]) -> str:
    """Which header this burst's card gets."""
    if amendments and all(a["kind"] == "fix" for a in amendments):
        return "fix"
    unresolved = any(a.get("is_question") or a.get("new_datetime") is None for a in amendments)
    return "suggestion" if unresolved else "proposal"


def proposal_card(
    amendments: list[dict],
    runs: dict[str, dict],
    tz: ZoneInfo,
    unanswered: list[str] | None = None,
    confidence: float | None = None,
    who: Audience | None = None,
) -> Card:
    """One card for a whole burst -- one embed field per amendment.

    ``runs`` maps ``amendment["run_id"]`` to the run it targets.  ``unanswered``
    is the people a field is still waiting on; per DESIGN.md §2b.1 they are named
    rather than assumed, and no value is ever filled in on their behalf.
    """
    kind = card_kind(amendments)
    title, colour = PROPOSAL_HEADERS[kind]

    mentioned = everyone_on(
        [
            {
                "participants": a["participants"]
                or (runs.get(a["run_id"]) or {}).get("participants", [])
            }
            for a in amendments
        ]
    )
    content = f"{title}\n{format_participants(mentioned, who)}" if mentioned else title

    fields = [proposal_line(a, runs.get(a["run_id"]), tz, who) for a in amendments]
    footer = CONFIRM_HINT
    if confidence is not None:
        footer += f"  (confidence {confidence:.2f})"

    description = None
    if unanswered:
        description = "Not yet answered: " + format_participants(unanswered, who)
    return Card(
        content=content,
        description=description,
        fields=fields,
        footer=footer,
        colour=colour,
        mention_users=list(who.mentioned) if who else [],
    )


#: Put on a card that a newer card (or a committed sibling) has retired, so a
#: ✅ on it can never re-apply a change the group has already moved past.
SUPERSEDED_NOTICE = "↪ superseded by a newer card"


def applied_notice(display_name: str) -> str:
    return f"✅ applied by {display_name}"


def rejected_notice(display_name: str) -> str:
    return f"❌ rejected by {display_name}"


# ---------------------------------------------------------------------------
# weekly digest (DESIGN.md §3, posted at reset; the portal can post it on demand)
# ---------------------------------------------------------------------------

COLOUR_DIGEST = 0x5865F2

#: Statuses that mean "nobody has committed to this yet" -- the digest calls
#: these out, because the reset morning is when they can still be settled.
UNSETTLED = ("planned", "at_risk")


def digest_line(run: dict, tz: ZoneInfo, rsvps: dict[str, str]) -> str:
    """One run inside a digest day, without the `<@id>` mentions.

    The digest covers the whole guild, so it names people rather than pinging
    them -- 30 bossers must not all get a notification for every party's run.
    """
    when = "own time" if run["status"] == "otot" else local_time(run["datetime"], tz)
    marker = "⚠️ " if run["status"] in UNSETTLED else ""
    where = f" · <#{run['channel_id']}>" if run.get("channel_id") else ""
    return (
        f"{marker}`{when}` **{format_bosses(run['bosses'])}** · "
        f"{rsvp_tally(run['participants'], rsvps)} · "
        f"`#{short_id(run['id'])}`{where}"
    )


def digest_card(
    runs: list[dict],
    week_start: datetime,
    tz: ZoneInfo,
    rsvps_by_run: dict[str, dict[str, str]],
) -> Card:
    """The whole guild's week, grouped by day, with anything unsettled marked."""
    local_ws = week_start.astimezone(tz)
    live = [r for r in runs if r["status"] != "cancelled"]
    title = f"🗓️ Boss week of {local_ws.strftime('%a %d %b')}"
    if not live:
        return Card(
            content=title,
            description="Nothing on the schedule yet. Add a baseline with `/fixed add`.",
            colour=COLOUR_DIGEST,
        )

    unsettled = sum(1 for r in live if r["status"] in UNSETTLED)
    fields = [
        (heading, "\n".join(digest_line(run, tz, rsvps_by_run.get(run["id"], {})) for run in day))
        for heading, day in group_by_day(live, tz)
    ]
    summary = f"{len(live)} run(s) across {len(fields)} day(s)"
    if unsettled:
        summary += f" · **{unsettled}** still unconfirmed ⚠️"
    return Card(
        content=title,
        description=summary,
        fields=fields,
        footer="React ✅/❌ on your run's reminders · /schedule scope:mine for just yours",
        colour=COLOUR_DIGEST,
    )
