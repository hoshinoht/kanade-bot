"""Everything the API does, in terms of the repository and the live bot.

The slash commands and this module must not drift apart, so the mutating
functions here call exactly the repository methods :mod:`bot.commands` calls --
``set_run_datetime`` + ``refresh_run_reminders`` for a move, ``set_run_status``
for a cancel, ``commit()`` for an approval -- and reuse the same validation
(``BossTable.parse``, the bossing-role check, "the channel must be watched").
What is new here is only the *shape*: ids arrive as strings from HTTP rather
than from an autocomplete, so every lookup accepts a short prefix, and the
return values are plain JSON-able dicts that both ``routes_api`` and the Jinja
templates render.

Nothing in here imports FastAPI. Functions that need Discord (posting a card,
paging channel history) are ``async`` and take the live :class:`~bot.client.BossBot`.
"""

from __future__ import annotations

import logging
import re
import zlib
from collections.abc import AsyncIterator, Callable, Iterable, Sequence
from datetime import datetime, time, timedelta
from typing import TYPE_CHECKING, Any

import dateparser

from .. import formatting
from ..bosses import BossParseError
from ..export import message_record
from ..extract.commit import commit, reject
from ..extract.window import DEFAULT_WINDOW, WINDOWS
from ..ids import IdAmbiguous, IdError, resolve_id, short_id
from ..materialise import DAY_OF, LIVE_STATUSES, countdown_minutes, refresh_run_reminders
from ..pings import audience, normalise_level
from ..rsvp import compute_status, recompute_after_roster_change
from ..timeutil import local_naive, to_iso, utcnow
from ..weeks import (
    WEEKDAY_NAMES,
    current_week_start,
    next_week_start,
    parse_hhmm,
    parse_weekday,
    slot_in_week,
    week_start,
)
from .errors import BadRequest, NotFound

if TYPE_CHECKING:  # pragma: no cover
    from ..client import BossBot

log = logging.getLogger(__name__)

#: Put on a Discord card that the portal (rather than a ✅ reaction) applied.
PORTAL_APPLIED = "✅ applied via portal"
PORTAL_REJECTED = "❌ rejected via portal"

#: Runtime config the portal may read and write, and how to validate each one.
#: Anything not listed here is refused -- `config set` is not a way to write
#: arbitrary rows into the `config` table.
CONFIG_KEYS = ("day_of_ping_time", "countdown_minutes", "paused", "extract_enabled", "quiet_mode")


# ---------------------------------------------------------------------------
# weeks and ids
# ---------------------------------------------------------------------------


def week_for(bot: BossBot, which: str = "this") -> datetime:
    """``"this"`` / ``"next"`` -> that boss week's start instant."""
    which = (which or "this").lower()
    if which not in ("this", "next"):
        raise BadRequest("week must be `this` or `next`")
    now = utcnow()
    if which == "next":
        return next_week_start(bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, now)
    return current_week_start(bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, now)


def _resolve(raw: str, candidates: Iterable[str], noun: str) -> str:
    """Full uuid or any unique prefix -> one id, or a 404/400 a human can act on."""
    try:
        return resolve_id(str(raw), candidates)
    except IdAmbiguous as exc:
        listed = ", ".join(short_id(c) for c in exc.candidates[:8])
        raise BadRequest(f"`{raw}` matches several {noun}s: {listed}") from None
    except IdError as exc:
        raise NotFound(f"{exc}") from None


def load_run(bot: BossBot, run_id: str) -> dict:
    run = bot.repo.get_run(_resolve(run_id, [r["id"] for r in bot.repo.list_runs()], "run"))
    if run is None:  # pragma: no cover - _resolve guarantees it exists
        raise NotFound(f"no run `{run_id}`")
    return run


def load_fixed(bot: BossBot, fixed_id: str) -> dict:
    resolved = _resolve(fixed_id, [f["id"] for f in bot.repo.list_fixed_runs()], "fixed run")
    fixed = bot.repo.get_fixed_run(resolved)
    if fixed is None:  # pragma: no cover
        raise NotFound(f"no fixed run `{fixed_id}`")
    return fixed


def load_amendment(bot: BossBot, amendment_id: str) -> dict:
    resolved = _resolve(
        amendment_id, [a["id"] for a in bot.repo.list_amendments()], "proposed change"
    )
    amendment = bot.repo.get_amendment(resolved)
    if amendment is None:  # pragma: no cover
        raise NotFound(f"no proposed change `{amendment_id}`")
    return amendment


def load_extraction(bot: BossBot, extraction_id: str) -> dict:
    resolved = _resolve(extraction_id, bot.repo.list_extraction_ids(), "extraction")
    extraction = bot.repo.get_extraction(resolved)
    if extraction is None:  # pragma: no cover
        raise NotFound(f"no extraction `{extraction_id}`")
    return extraction


# ---------------------------------------------------------------------------
# naming things
# ---------------------------------------------------------------------------


def member_name(bot: BossBot, user_id: int | str) -> str:
    member = bot.repo.get_member(user_id)
    if member:
        return member["nickname"] or member["display_name"] or str(user_id)
    return f"user {short_id(str(user_id)) or user_id}"


#: Discord's raw mention markup, as it is stored on a message.
_MENTION_RE = re.compile(r"<@!?(\d+)>")


def render_mentions(bot: BossBot, text: str | None) -> str | None:
    """``<@100000000000000002>`` as ``@kanon``, for text shown in the portal.

    Discord renders mentions itself; a page that shows the same string raw
    prints a snowflake at the reader, which is unreadable exactly where it
    matters most -- the chat line that justifies a change. The stored message
    keeps the markup (it is what the extractor read, and what a re-post needs),
    so this resolves only on the way out. An id nobody in the roster matches
    reads ``@member`` rather than leaking the number.
    """
    if not text:
        return text

    def name_for(match: re.Match[str]) -> str:
        member = bot.repo.get_member(match.group(1))
        name = (member["nickname"] or member["display_name"]) if member else None
        return f"@{name}" if name else "@member"

    return _MENTION_RE.sub(name_for, text)


def channel_name(bot: BossBot, channel_id: int | str | None) -> str | None:
    """``#party-channel`` when the bot can see it, else ``None``.

    The portal is often opened while the bot is connected, but the tests and a
    disconnected container are not, so this never assumes a live gateway.
    """
    if channel_id is None:
        return None
    try:
        channel = bot.get_channel(int(channel_id))
    except (TypeError, ValueError):  # pragma: no cover - defensive
        return None
    name = getattr(channel, "name", None)
    return f"#{name}" if name else None


def message_url(bot: BossBot, channel_id: Any, message_id: Any) -> str | None:
    """The ``https://discord.com/channels/...`` deep link for a posted message."""
    if not channel_id or not message_id:
        return None
    return f"https://discord.com/channels/{bot.settings.guild_id}/{channel_id}/{message_id}"


def card_label(kind: str) -> str:
    """What a reminder kind is called on a run row.

    The row is read by a person, not by the scheduler, so a card is named for
    when it lands: the morning card, then the countdowns as time-to-go.
    ``format_offset`` is the same function that writes the countdown itself, so
    the portal and the Discord message cannot end up disagreeing about "1h".
    """
    if kind == DAY_OF:
        return "morning"
    minutes = countdown_minutes(kind)
    return f"T-{formatting.format_offset(minutes)}" if minutes is not None else kind


