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
from collections.abc import AsyncIterator, Iterable, Sequence
from datetime import datetime, time, timedelta
from typing import TYPE_CHECKING, Any

import dateparser

from .. import formatting
from ..bosses import BossParseError
from ..export import message_record
from ..extract.commit import commit, reject
from ..ids import IdAmbiguous, IdError, resolve_id, short_id
from ..materialise import refresh_run_reminders
from ..rsvp import compute_status
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
CONFIG_KEYS = ("day_of_ping_time", "countdown_minutes", "paused", "extract_enabled")


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


def boss_view(bot: BossBot, token: str) -> dict:
    """``"HFA"`` plus the difficulty letter and the full in-game name."""
    parts = bot.bosses.split(token)
    return {
        "token": token,
        "difficulty": parts[0].upper() if parts else "",
        "label": bot.bosses.describe(token),
    }


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
        "unanswered": sum(1 for p in participants if p["rsvp"] is None),
    }


def fixed_view(bot: BossBot, fixed: dict) -> dict:
    return {
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
                "content": row["content"],
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
        "run_id": amendment["run_id"],
        "run": run_view(bot, run) if run else None,
        "new_datetime": to_iso(amendment["new_datetime"]) if amendment["new_datetime"] else None,
        "when": formatting.when_text(amendment, bot.tz).replace("**", ""),
        "day_ref": amendment["day_ref"],
        "time_ref": amendment["time_ref"],
        "confidence": amendment["confidence"],
        "is_question": amendment["is_question"],
        "summary": amendment["summary"],
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
    include_cancelled: bool = True,
) -> dict:
    """One week's runs, filtered, grouped by day -- the Week view's whole payload."""
    ws = week_for(bot, week)
    runs = bot.repo.list_runs(
        week_start=ws,
        participant=str(user_id) if user_id else None,
        channel_id=channel_id,
        include_cancelled=include_cancelled,
    )
    if boss:
        wanted = boss.strip().lower()
        runs = [r for r in runs if any(wanted in b.lower() for b in r["bosses"])]
    views = [run_view(bot, run) for run in runs]
    return {
        "week": week,
        "week_start": to_iso(ws),
        "week_label": ws.astimezone(bot.tz).strftime("%a %d %b"),
        "timezone": bot.settings.tz,
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


def create_fixed(
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
    return fixed_view(bot, bot.repo.get_fixed_run(fixed_id))


def update_fixed(bot: BossBot, fixed_id: str, **changes: Any) -> dict:
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
    return fixed_view(bot, bot.repo.get_fixed_run(fixed["id"]))


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


def delete_fixed(bot: BossBot, fixed_id: str) -> dict:
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
    await _announce(
        bot,
        formatting.amend_notice(updated, old_at, bot.tz),
        updated["participants"],
        updated["channel_id"],
    )
    return run_view(bot, updated)


async def cancel_run(bot: BossBot, run_id: str) -> dict:
    run = load_run(bot, run_id)
    bot.repo.set_run_status(run["id"], "cancelled")
    refresh_run_reminders(bot.repo, run["id"], bot.tz, bot.ping_time, bot.countdowns)
    await _announce(
        bot,
        f"🚫 **{formatting.format_bosses(run['bosses'])}** "
        f"({formatting.local_day(run['datetime'], bot.tz)}) is cancelled — "
        f"{formatting.format_participants(run['participants'])}",
        run["participants"],
        run["channel_id"],
    )
    return run_view(bot, bot.repo.get_run(run["id"]))


def otot_run(bot: BossBot, run_id: str) -> dict:
    """Own time: stays in the morning ping, loses its countdowns."""
    run = load_run(bot, run_id)
    bot.repo.set_run_status(run["id"], "otot")
    refresh_run_reminders(bot.repo, run["id"], bot.tz, bot.ping_time, bot.countdowns)
    return run_view(bot, bot.repo.get_run(run["id"]))


async def set_rsvp(bot: BossBot, run_id: str, user_id: int | str, answer: str) -> dict:
    run = load_run(bot, run_id)
    if answer not in ("yes", "no", "maybe"):
        raise BadRequest("answer must be `yes`, `no` or `maybe`")
    uid = str(user_id)
    if uid not in run["participants"]:
        raise BadRequest(f"{member_name(bot, uid)} isn't on run {short_id(run['id'])}")
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
    bot: BossBot, content: str, mention_users: Sequence[str], channel_id: str | None
) -> None:
    channel = await bot.post_channel(channel_id)
    if channel is None:
        log.warning("no channel available; dropping the portal's announcement")
        return
    await bot.post_plain(channel, content, list(mention_users))


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
            await _announce(
                bot,
                formatting.amend_notice(run, result.old_datetime, bot.tz),
                run["participants"],
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


def members(bot: BossBot, with_role: bool = True) -> list[dict]:
    counts: dict[str, int] = {}
    for run in bot.repo.list_runs(week_start=week_for(bot, "this"), include_cancelled=False):
        for uid in run["participants"]:
            counts[uid] = counts.get(uid, 0) + 1
    return [member_view(bot, m, counts) for m in bot.repo.list_members(with_role=with_role)]


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


def get_config(bot: BossBot) -> dict:
    return {
        "day_of_ping_time": bot.ping_time.strftime("%H:%M"),
        "countdown_minutes": ",".join(str(m) for m in bot.countdowns),
        "paused": bot.paused,
        "extract_enabled": bot.extract_enabled,
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
    message = await bot.post_digest(channel_id, week=week)
    if message is None:
        raise BadRequest(
            "couldn't post the digest - set POST_CHANNEL_ID, or pass a channel the bot can see"
        )
    cid = getattr(getattr(message, "channel", None), "id", channel_id)
    return {
        "posted": True,
        "week": week,
        "channel_id": str(cid) if cid else None,
        "message_id": str(message.id),
        "url": message_url(bot, cid, message.id),
    }


async def rescan(bot: BossBot, channel_id: int | str, hours: int = 24, post: bool = True) -> dict:
    if not channel_is_watched(bot, channel_id):
        raise BadRequest(f"channel {channel_id} isn't watched, so nothing is stored for it")
    if bot.paused:
        raise BadRequest("chat watching is paused - resume it in Config first")
    if not bot.extract_enabled:
        raise BadRequest("the extractor is switched off - turn it on in Config first")
    hours = max(1, min(int(hours), 168))
    plan = await bot.extractor.rescan(channel_id, hours=hours, post=post)
    if plan is None:
        return {"asked": False, "hours": hours, "proposed": [], "dropped": 0, "summary": ""}
    return {
        "asked": True,
        "hours": hours,
        "error": plan.error,
        "latency_ms": plan.latency_ms,
        "summary": plan.summary,
        "dropped": len(plan.dropped),
        "amendment_ids": plan.amendment_ids,
        "proposed": [
            {
                "kind": p.kind,
                "bosses": list(p.amendment.bosses),
                "confidence": p.amendment.confidence,
                "run_id": p.run["id"] if p.run else None,
            }
            for p in plan.planned
        ],
    }


async def debug_ping(bot: BossBot, run_id: str, kind: str) -> dict:
    """Post one real reminder message now, without touching the run's reminder rows."""
    from ..debug import TEST_PREFIX, DebugGroup

    run = load_run(bot, run_id)
    card = DebugGroup._render(bot, run, kind, _PortalActor(bot))
    if card is None:
        raise BadRequest(
            f"don't know how to render `{kind}` - try day_of, countdown_60, amend or decline"
        )
    channel = await bot.post_channel(run["channel_id"])
    if channel is None:
        raise BadRequest("that run's home channel isn't reachable")
    card.content = TEST_PREFIX + card.content
    message = await bot._post(channel, card, mention_users=run["participants"])
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
    "amend_run",
    "amendment_view",
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
    "members",
    "otot_run",
    "parse_since",
    "pending",
    "post_digest",
    "reject_amendment",
    "reminders",
    "rescan",
    "run_view",
    "schedule",
    "set_config",
    "set_nick",
    "set_rsvp",
    "week_for",
    "week_rail",
]
