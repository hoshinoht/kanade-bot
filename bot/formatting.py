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
from dataclasses import dataclass, field, replace
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

#: The same six states where there is no room for the words: the portal's
#: compact run cards, seven to a column. Deliberately the emoji each label
#: already opens with rather than a second vocabulary -- a reader who has seen
#: "⚠️ unconfirmed" on a Discord card should not have to learn a new glyph for
#: it on the board. Keep in step with :data:`STATUS_LABEL` above.
STATUS_MARK: dict[str, str] = {
    status: label.split(" ", 1)[0] for status, label in STATUS_LABEL.items()
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


#: The difficulty prefix spelled out, mirroring ``difficulties:`` in
#: ``config/bosses.yaml``. Kept here rather than read from the table because
#: every caller of :func:`boss_label` renders a stored token and has no
#: :class:`bot.bosses.BossTable` to hand; ``tests/test_boss_labels.py`` fails if
#: the two ever drift apart.
DIFFICULTY_WORDS: dict[str, str] = {
    "e": "Easy",
    "n": "Normal",
    "h": "Hard",
    "c": "Chaos",
    "x": "Extreme",
}


def boss_label(token: str) -> str:
    """``"XKalos"`` -> ``"Extreme Kalos"``, for anything a member reads.

    A stored token is a difficulty letter followed by the boss's short name, so
    the words are recoverable from the token alone. Anything that does not look
    like one -- a boss whose short name is a single letter, a token from a table
    that has been re-lettered -- comes back unchanged rather than mangled.
    """
    letter, short = token[:1].lower(), token[1:]
    word = DIFFICULTY_WORDS.get(letter)
    return f"{word} {short}" if word and short else token


def boss_labels(bosses: Iterable[str]) -> str:
    """Every boss on a run, spelled out: ``"Extreme Kalos + Hard Bellona"``."""
    labelled = [boss_label(token) for token in bosses]
    return " + ".join(labelled) if labelled else "(no bosses)"


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


def unanswered(run: dict, rsvps: dict[str, str]) -> list[str]:
    """Participants who have not answered at all - the only people a countdown pings.

    Not "has not said ✅": somebody who reacted ❌ *has* answered, and pinging
    them at T-1h and again at T-15m asks a question they took the trouble to
    answer -- while the run is already `at_risk` and the rest of the party has
    already had the decline notice. A ``maybe`` still counts as unanswered,
    because "maybe" is exactly the person a countdown is for.
    """
    return [uid for uid in run["participants"] if rsvps.get(uid) not in ("yes", "no")]


def declined(run: dict, rsvps: dict[str, str]) -> list[str]:
    """Participants who said ❌."""
    return [uid for uid in run["participants"] if rsvps.get(uid) == "no"]


def not_declined(run: dict, rsvps: dict[str, str]) -> list[str]:
    """Everyone still on the run - the people a countdown may notify.

    A countdown goes to the whole party, minus anybody who has already said
    they can't make it: the run is an hour away and the people on it want the
    reminder whether or not they have ticked yet. Only an explicit ❌ takes
    somebody off the list, and they are still *named* on the card so the party
    can see who is out.
    """
    return [uid for uid in run["participants"] if rsvps.get(uid) != "no"]


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
    """A countdown names the whole party except whoever has already declined.

    Three things it can say, and they are not the same thing: somebody still
    has to answer, everybody has and one of them can't come, or everybody is
    on. Only the last is the green "all set" card -- a run somebody dropped out
    of has nothing left to ask but is not settled either. Whoever is still to
    answer goes in the embed, so a card that lists four people still says which
    two of them the run is waiting on.
    """
    pending = unanswered(run, rsvps)
    out = declined(run, rsvps)
    still_on = not_declined(run, rsvps)
    if not pending and not out:
        waiting = "everyone's confirmed ✅"
    else:
        parts = [format_participants(still_on, who)] if still_on else []
        if out:
            parts.append(f"{format_participants(out, who)} out")
        waiting = " · ".join(parts) if parts else "(nobody)"
    content = (
        f"⏰ **{format_bosses(run['bosses'])}** in {format_offset(minutes)} "
        f"({local_time(run['datetime'], tz)}) — {waiting}"
    )
    detail = [boss_detail(run["bosses"], table), status_line(run, rsvps)]
    if pending:
        detail.append(f"Still to answer: {format_participants(pending, who)}")
    return Card(
        content=content,
        description="\n".join(detail),
        footer=REACT_HINT if pending else None,
        colour=COLOUR_COUNTDOWN if pending or out else COLOUR_ALL_SET,
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


#: Shown on everything posted while quiet mode is on.  The bot still writes the
#: same words, and Discord still renders `<@id>` as a highlighted name, so
#: without this there is nothing on the message to say the party was never
#: actually pinged -- and nothing to remind whoever left it on.
QUIET_MARKER = "🔕"
QUIET_NOTE = f"{QUIET_MARKER} quiet mode - nobody was notified"


def quiet_line(content: str) -> str:
    """Mark a plain message, which has no embed to carry a footer."""
    return f"{content}\n_{QUIET_NOTE}_"


def quieted(card: Card) -> Card:
    """``card``, notifying nobody and saying so.

    The allow-list is emptied here as well as at the send, so a card that has
    been through this cannot ping even if it is handed to something else.
    """
    if card.has_embed:
        footer = f"{card.footer}  ·  {QUIET_NOTE}" if card.footer else QUIET_NOTE
        return replace(card, footer=footer, mention_users=[])
    return replace(card, content=quiet_line(card.content), mention_users=[])


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
    # Deliberately the mirror of the "remove weekly" verb below: the two `fix`
    # cards are opposites, and a reader glancing at a card should not have to
    # work out which one they are looking at.
    "fix": "new weekly",
    "rsvp": "answer",
}

#: The `fix` cards that are not "new weekly", by the ``op`` marker their payload
#: carries (:data:`bot.extract.commit.FIX_REMOVE` and ``FIX_EDIT``). Spelled out
#: rather than imported: this module is what everything else formats through and
#: it knows nothing of the extractor.
FIX_VERB: dict[str, str] = {"remove": "remove weekly", "edit": "change weekly"}

#: How an `rsvp` amendment reads on a proposal card.
RSVP_ANSWER: dict[str | None, str] = {
    "yes": "**can make it**",
    "no": "**can't make it**",
    "maybe": "**not sure yet**",
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


def weekly_text(amendment: dict, tz: ZoneInfo) -> str:
    """How a `fix` that *creates* a baseline reads: the night, and that it recurs.

    A recurring timing rendered as a date -- "**Tue 02 Sep 21:30**", which is all
    a `fix` used to say -- is indistinguishable from the one-night `add` beside
    it, and the two are a fortnight apart in what a ✅ commits the party to. The
    weekday and HH:MM come from the payload, which is what ``fixed_runs``
    actually stores; a row that never got one (a day with no time) falls back to
    the words that were written, still labelled as recurring.
    """
    payload = amendment.get("payload") or {}
    weekday, hhmm = payload.get("weekday"), payload.get("time")
    when = (
        f"**every {WEEKDAY_NAMES[int(weekday)]} {hhmm}**"
        if weekday is not None and hhmm
        else f"{when_text(amendment, tz)} — **and every week after**"
    )
    return (
        f"{when} — recurring from now on, not a one-off; "
        "this week's run is added if that night is still ahead"
    )


def weekly_change_text(amendment: dict, who: Audience | None = None) -> list[str]:
    """How a `fix` that *edits* a baseline reads: what it is now, what it becomes.

    Old → new on both halves, and never one without the other. The failure this
    card exists for is a weekly that got "changed" by putting a second one beside
    it: whoever reads this has to see at a glance that it is the same timing
    moved, not another one added.

    A half that is not changing is simply absent from the payload, so it is shown
    as it stands rather than arrowed to itself.
    """
    payload = amendment.get("payload") or {}
    was = payload.get("weekly_when")
    weekday, hhmm = payload.get("weekday"), payload.get("time")
    lines: list[str] = []
    if weekday is not None and hhmm:
        moved = f"**every {WEEKDAY_NAMES[int(weekday)]} {hhmm}**"
        lines.append(
            (f"~~every {was}~~ → {moved}" if was else moved)
            + " — the weekly timing itself, so every week from now on; "
            "this week's run moves with it"
        )
    elif was:
        lines.append(f"**every {was}** — the night is unchanged")
    party = [str(uid) for uid in payload.get("participants") or []]
    if party:
        lines.append(
            f"{format_participants([str(u) for u in amendment['participants']], who)} → "
            f"{format_participants(party, who)}"
        )
    return lines


def proposal_line(
    amendment: dict, run: dict | None, tz: ZoneInfo, who: Audience | None = None
) -> tuple[str, str]:
    """``(field name, field value)`` for one amendment on a card.

    When a run matched, the heading names **that run's** bosses, not the
    amendment's: the `#id` beside them points at the run, and showing one run's
    id next to another's bosses is how a reader ✅s the wrong night.
    """
    # Spelled out, not the stored token: a card is the last thing anyone reads
    # before committing to a night, and "XKalos" is a name only the regulars can
    # decode. Tokens stay the input vocabulary everywhere else.
    bosses = boss_labels(run["bosses"] if run is not None else amendment["bosses"])
    payload = amendment.get("payload") or {}
    #: A `fix` that removes rather than creates. Given its own verb and its own
    #: line because the two are opposites, and because "remove the fixed run"
    #: and "cancel tonight" are a fortnight apart in consequence: one stops the
    #: guild scheduling this boss at all, the other frees up one evening.
    removes_baseline = amendment["kind"] == "fix" and payload.get("op") == "remove"
    #: The third `fix`: the same baseline, changed rather than created or retired.
    changes_baseline = amendment["kind"] == "fix" and payload.get("op") == "edit"
    verb = (FIX_VERB.get(str(payload.get("op"))) if amendment["kind"] == "fix" else None) or (
        KIND_VERB.get(amendment["kind"], amendment["kind"])
    )
    name = f"{verb} · {bosses}"
    if run is not None:
        name += f" · `#{short_id(run['id'])}`"

    lines: list[str] = []
    kind = amendment["kind"]
    if removes_baseline:
        when = payload.get("weekly_when")
        lines.append(
            f"**stop scheduling this every week**{f' ({when})' if when else ''} — "
            "future weeks will not be scheduled, and this week's run is cancelled"
        )
    elif changes_baseline:
        lines.extend(weekly_change_text(amendment, who))
    elif kind == "fix":
        lines.append(weekly_text(amendment, tz))
    elif kind in ("move", "add", "split"):
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
            # Nobody offered to cover, so the proposal is the removal itself --
            # the same "-1 for this week" a `/swap out:` does. Asking the channel
            # to find a temp is a job nothing here does, and a card that says
            # "temp needed" is a to-do item rather than something to ✅.
            lines.append(f"{format_participants(out, who)} **out this week**")
    elif kind == "rsvp":
        # The extractor applies a chat answer straight away and never cards one.
        # The chatbot does card it: it is answering on somebody's behalf from a
        # sentence it was told, so the person gets to see it before it counts.
        lines.append(RSVP_ANSWER.get(amendment.get("rsvp"), "**answered**"))
    else:  # pragma: no cover - every kind above is handled
        lines.append(when_text(amendment, tz))

    people = amendment["participants"] or (run["participants"] if run else [])
    # A change card that alters the party has already said so as old → new;
    # repeating the old list under it would read as the party it is becoming.
    party_shown = changes_baseline and bool(payload.get("participants"))
    if people and kind != "sub" and not party_shown:
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
#: these out, because the reset morning is when they can still be settled -- and
#: the marker each one gets. They are not the same problem: `planned` is waiting
#: on answers, while `at_risk` already *has* one and it was a no, so the party
#: has a night to re-plan rather than a form to fill in. One shared ⚠️ hid that
#: difference in the one post meant to surface it.
UNSETTLED_MARKERS: dict[str, str] = {"planned": "⚠️ ", "at_risk": "❗ "}
UNSETTLED = tuple(UNSETTLED_MARKERS)


def digest_line(run: dict, tz: ZoneInfo, rsvps: dict[str, str]) -> str:
    """One run inside a digest day, without the `<@id>` mentions.

    The digest covers the whole guild, so it names people rather than pinging
    them -- 30 bossers must not all get a notification for every party's run.
    """
    when = "own time" if run["status"] == "otot" else local_time(run["datetime"], tz)
    marker = UNSETTLED_MARKERS.get(run["status"], "")
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
    at_risk = sum(1 for r in live if r["status"] == "at_risk")
    fields = [
        (heading, "\n".join(digest_line(run, tz, rsvps_by_run.get(run["id"], {})) for run in day))
        for heading, day in group_by_day(live, tz)
    ]
    summary = f"{len(live)} run(s) across {len(fields)} day(s)"
    if unsettled:
        summary += f" · **{unsettled}** still unconfirmed ⚠️"
    if at_risk:
        # Counted in the line above as well: they are unconfirmed *and* somebody
        # has said no, which is the half that needs a decision this morning.
        summary += f" · **{at_risk}** at risk ❗"
    return Card(
        content=title,
        description=summary,
        fields=fields,
        footer="React ✅/❌ on your run's reminders · /schedule scope:mine for just yours",
        colour=COLOUR_DIGEST,
    )