def run_cards(bot: BossBot, run: dict) -> list[dict]:
    """Every message this run has produced or still owes, oldest first.

    Three states, and they are three different facts: **posted** (with a deep
    link into Discord), **queued** (nothing has been said yet), and **skipped**
    -- a reminder retired without a message, which is what a sleeping host or a
    cancelled run leaves behind. Collapsing the last two into "not posted" would
    hide the one case worth investigating.
    """
    cards: list[dict] = []
    for reminder in bot.repo.list_reminders(run["id"]):
        sent_at = reminder["sent_at"]
        url = message_url(bot, run["channel_id"], reminder["message_id"])
        cards.append(
            {
                "kind": reminder["kind"],
                "label": card_label(reminder["kind"]),
                "fire_at": to_iso(reminder["fire_at"]),
                "local_fire_at": formatting.local_time(reminder["fire_at"], bot.tz),
                "sent_at": to_iso(sent_at) if sent_at else None,
                "local_sent_at": formatting.local_time(sent_at, bot.tz) if sent_at else None,
                "message_id": reminder["message_id"],
                "url": url,
                "state": "posted" if url else ("skipped" if sent_at else "queued"),
            }
        )
    return cards


def channel_is_watched(bot: BossBot, channel_id: int | str) -> bool:
    """Would the bot listen to (and post in) this channel?

    Resolved against the live channel when there is one, because a channel is
    usually watched through its *category*.  With no gateway the explicit
    ``CHAT_CHANNEL_IDS`` list is all that can be checked.
    """
    try:
        cid = int(channel_id)
    except (TypeError, ValueError):
        return False
    channel = bot.get_channel(cid)
    if channel is not None:
        return bot.is_watched(channel)
    return cid in bot.settings.chat_channel_id_list


# ---------------------------------------------------------------------------
# views: the JSON shapes both `routes_api` and the templates render
# ---------------------------------------------------------------------------


def monogram(name: str) -> dict:
    """A stand-in badge for a boss with no portrait.

    Deterministic from the name, so the same boss is always the same colour and
    the grid does not reshuffle between page loads. Sits in the same box a
    portrait would, so nothing shifts when one is added.
    """
    letters = [word[0] for word in re.split(r"[^A-Za-z0-9]+", name) if word][:2]
    return {
        "text": "".join(letters).upper() or "?",
        "hue": zlib.crc32(name.encode("utf-8")) % 360,
    }


def boss_view(bot: BossBot, token: str) -> dict:
    """One boss as the portal renders it: full name, difficulty pill, level.

    Falls back to the raw token for anything not in ``bosses.yaml`` -- an old
    run whose boss was removed from the table still has to render.
    """
    detail = bot.bosses.detail(token)
    if detail is None:
        return {
            "token": token,
            "short": token,
            "full": token,
            "level": None,
            "letter": "",
            "difficulty": "",
            "label": token,
            "portrait": None,
            "monogram": monogram(token),
        }
    return {
        **detail,
        "letter": detail["letter"].upper(),
        "difficulty": detail["difficulty"].upper(),
        "label": bot.bosses.describe(token),
        "portrait": portrait_url(bot, detail["short"]),
        "monogram": monogram(detail["full"]),
    }


def portrait_url(bot: BossBot, short: str) -> str | None:
    """The portal URL for a boss portrait, or ``None`` when there is no file."""
    return f"/static/portraits/{short}" if bot.bosses.portrait_path(short) else None


def boss_grid(bot: BossBot, selected: Sequence[str] = ()) -> list[dict]:
    """The in-game boss list: one row per boss, its difficulties as pills.

    ``selected`` marks the tokens that are already chosen, so the same grid
    serves the fixed-run editor (a picker) and `/bosses` (a read-only view of
    what the guild actually runs).
    """
    chosen = {str(t) for t in selected}
    rows = []
    for boss in bot.bosses.ordered():
        options = [
            {
                "token": boss.canonical(letter),
                "letter": letter.upper(),
                "difficulty": bot.bosses.difficulty_name(letter).upper(),
                "selected": boss.canonical(letter) in chosen,
            }
            for letter in boss.difficulties
        ]
        rows.append(
            {
                "short": boss.short,
                "full": boss.full,
                "level": boss.level,
                "difficulties": options,
                "any_selected": any(o["selected"] for o in options),
                "portrait": portrait_url(bot, boss.short),
                "monogram": monogram(boss.full),
            }
        )
    return rows


def bosses_in_use(bot: BossBot) -> list[str]:
    """Every canonical boss the guild has a fixed timing for."""
    seen: list[str] = []
    for fixed in bot.repo.list_fixed_runs():
        for token in fixed["bosses"]:
            if token not in seen:
                seen.append(token)
    return seen


def run_view(bot: BossBot, run: dict, rsvps: dict[str, str] | None = None) -> dict:
    rsvps = bot.repo.get_rsvps(run["id"]) if rsvps is None else rsvps
    local = run["datetime"].astimezone(bot.tz)
    participants = [
        {"id": uid, "name": member_name(bot, uid), "rsvp": rsvps.get(uid)}
        for uid in run["participants"]
    ]
    return {
        "id": run["id"],
        "short_id": short_id(run["id"]),
        "bosses": run["bosses"],
        "boss_detail": [boss_view(bot, b) for b in run["bosses"]],
        "datetime": to_iso(run["datetime"]),
        "local_date": local.strftime("%Y-%m-%d"),
        "local_day": formatting.local_day(run["datetime"], bot.tz),
        "local_time": formatting.local_time(run["datetime"], bot.tz),
        "weekday": local.weekday(),
        "status": run["status"],
        "status_label": formatting.STATUS_LABEL.get(run["status"], run["status"]),
        "source": run["source"],
        "fixed_run_id": run["fixed_run_id"],
        "channel_id": run["channel_id"],
        "channel_name": channel_name(bot, run["channel_id"]),
        "week_start": to_iso(run["week_start"]),
        "participants": participants,
        "yes": sum(1 for p in participants if p["rsvp"] == "yes"),
        "no": sum(1 for p in participants if p["rsvp"] == "no"),
        "maybe": sum(1 for p in participants if p["rsvp"] == "maybe"),
        "unanswered": sum(1 for p in participants if p["rsvp"] is None),
        "cards": run_cards(bot, run),
        "roster_change": roster_change(bot, run),
    }


def fixed_view(bot: BossBot, fixed: dict, with_grid: bool = False) -> dict:
    view = {
        "id": fixed["id"],
        "short_id": short_id(fixed["id"]),
        "owner_id": fixed["owner_id"],
        "owner_name": member_name(bot, fixed["owner_id"]),
        "channel_id": fixed["channel_id"],
        "channel_name": channel_name(bot, fixed["channel_id"]),
        "channel_watched": channel_is_watched(bot, fixed["channel_id"])
        if fixed["channel_id"]
        else False,
        "bosses": fixed["bosses"],
        "boss_detail": [boss_view(bot, b) for b in fixed["bosses"]],
        "weekday": fixed["weekday"],
        "weekday_name": WEEKDAY_NAMES[fixed["weekday"]],
        "time": fixed["time"],
        "participants": [
            {"id": uid, "name": member_name(bot, uid)} for uid in fixed["participants"]
        ],
        "note": fixed["note"],
    }
    if with_grid:
        # The editor renders the same picker as the create form, pre-ticked.
        view["grid"] = boss_grid(bot, fixed["bosses"])
    return view


def evidence_view(bot: BossBot, message_ids: Sequence[str]) -> list[dict]:
    """The chat lines an extracted change cites, in the order they were said."""
    rows = []
    for mid in message_ids:
        row = bot.repo.get_message(mid)
        if row is None:
            rows.append({"id": str(mid), "missing": True})
            continue
        rows.append(
            {
                "id": str(row["id"]),
                "missing": False,
                "author_id": row["author_id"],
                "author_name": member_name(bot, row["author_id"]),
                "created_at": to_iso(row["created_at"]),
                "local_time": row["created_at"].astimezone(bot.tz).strftime("%a %d %b %H:%M"),
                "content": render_mentions(bot, row["content"]),
                "url": message_url(bot, row["channel_id"], row["id"]),
            }
        )
    rows.sort(key=lambda r: (r.get("created_at") or "", r["id"]))
    return rows


def amendment_view(bot: BossBot, amendment: dict, with_evidence: bool = True) -> dict:
    run = bot.repo.get_run(amendment["run_id"]) if amendment["run_id"] else None
    view = {
        "id": amendment["id"],
        "short_id": short_id(amendment["id"]),
        "kind": amendment["kind"],
        "kind_label": formatting.KIND_VERB.get(amendment["kind"], amendment["kind"]),
        "status": amendment["status"],
        "bosses": amendment["bosses"],
        "boss_detail": [boss_view(bot, b) for b in amendment["bosses"]],
        "run_id": amendment["run_id"],
        "run": run_view(bot, run) if run else None,
        "new_datetime": to_iso(amendment["new_datetime"]) if amendment["new_datetime"] else None,
        "when": formatting.when_text(amendment, bot.tz).replace("**", ""),
        "day_ref": amendment["day_ref"],
        "time_ref": amendment["time_ref"],
        # Only a move has an "old → new": a new run or a correction has no
        # earlier time to arrow away from, even when it matched an existing run
        # (which is how `new run` cards came to read "Sat 21:30 → TBD"). The
        # Discord card already reads it this way; see formatting.proposal_line.
        "from_when": (
            f"{formatting.local_day(run['datetime'], bot.tz)} "
            f"{formatting.local_time(run['datetime'], bot.tz)}"
            if run is not None and amendment["kind"] == "move"
            else None
        ),
        "confidence": amendment["confidence"],
        "is_question": amendment["is_question"],
        "summary": render_mentions(bot, amendment["summary"]),
        "rsvp": amendment["rsvp"],
        "payload": amendment["payload"],
        "channel_id": amendment["channel_id"],
        "channel_name": channel_name(bot, amendment["channel_id"]),
        "created_at": to_iso(amendment["created_at"]),
        "week_start": to_iso(amendment["week_start"]),
        "participants": [
            {"id": uid, "name": member_name(bot, uid)} for uid in amendment["participants"]
        ],
        "card_url": message_url(bot, amendment["channel_id"], amendment["proposal_message_id"]),
    }
    if with_evidence:
        view["evidence"] = evidence_view(bot, amendment["evidence_msg_ids"])
    return view


def extraction_view(bot: BossBot, extraction: dict, detail: bool = False) -> dict:
    view = {
        "id": extraction["id"],
        "short_id": short_id(extraction["id"]),
        "at": to_iso(extraction["at"]),
        "local_time": extraction["at"].astimezone(bot.tz).strftime("%a %d %b %H:%M:%S"),
        "model": extraction["model"],
        "latency_ms": extraction["latency_ms"],
        "message_ids": extraction["message_ids"],
        "amendment_count": len(extraction["amendment_ids"]),
    }
    if detail:
        view["prompt"] = extraction["prompt"]
        view["raw_response"] = extraction["raw_response"]
        view["messages"] = evidence_view(bot, extraction["message_ids"])
        view["amendments"] = [
            amendment_view(bot, a, with_evidence=False)
            for a in (bot.repo.get_amendment(i) for i in extraction["amendment_ids"])
            if a is not None
        ]
    return view


def member_view(bot: BossBot, member: dict, run_counts: dict[str, int]) -> dict:
    return {
        "user_id": member["user_id"],
        "display_name": member["display_name"],
        "nickname": member["nickname"],
        "aliases": member["aliases"],
        "has_role": member["has_role"],
        "ping_level": member["ping_level"],
        "updated_at": member["updated_at"],
        "runs_this_week": run_counts.get(member["user_id"], 0),
    }


def reminder_view(bot: BossBot, reminder: dict, run: dict | None) -> dict:
    return {
        "id": reminder["id"],
        "short_id": short_id(reminder["id"]),
        "run_id": reminder["run_id"],
        "run_short_id": short_id(reminder["run_id"]),
        "kind": reminder["kind"],
        "fire_at": to_iso(reminder["fire_at"]),
        "local_fire_at": reminder["fire_at"].astimezone(bot.tz).strftime("%a %d %b %H:%M"),
        "sent_at": to_iso(reminder["sent_at"]) if reminder["sent_at"] else None,
        "message_id": reminder["message_id"],
        "url": message_url(bot, run["channel_id"] if run else None, reminder["message_id"]),
        "bosses": run["bosses"] if run else [],
        "boss_detail": [boss_view(bot, b) for b in run["bosses"]] if run else [],
        "run_local": formatting.local_day(run["datetime"], bot.tz) if run else None,
        "status": run["status"] if run else None,
    }


# ---------------------------------------------------------------------------
# schedule
# ---------------------------------------------------------------------------


def schedule(
    bot: BossBot,
    week: str = "this",
    channel_id: int | str | None = None,
    user_id: int | str | None = None,
    boss: str | None = None,
    show_past: bool = False,
) -> dict:
    """One week's runs, filtered, grouped by day -- the Week view's whole payload.

    A boss week is materialised whole, so by Sunday it already holds Thursday's
    finished runs. ``done`` and ``cancelled`` are hidden unless ``show_past``,
    and the count of what was hidden is returned so the page can say so rather
    than quietly dropping rows.
    """
    ws = week_for(bot, week)
    # "mine" counts runs whose fixed timing the member owns, not just ones they
    # are on -- the same rule `/schedule scope:mine` applies.
    everything = bot.repo.list_runs(week_start=ws, involving=user_id, channel_id=channel_id)
    if boss:
        wanted = boss.strip().lower()
        everything = [r for r in everything if any(wanted in b.lower() for b in r["bosses"])]
    runs = everything if show_past else [r for r in everything if r["status"] in LIVE_STATUSES]
    views = [run_view(bot, run) for run in runs]
    return {
        "week": week,
        "week_start": to_iso(ws),
        "week_label": ws.astimezone(bot.tz).strftime("%a %d %b"),
        "timezone": bot.settings.tz,
        "show_past": show_past,
        "hidden": len(everything) - len(runs),
        "days": [
            {"heading": heading, "runs": [run_view(bot, r) for r in day]}
            for heading, day in formatting.group_by_day(runs, bot.tz)
        ],
        "runs": views,
        "count": len(views),
    }


def week_rail(bot: BossBot, week: str = "this") -> list[dict]:
    """The seven days of a boss week, in the week's own order (reset day first).

    This is what the portal's rail renders: the shape of the week, whose first
    column is Thursday here rather than Monday, plus a pip per run so a glance
    says which nights are busy.
    """
    ws = week_for(bot, week)
    local_ws = ws.astimezone(bot.tz)
    today = utcnow().astimezone(bot.tz).date()
    runs = bot.repo.list_runs(week_start=ws)
    by_date: dict[str, list[dict]] = {}
    for run in runs:
        key = run["datetime"].astimezone(bot.tz).strftime("%Y-%m-%d")
        by_date.setdefault(key, []).append(run)

    days = []
    for offset in range(7):
        date = (local_ws.replace(tzinfo=None) + timedelta(days=offset)).date()
        key = date.strftime("%Y-%m-%d")
        day_runs = sorted(by_date.get(key, []), key=lambda r: r["datetime"])
        days.append(
            {
                "date": key,
                "weekday": WEEKDAY_NAMES[date.weekday()],
                "day": date.day,
                "month": date.strftime("%b"),
                "is_today": date == today,
                "runs": [
                    {
                        "id": r["id"],
                        "status": r["status"],
                        "time": formatting.local_time(r["datetime"], bot.tz),
                        "bosses": formatting.format_bosses(r["bosses"]),
                    }
                    for r in day_runs
                ],
            }
        )
    return days


# ---------------------------------------------------------------------------
# fixed runs
# ---------------------------------------------------------------------------


def validate_bosses(bot: BossBot, text: str) -> list[str]:
    try:
        return bot.bosses.parse(text)
    except BossParseError as exc:
        raise BadRequest(str(exc)) from None


def validate_participants(bot: BossBot, ids: Sequence[int | str]) -> list[str]:
    """The same gate ``/fixed add`` applies: rostered humans only, no duplicates."""
    out: list[str] = []
    for raw in ids:
        uid = str(raw).strip()
        if not uid:
            continue
        if not uid.isdigit():
            raise BadRequest(f"`{uid}` is not a Discord user id")
        if uid not in out:
            out.append(uid)
    if not out:
        raise BadRequest("a run needs at least one participant")
    outsiders = [uid for uid in out if not bot.repo.has_role(uid)]
    if outsiders:
        raise BadRequest(
            "not in the bossing role: "
            + ", ".join(f"{member_name(bot, u)} ({u})" for u in outsiders)
        )
    bots = [uid for uid in out if getattr(bot.get_user(int(uid)), "bot", False)]
    if bots:
        raise BadRequest("bots can't be participants: " + ", ".join(bots))
    return out


def validate_channel(bot: BossBot, channel_id: int | str) -> str:
    if not channel_is_watched(bot, channel_id):
        raise BadRequest(
            f"channel {channel_id} isn't watched, so its runs would never get their pings - "
            "add it to CHAT_CHANNEL_IDS, or its category to CHAT_CATEGORY_IDS"
        )
    return str(channel_id)


async def create_fixed(
    bot: BossBot,
    *,
    bosses: str | Sequence[str],
    day: str | int,
    time_hhmm: str,
    participants: Sequence[int | str],
    channel_id: int | str,
    owner_id: int | str | None = None,
    note: str | None = None,
) -> dict:
    boss_list = validate_bosses(bot, bosses if isinstance(bosses, str) else ",".join(bosses))
    try:
        weekday = parse_weekday(day)
        hhmm = parse_hhmm(str(time_hhmm)).strftime("%H:%M")
    except ValueError as exc:
        raise BadRequest(str(exc)) from None
    people = validate_participants(bot, participants)
    home = validate_channel(bot, channel_id)
    fixed_id = bot.repo.add_fixed_run(
        owner_id=str(owner_id or bot.portal_actor_id),
        bosses=boss_list,
        weekday=weekday,
        time_hhmm=hhmm,
        participants=people,
        note=note,
        channel_id=home,
    )
    bot.materialise_weeks()
    fixed = bot.repo.get_fixed_run(fixed_id)
    who = audience(bot.repo, fixed["participants"], "fixed")
    await _announce(bot, formatting.fixed_notice(fixed, "added", who), who.mentioned, home)
    return fixed_view(bot, fixed)


async def update_fixed(bot: BossBot, fixed_id: str, **changes: Any) -> dict:
    """Apply a partial edit, then push only the touched fields onto live runs.

    Re-snapping every field would undo this week's ``/amend`` -- editing a note
    would drag a run that was moved Mon -> Wed back to Monday -- so this mirrors
    ``bot.commands._apply_fixed_to_runs`` exactly.
    """
    fixed = load_fixed(bot, fixed_id)
    fields: dict[str, Any] = {}
    if changes.get("bosses") is not None:
        raw = changes["bosses"]
        fields["bosses"] = validate_bosses(bot, raw if isinstance(raw, str) else ",".join(raw))
    if changes.get("day") is not None:
        try:
            fields["weekday"] = parse_weekday(changes["day"])
        except ValueError as exc:
            raise BadRequest(str(exc)) from None
    if changes.get("time") is not None:
        try:
            fields["time"] = parse_hhmm(str(changes["time"])).strftime("%H:%M")
        except ValueError as exc:
            raise BadRequest(str(exc)) from None
    if changes.get("participants") is not None:
        fields["participants"] = validate_participants(bot, changes["participants"])
    if changes.get("channel_id") is not None:
        fields["channel_id"] = validate_channel(bot, changes["channel_id"])
    if changes.get("note") is not None:
        fields["note"] = changes["note"]
    if not fields:
        raise BadRequest("nothing to change")

    bot.repo.update_fixed_run(fixed["id"], **fields)
    _apply_fixed_to_runs(bot, fixed["id"], set(fields))
    bot.materialise_weeks()
    updated = bot.repo.get_fixed_run(fixed["id"])
    who = audience(bot.repo, updated["participants"], "fixed")
    await _announce(
        bot,
        formatting.fixed_notice(updated, "changed", who),
        who.mentioned,
        updated["channel_id"],
    )
    return fixed_view(bot, updated)


def _apply_fixed_to_runs(bot: BossBot, fixed_id: str, changed: set[str]) -> None:
    fixed = bot.repo.get_fixed_run(fixed_id)
    if fixed is None or not changed:
        return
    reschedule = bool(changed & {"weekday", "time"})
    for which in ("this", "next"):
        ws = week_for(bot, which)
        run = bot.repo.run_for_fixed(fixed_id, ws)
        if run is None or run["status"] in ("done", "cancelled"):
            continue
        if "bosses" in changed:
            bot.repo.set_run_bosses(run["id"], fixed["bosses"])
        if "participants" in changed:
            bot.repo.set_run_participants(run["id"], fixed["participants"])
        if "channel_id" in changed:
            bot.repo.set_run_channel(run["id"], fixed["channel_id"])
        if reschedule:
            hour, minute = (int(p) for p in fixed["time"].split(":"))
            run_at = slot_in_week(ws, bot.tz, fixed["weekday"], time(hour, minute))
            bot.repo.set_run_datetime(run["id"], run_at, ws)
            refresh_run_reminders(bot.repo, run["id"], bot.tz, bot.ping_time, bot.countdowns)


async def delete_fixed(bot: BossBot, fixed_id: str) -> dict:
    """Remove a baseline timing and cancel the runs it had already produced."""
    fixed = load_fixed(bot, fixed_id)
    cancelled = 0
    for which in ("this", "next"):
        run = bot.repo.run_for_fixed(fixed["id"], week_for(bot, which))
        if run is not None and run["status"] not in ("done", "cancelled"):
            bot.repo.set_run_status(run["id"], "cancelled")
            refresh_run_reminders(bot.repo, run["id"], bot.tz, bot.ping_time, bot.countdowns)
            cancelled += 1
    bot.repo.delete_fixed_run(fixed["id"])
    who = audience(bot.repo, fixed["participants"], "fixed")
    await _announce(
        bot, formatting.fixed_notice(fixed, "removed", who), who.mentioned, fixed["channel_id"]
    )
    return {"id": fixed["id"], "short_id": short_id(fixed["id"]), "cancelled_runs": cancelled}


# ---------------------------------------------------------------------------
# run mutations
# ---------------------------------------------------------------------------


def parse_when(bot: BossBot, text: str) -> datetime:
    """``"wed 21:30"`` / ``"tomorrow 9:45pm"`` -> an instant, exactly as ``/amend``."""
    parsed = dateparser.parse(
        text,
        settings={
            "RELATIVE_BASE": local_naive(utcnow(), bot.tz),
            "PREFER_DATES_FROM": "future",
            "TIMEZONE": bot.settings.tz,
            "RETURN_AS_TIMEZONE_AWARE": True,
        },
    )
    if parsed is None:
        raise BadRequest(
            f"couldn't read `{text}` as a date - try `wed 21:30` or `2026-09-02 21:30`"
        )
    return parsed


async def amend_run(bot: BossBot, run_id: str, to: str) -> dict:
    run = load_run(bot, run_id)
    parsed = parse_when(bot, to)
    old_at = run["datetime"]
    ws = week_start(parsed, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time)
    bot.repo.set_run_datetime(run["id"], parsed, ws)
    if run["status"] in ("confirmed", "at_risk"):
        # Moving a run invalidates the answers people gave about the old slot.
        bot.repo.set_run_status(run["id"], "planned")
    refresh_run_reminders(bot.repo, run["id"], bot.tz, bot.ping_time, bot.countdowns)
    updated = bot.repo.get_run(run["id"])
    who = audience(bot.repo, updated["participants"], "amend")
    await _announce(
        bot,
        formatting.amend_notice(updated, old_at, bot.tz, who),
        who.mentioned,
        updated["channel_id"],
    )
    return run_view(bot, updated)


async def cancel_run(bot: BossBot, run_id: str) -> dict:
    return await set_status(bot, run_id, "cancelled")


#: The statuses a person may set, in the order the portal's control shows them.
#: `at_risk` is *derived* -- it means somebody said no, and setting it by hand
#: would be a claim about an answer nobody gave.
SETTABLE_STATUSES: tuple[str, ...] = ("planned", "confirmed", "otot", "done", "cancelled")

#: How each target status reads in the party's channel, and what it does to the
#: run's reminders. `refresh_run_reminders` derives the right set from the
#: status itself (`bot.materialise.reminder_specs`), so "rebuild" covers
#: planned/confirmed keeping both kinds, `otot` keeping only the morning ping,
#: and `cancelled`/`done` keeping none.
STATUS_LABELS: dict[str, str] = {
    "planned": "back on the schedule",
    "confirmed": "confirmed",
    "otot": "own time",
    "done": "cleared",
    "cancelled": "cancelled",
}


def status_notice(
    bot: BossBot, run: dict, status: str, who: formatting.Audience | None = None
) -> str | None:
    """What the channel is told about a status change, or ``None`` to stay quiet."""
    if status == "cancelled":
        return formatting.cancel_notice(run, bot.tz, who)
    if status == "otot":
        return formatting.otot_notice(run, bot.tz, who)
    if status == "done":
        return formatting.done_notice(run, who)
    if status == "planned":
        return formatting.restore_notice(run, bot.tz, who)
    if status == "confirmed":
        return (
            f"✅ **{formatting.format_bosses(run['bosses'])}** is confirmed for "
            f"{formatting.local_day(run['datetime'], bot.tz)} "
            f"{formatting.local_time(run['datetime'], bot.tz)} — "
            f"{formatting.format_participants(run['participants'], who)}"
        )
    return None


async def set_status(
    bot: BossBot, run_id: str, status: str, announce: bool = True, mark: bool = True
) -> dict:
    """Move a run to an explicitly chosen status, with the side effects it implies.

    One function behind `/status`, `/otot`, `/cancel`, `/restore`, `/done`, the
    portal's segmented control and `bossctl status`, so a transition cannot mean
    one thing in Discord and another in the portal.

    * reminders are rebuilt from the new status -- countdowns come back for
      ``planned``/``confirmed``, drop for ``otot``, and go entirely for
      ``cancelled``/``done``;
    * answers survive going to ``confirmed`` or ``done`` (they were about this
      run) and are cleared coming *back* from ``cancelled``/``otot``, where they
      were about a run that was off;
    * nothing is posted when the status did not actually change.

    ``mark`` adds the ``(via portal)`` suffix; a slash command turns it off,
    because that change *was* a chat decision.
    """
    if status not in SETTABLE_STATUSES:
        raise BadRequest(
            f"`{status}` is not a status you can set - one of {', '.join(SETTABLE_STATUSES)}. "
            "`at_risk` is derived from the answers people give."
        )
    run = load_run(bot, run_id)
    previous = run["status"]
    if previous == status:
        return run_view(bot, run)

    if status == "planned" and previous in ("cancelled", "otot", "done"):
        # Those answers were about a run that was off; ask again.
        for uid in run["participants"]:
            bot.repo.clear_rsvp(run["id"], uid)
    bot.repo.set_run_status(run["id"], status)
    refresh_run_reminders(bot.repo, run["id"], bot.tz, bot.ping_time, bot.countdowns)

    fresh = bot.repo.get_run(run["id"])
    if announce:
        who = audience(bot.repo, fresh["participants"], "status")
        notice = status_notice(bot, fresh, status, who)
        if notice:
            await _announce(bot, notice, who.mentioned, fresh["channel_id"], mark=mark)
    return run_view(bot, fresh)


async def otot_run(bot: BossBot, run_id: str) -> dict:
    """Own time: stays in the morning ping, loses its countdowns."""
    return await set_status(bot, run_id, "otot")


async def restore_run(bot: BossBot, run_id: str) -> dict:
    """Put a cancelled, own-time or finished run back on the schedule."""
    return await set_status(bot, run_id, "planned")


async def swap_participants(
    bot: BossBot,
    run_id: str,
    remove: Sequence[str] = (),
    add: Sequence[str] = (),
    mark: bool = True,
) -> dict:
    """Change who is on a run **for this week only**.

    Not the same as editing the fixed timing: a stand-in for one night must not
    rewrite the baseline, or next week pings the wrong people. The run's own
    participant list is what the day-of and countdown messages mention, so
    changing it here is all that is needed.

    Answers from anyone taken off are cleared -- theirs was about a run they are
    no longer on -- and a run may not be emptied.
    """
    run = load_run(bot, run_id)
    leaving = [str(u) for u in remove or ()]
    joining = validate_participants(bot, add) if add else []

    people = [uid for uid in run["participants"] if uid not in leaving]
    unknown = [uid for uid in leaving if uid not in run["participants"]]
    if unknown:
        raise BadRequest(
            "not on this run: " + ", ".join(f"{member_name(bot, u)} ({u})" for u in unknown)
        )
    for uid in joining:
        if uid not in people:
            people.append(uid)
    if not people:
        raise BadRequest("a run needs at least one participant - cancel it instead")
    if people == run["participants"]:
        return run_view(bot, run)

    bot.repo.set_run_participants(run["id"], people)
    for uid in leaving:
        bot.repo.clear_rsvp(run["id"], uid)

    # Recompute: losing the person who said no can settle a run, and gaining
    # someone who has not answered unsettles one -- including one that was
    # confirmed by hand, which an incomplete tally alone would no longer undo.
    recompute_after_roster_change(bot.repo, run["id"])
    fresh = bot.repo.get_run(run["id"])

    who = audience(bot.repo, [*fresh["participants"], *leaving], "swap")
    await _announce(
        bot,
        formatting.swap_notice(fresh, leaving, joining, bot.tz, who),
        who.mentioned,
        fresh["channel_id"],
        mark=mark,
    )
    return run_view(bot, fresh)


def roster_change(bot: BossBot, run: dict) -> dict:
    """How this week's party differs from the fixed timing behind it.

    Shown as "(this week: -X +Y)" so a one-night stand-in is visible without
    opening anything -- the baseline is unchanged, and that is easy to forget.
    """
    fixed_id = run.get("fixed_run_id")
    fixed = bot.repo.get_fixed_run(fixed_id) if fixed_id else None
    if fixed is None:
        return {"out": [], "in": [], "changed": False}
    baseline = list(fixed["participants"])
    out = [uid for uid in baseline if uid not in run["participants"]]
    joined = [uid for uid in run["participants"] if uid not in baseline]
    return {
        "out": [{"id": uid, "name": member_name(bot, uid)} for uid in out],
        "in": [{"id": uid, "name": member_name(bot, uid)} for uid in joined],
        "changed": bool(out or joined),
    }


#: What an answer can be set to from the portal, the API or `bossctl`.
#: ``clear`` is not an answer but the removal of one -- the correction for a
#: reaction somebody left by accident, which needs to leave the person
#: *unanswered* rather than recorded as a maybe.
RSVP_ANSWERS = ("yes", "no", "maybe", "clear")


async def set_rsvp(bot: BossBot, run_id: str, user_id: int | str, answer: str) -> dict:
    run = load_run(bot, run_id)
    if answer not in RSVP_ANSWERS:
        raise BadRequest("answer must be `yes`, `no`, `maybe` or `clear`")
    uid = str(user_id)
    if uid not in run["participants"]:
        raise BadRequest(f"{member_name(bot, uid)} isn't on run {short_id(run['id'])}")
    if answer == "clear":
        bot.repo.clear_rsvp(run["id"], uid)
    else:
        bot.repo.set_rsvp(run["id"], uid, answer, source="chat")
    new_status = compute_status(run["status"], run["participants"], bot.repo.get_rsvps(run["id"]))
    if new_status != run["status"]:
        bot.repo.set_run_status(run["id"], new_status)
    fresh = bot.repo.get_run(run["id"])
    if answer == "no":
        await bot.notify_decline(fresh, uid, member_name(bot, uid))
    else:
        await bot.retract_decline(fresh, uid)
    return run_view(bot, fresh)


async def _announce(
    bot: BossBot,
    content: str,
    mention_users: Sequence[str],
    channel_id: str | None,
    mark: bool = True,
) -> None:
    """Post a change notice in a run's home channel.

    Everything the portal, the CLI and the API do to the schedule goes through
    here, marked ``(via portal)``: a run that moves without anyone in the
    channel having said anything is otherwise baffling to the party.
    """
    found = await bot.find_channel(channel_id)
    if found.channel is None:
        # A notice is a courtesy, not the change itself: the schedule edit has
        # already happened, so this warns rather than failing the request.
        log.warning("dropping a portal notice: %s", found.problem)
        return
    channel = found.channel
    await bot.post_plain(
        channel, formatting.via_portal(content) if mark else content, list(mention_users)
    )


# ---------------------------------------------------------------------------
# the inbox: approving and rejecting what the extractor proposed
# ---------------------------------------------------------------------------


def pending(bot: BossBot, channel_id: int | str | None = None) -> list[dict]:
    return [
        amendment_view(bot, a)
        for a in bot.repo.list_amendments(status="proposed", channel_id=channel_id)
    ]


async def approve(bot: BossBot, amendment_id: str, actor_id: int | str | None = None) -> dict:
    """Apply a proposed change -- the same code path a ✅ on the card runs.

    The Discord card is annotated so the channel sees the decision even though
    nobody reacted to it.
    """
    amendment = load_amendment(bot, amendment_id)
    if amendment["status"] != "proposed":
        raise BadRequest(f"that change is already `{amendment['status']}`")
    actor = str(actor_id or bot.portal_actor_id)
    result = commit(
        bot.repo,
        amendment,
        tz=bot.tz,
        reset_weekday=bot.settings.reset_weekday,
        reset_time=bot.settings.reset_time,
        ping_time=bot.ping_time,
        countdowns=bot.countdowns,
        actor_id=actor,
        channel_id=amendment.get("channel_id"),
        on_fixed_created=lambda _fixed_id: bot.materialise_weeks(),
    )
    if not result.applied:
        raise BadRequest(result.problem or "that change could not be applied")

    await bot._mark_superseded(result.superseded)
    await bot.annotate_message(
        amendment["channel_id"], amendment["proposal_message_id"], PORTAL_APPLIED
    )
    if result.kind == "move" and result.run_id and result.old_datetime is not None:
        run = bot.repo.get_run(result.run_id)
        if run is not None:
            who = audience(bot.repo, run["participants"], "amend")
            await _announce(
                bot,
                formatting.amend_notice(run, result.old_datetime, bot.tz, who),
                who.mentioned,
                run["channel_id"],
            )
    return {
        "id": amendment["id"],
        "short_id": short_id(amendment["id"]),
        "kind": result.kind,
        "applied": True,
        "actor_id": actor,
        "run_id": result.run_id,
        "fixed_run_id": result.fixed_run_id,
        "created_run_ids": result.created_run_ids,
        "superseded": [short_id(a["id"]) for a in result.superseded],
    }


async def reject_amendment(bot: BossBot, amendment_id: str) -> dict:
    amendment = load_amendment(bot, amendment_id)
    if amendment["status"] != "proposed":
        raise BadRequest(f"that change is already `{amendment['status']}`")
    reject(bot.repo, amendment)
    await bot.annotate_message(
        amendment["channel_id"], amendment["proposal_message_id"], PORTAL_REJECTED
    )
    return {"id": amendment["id"], "short_id": short_id(amendment["id"]), "status": "rejected"}


# ---------------------------------------------------------------------------
# members, reminders, config
# ---------------------------------------------------------------------------


def _run_counts(bot: BossBot) -> dict[str, int]:
    counts: dict[str, int] = {}
    for run in bot.repo.list_runs(week_start=week_for(bot, "this"), include_cancelled=False):
        for uid in run["participants"]:
            counts[uid] = counts.get(uid, 0) + 1
    return counts


def members(bot: BossBot, with_role: bool = True) -> list[dict]:
    counts = _run_counts(bot)
    return [member_view(bot, m, counts) for m in bot.repo.list_members(with_role=with_role)]


def update_member(bot: BossBot, user_id: int | str, ping_level: str | None = None) -> dict:
    """Edit one member's own settings. Only ``ping_level`` is editable today."""
    member = bot.repo.get_member(user_id)
    if member is None:
        raise NotFound(f"no member {user_id} - the roster syncs from the bossing role")
    if ping_level is None:
        raise BadRequest("nothing to change - pass `ping_level`")
    try:
        level = normalise_level(ping_level)
    except ValueError as exc:
        raise BadRequest(str(exc)) from None
    bot.repo.set_ping_level(user_id, level)
    return member_view(bot, bot.repo.get_member(user_id), _run_counts(bot))


def set_nick(bot: BossBot, user_id: int | str, alias: str) -> dict:
    alias = (alias or "").strip()
    if not alias:
        raise BadRequest("alias can't be empty")
    member = bot.repo.get_member(user_id)
    if member is None:
        raise NotFound(f"no member {user_id} - the roster syncs from the bossing role")
    aliases = bot.repo.add_alias(user_id, alias)
    return {"user_id": str(user_id), "name": member_name(bot, user_id), "aliases": aliases}


def reminders(bot: BossBot, run_id: str | None = None, limit: int = 200) -> list[dict]:
    rows: list[tuple[dict, dict | None]] = []
    if run_id:
        run = load_run(bot, run_id)
        rows = [(r, run) for r in bot.repo.list_reminders(run["id"])]
    else:
        for run in bot.repo.list_runs():
            rows.extend((r, run) for r in bot.repo.list_reminders(run["id"]))
    rows.sort(key=lambda pair: pair[0]["fire_at"], reverse=True)
    return [reminder_view(bot, reminder, run) for reminder, run in rows[:limit]]


def access_report(bot: BossBot) -> list[dict]:
    """Per-channel read/send permissions, so missing ones are visible at a glance."""
    return bot.access_report()


def get_config(bot: BossBot) -> dict:
    return {
        "day_of_ping_time": bot.ping_time.strftime("%H:%M"),
        "countdown_minutes": ",".join(str(m) for m in bot.countdowns),
        "paused": bot.paused,
        "extract_enabled": bot.extract_enabled,
        "quiet_mode": bot.quiet_mode,
        "timezone": bot.settings.tz,
        "reset": f"{WEEKDAY_NAMES[bot.settings.reset_weekday]} "
        f"{bot.settings.reset_time.strftime('%H:%M')}",
        "model": bot.settings.ollama_model,
        "ollama_host": bot.settings.ollama_host,
        "min_confidence": bot.settings.extract_min_confidence,
        "post_channel_id": str(bot.settings.post_channel_id)
        if bot.settings.post_channel_id
        else None,
        "guild_id": str(bot.settings.guild_id),
        "watched_channels": [str(c) for c in bot.settings.chat_channel_id_list],
        "watched_categories": [str(c) for c in bot.settings.chat_category_id_list],
        # Read live from the guild every time this is asked for: a permission
        # granted in Discord must show up here without a restart.
        "missing_manage_messages": bot.missing_manage_messages(),
    }


def _as_flag(value: Any) -> str:
    text = str(value).strip().lower()
    if text in ("1", "true", "yes", "on"):
        return "1"
    if text in ("0", "false", "no", "off"):
        return "0"
    raise BadRequest(f"`{value}` is not a yes/no value")


def set_config(bot: BossBot, key: str, value: Any) -> dict:
    """Write one runtime setting, validating it the way its slash command does."""
    key = (key or "").strip().lower()
    if key not in CONFIG_KEYS:
        raise BadRequest(f"unknown setting `{key}` - one of {', '.join(CONFIG_KEYS)}")
    if key == "day_of_ping_time":
        try:
            stored = parse_hhmm(str(value)).strftime("%H:%M")
        except ValueError as exc:
            raise BadRequest(str(exc)) from None
        bot.repo.set_config(key, stored)
        # Re-place every morning ping that has not fired yet, as /pingtime does.
        for reminder in bot.repo.unsent_reminders(kind="day_of"):
            refresh_run_reminders(
                bot.repo, reminder["run_id"], bot.tz, bot.ping_time, bot.countdowns
            )
    elif key == "countdown_minutes":
        minutes = [p.strip() for p in str(value).replace(";", ",").split(",") if p.strip()]
        try:
            parsed = [int(m) for m in minutes]
        except ValueError:
            raise BadRequest("countdown_minutes must be whole minutes, e.g. `60,15`") from None
        if not parsed or any(m <= 0 for m in parsed):
            raise BadRequest("countdown_minutes must be positive whole minutes, e.g. `60,15`")
        bot.repo.set_config(key, ",".join(str(m) for m in sorted(set(parsed), reverse=True)))
        for run in bot.repo.list_runs():
            refresh_run_reminders(bot.repo, run["id"], bot.tz, bot.ping_time, bot.countdowns)
    else:
        bot.repo.set_config(key, _as_flag(value))
    return get_config(bot)


# ---------------------------------------------------------------------------
# things that talk to Discord or the model
# ---------------------------------------------------------------------------


async def post_digest(
    bot: BossBot, channel_id: int | str | None = None, week: str = "this"
) -> dict:
    week_for(bot, week)  # validates
    found = await bot.find_channel(channel_id)
    if found.channel is None:
        raise BadRequest(f"couldn't post the digest: {found.problem}")
    message = await bot.post_digest(channel_id, week=week)
    if message is None:
        raise BadRequest(
            "couldn't post the digest - Discord rejected the message; check the bot logs"
        )
    cid = getattr(getattr(message, "channel", None), "id", channel_id)
    return {
        "posted": True,
        "week": week,
        "channel_id": str(cid) if cid else None,
        "message_id": str(message.id),
        "url": message_url(bot, cid, message.id),
    }


#: General channels (no fixed run lives there) are offered last: a rescan is
#: usually about a party's own channel, and the general one is the noisy one.
def rescan_targets(bot: BossBot) -> list[dict]:
    """Every watched channel a rescan can cover, party channels first.

    Resolved from the live guild so a channel added to a watched category is
    included without a restart -- the same rule the listener applies.
    """
    with_runs = {f["channel_id"] for f in bot.repo.list_fixed_runs() if f["channel_id"]}
    rows = []
    for channel in bot.watched_text_channels():
        rows.append(
            {
                "id": str(channel.id),
                "name": f"#{channel.name}",
                "has_runs": str(channel.id) in with_runs,
            }
        )
    for cid in bot.settings.chat_channel_id_list:
        if not any(r["id"] == str(cid) for r in rows):
            rows.append(
                {
                    "id": str(cid),
                    "name": channel_name(bot, cid) or f"channel {cid}",
                    "has_runs": str(cid) in with_runs,
                }
            )
    # Party channels first, then the general ones, each group alphabetical.
    rows.sort(key=lambda r: (not r["has_runs"], r["name"]))
    return rows


def _check_rescan_allowed(bot: BossBot, window: str) -> None:
    if window not in WINDOWS:
        raise BadRequest(f"window must be one of {', '.join(WINDOWS)}")
    if bot.paused:
        raise BadRequest("chat watching is paused - resume it in Config first")
    if not bot.extract_enabled:
        raise BadRequest("the extractor is switched off - turn it on in Config first")


def resolve_rescan_channels(bot: BossBot, channels: Sequence[str] | None) -> list[str]:
    """The channels a rescan will cover; empty or ``None`` means all watched ones."""
    if not channels:
        picked = [row["id"] for row in rescan_targets(bot)]
        if not picked:
            raise BadRequest(
                "no watched channels are visible - check CHAT_CHANNEL_IDS / CHAT_CATEGORY_IDS, "
                "and that the bot is connected"
            )
        return picked
    for channel_id in channels:
        if not channel_is_watched(bot, channel_id):
            raise BadRequest(f"channel {channel_id} isn't watched, so there's nothing to re-read")
    return [str(c) for c in channels]


async def rescan_one(
    bot: BossBot,
    channel_id: int | str,
    window: str = DEFAULT_WINDOW,
    post: bool = True,
    automated: bool = False,
    should_stop: Callable[[], bool] | None = None,
) -> dict:
    """Backfill one channel from Discord, then re-read the window burst by burst."""
    report = await bot.extractor.rescan_window(
        channel_id, window=window, post=post, automated=automated, should_stop=should_stop
    )
    return {
        "channel_id": str(channel_id),
        "channel_name": channel_name(bot, channel_id) or f"channel {channel_id}",
        "asked": report.asked,
        "window": report.window,
        "since": to_iso(report.since),
        "widened": report.widened,
        "backfilled": report.backfilled,
        "stored": report.stored,
        "gated": report.gated,
        "bursts": report.bursts,
        "extracted": report.extracted,
        "proposals": report.proposals,
        "dropped": report.dropped,
        "stale": report.stale,
        "elapsed_ms": report.elapsed_ms,
        "cancelled": report.cancelled,
        "error": report.errors[0] if report.errors else None,
        # Every burst that had something to say, not just the first one: a week
        # is several conversations and one of their summaries describes one.
        "summary": "; ".join(dict.fromkeys(p.summary for p in report.plans if p.summary)),
        "proposed": [
            {
                "kind": p.kind,
                "bosses": list(p.amendment.bosses),
                "confidence": p.amendment.confidence,
                "run_id": p.run["id"] if p.run else None,
            }
            for p in report.planned
        ],
    }


def rescan_totals(window: str, results: Sequence[dict]) -> dict:
    """Roll a per-channel rescan up into one answer."""
    return {
        "window": window,
        "channels": list(results),
        "asked": any(r["asked"] for r in results),
        "widened": any(r["widened"] for r in results),
        "backfilled": sum(r["backfilled"] for r in results),
        "gated": sum(r["gated"] for r in results),
        "bursts": sum(r["bursts"] for r in results),
        "extracted": sum(r["extracted"] for r in results),
        "proposals": sum(r["proposals"] for r in results),
        "dropped": sum(r["dropped"] for r in results),
        "stale": sum(r["stale"] for r in results),
        "elapsed_ms": sum(r["elapsed_ms"] for r in results),
        "errors": [r["error"] for r in results if r["error"]],
        "proposed": [p for r in results for p in r["proposed"]],
    }


def queue_rescan(
    bot: BossBot,
    channels: Sequence[str] | None = None,
    window: str = DEFAULT_WINDOW,
    source: str = "manual",
    automated: bool = False,
    requested_by: int | str | None = None,
) -> dict:
    """Put a rescan on the queue and return the job to watch.

    Deliberately *not* awaited: re-reading a boss week is minutes of model time,
    and doing it inline froze the reminder tick, reactions and every other
    command behind Ollama. :class:`bot.rescan.RescanWorker` drains the queue one
    channel at a time while the rest of the bot carries on.
    """
    from ..rescan import job_view

    _check_rescan_allowed(bot, window)
    targets = resolve_rescan_channels(bot, channels)
    names = {row["id"]: row["name"] for row in rescan_targets(bot)}
    job = bot.rescans.submit(
        targets,
        window=window,
        source=source,
        automated=automated,
        requested_by=requested_by,
        names=names,
    )
    return job_view(job)


def rescan_job(bot: BossBot, job_id: str) -> dict:
    """One job's progress, live from memory."""
    from ..rescan import job_view

    job = bot.rescans.get(job_id)
    if job is None:
        raise NotFound("that rescan is no longer in memory - start another one")
    view = job_view(job)
    view["totals"] = rescan_totals(view["window"], view["results"])
    return view


def cancel_rescan(bot: BossBot, job_id: str) -> dict:
    """Ask a queued or running rescan to stop between bursts."""
    if not bot.rescans.cancel(job_id):
        job = bot.rescans.get(job_id)
        if job is None:
            raise NotFound("that rescan is no longer in memory")
        raise BadRequest(f"that rescan is already `{job.status}`")
    return rescan_job(bot, job_id)


def recent_rescans(bot: BossBot, limit: int = 5) -> list[dict]:
    """The last few rescans, newest first -- what the Config page shows."""
    from ..rescan import job_view

    return [job_view(job) for job in bot.rescans.recent(limit)]


async def rescan(
    bot: BossBot,
    channels: Sequence[str] | None = None,
    window: str = DEFAULT_WINDOW,
    post: bool = True,
    automated: bool = False,
) -> dict:
    """Re-read channels **inline**, waiting for the result.

    Only for callers that genuinely want to block -- the offline dry run, and
    tests. Everything a person triggers goes through :func:`queue_rescan`.
    """
    _check_rescan_allowed(bot, window)
    targets = resolve_rescan_channels(bot, channels)
    results = [
        await rescan_one(bot, channel_id, window=window, post=post, automated=automated)
        for channel_id in targets
    ]
    return rescan_totals(window, results)


async def debug_ping(bot: BossBot, run_id: str, kind: str) -> dict:
    """Post one real reminder message now, without touching the run's reminder rows."""
    from ..debug import TEST_PREFIX, DebugGroup

    run = load_run(bot, run_id)
    card = DebugGroup._render(bot, run, kind, _PortalActor(bot))
    if card is None:
        raise BadRequest(
            f"don't know how to render `{kind}` - try day_of, countdown_60, amend or decline"
        )
    found = await bot.find_channel(run["channel_id"])
    if found.channel is None:
        raise BadRequest(f"couldn't post the test ping: {found.problem}")
    channel = found.channel
    card.content = TEST_PREFIX + card.content
    message = await bot._post(channel, card)
    if message is None:
        raise BadRequest("couldn't post the test message")
    bot.repo.add_debug_message(message.id, run["id"], getattr(channel, "id", None), kind)
    cid = getattr(channel, "id", None)
    return {
        "run_id": run["id"],
        "kind": kind,
        "channel_id": str(cid) if cid else None,
        "message_id": str(message.id),
        "url": message_url(bot, cid, message.id),
    }


class _PortalActor:
    """Stands in for the ``interaction`` ``/debug ping`` renders a `decline` against."""

    def __init__(self, bot: BossBot):
        self.user = _PortalUser(bot)


class _PortalUser:
    def __init__(self, bot: BossBot):
        self.id = bot.portal_actor_id
        self.display_name = member_name(bot, bot.portal_actor_id)


async def export_messages(
    bot: BossBot, channel_id: int | str, since: datetime, until: datetime | None = None
) -> AsyncIterator[dict]:
    """Stream one watched channel's messages as JSONL records (DESIGN.md §5).

    Pages Discord history when the channel is reachable -- which is the real
    export, and keeps the ``messages`` table in step as it goes -- and falls
    back to what is already stored when it is not.
    """
    if not channel_is_watched(bot, channel_id):
        raise BadRequest(
            f"channel {channel_id} isn't watched - only watched channels can be exported"
        )
    channel = bot.get_channel(int(channel_id))
    history = getattr(channel, "history", None)
    if history is None:
        for row in bot.repo.recent_messages(channel_id, since, until):
            yield {
                "id": str(row["id"]),
                "channel_id": str(row["channel_id"]),
                "channel_name": channel_name(bot, row["channel_id"]),
                "author_id": row["author_id"],
                "author_name": member_name(bot, row["author_id"]),
                "created_at": to_iso(row["created_at"]),
                "content": row["content"],
                "source": "stored",
            }
        return
    name = getattr(channel, "name", str(channel_id))
    async for message in history(after=since, before=until, oldest_first=True, limit=None):
        record = message_record(message, name)
        bot.repo.record_message(
            message.id,
            record["channel_id"],
            message.author.id,
            message.created_at,
            message.content or "",
        )
        record["source"] = "discord"
        yield record


def parse_since(bot: BossBot, value: str, field: str = "since") -> datetime:
    """``YYYY-MM-DD`` (midnight in the guild timezone) or a full ISO timestamp."""
    text = (value or "").strip()
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        raise BadRequest(f"{field} must be YYYY-MM-DD or an ISO timestamp, got `{value}`") from None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=bot.tz)
    return parsed


__all__ = [
    "CONFIG_KEYS",
    "PORTAL_APPLIED",
    "PORTAL_REJECTED",
    "access_report",
    "amend_run",
    "amendment_view",
    "boss_grid",
    "boss_view",
    "bosses_in_use",
    "monogram",
    "portrait_url",
    "approve",
    "cancel_run",
    "channel_is_watched",
    "create_fixed",
    "debug_ping",
    "delete_fixed",
    "export_messages",
    "extraction_view",
    "fixed_view",
    "get_config",
    "load_amendment",
    "load_extraction",
    "load_fixed",
    "load_run",
    "member_name",
    "render_mentions",
    "members",
    "SETTABLE_STATUSES",
    "otot_run",
    "restore_run",
    "roster_change",
    "set_status",
    "swap_participants",
    "parse_since",
    "pending",
    "post_digest",
    "reject_amendment",
    "reminders",
    "cancel_rescan",
    "queue_rescan",
    "recent_rescans",
    "rescan",
    "rescan_job",
    "rescan_one",
    "rescan_targets",
    "rescan_totals",
    "resolve_rescan_channels",
    "run_view",
    "schedule",
    "set_config",
    "set_nick",
    "set_rsvp",
    "week_for",
    "week_rail",
]
