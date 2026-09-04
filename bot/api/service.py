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

from .. import audit, behaviour_plugins, events, formatting
from ..bosses import BossParseError
from ..export import message_record
from ..extract import resolve
from ..extract.commit import commit, reject
from ..extract.window import DEFAULT_WINDOW, WINDOWS
from ..ids import IdAmbiguous, IdError, resolve_id, short_id
from ..materialise import (
    DAY_OF,
    LIVE_STATUSES,
    countdown_minutes,
    ensure_reminders,
    reconcile_day_of,
    refresh_run_reminders,
    retire_fixed_run,
)
from ..pings import audience, normalise_level
from ..rsvp import compute_status, recompute_after_roster_change
from ..timeutil import from_iso, local_naive, to_iso, utcnow
from ..util import is_bot_admin
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


def _audit(
    bot: BossBot,
    action: str,
    subject: str | None = None,
    detail: str = "",
    actor: audit.Actor | None = None,
) -> None:
    """Note a change on the audit trail, crediting whoever asked for it.

    The actor comes from the request in flight (:class:`bot.api.app.
    ActorMiddleware` puts it there) unless a caller knows better. Called *after*
    the change, so a mutation that raised leaves no row claiming it happened.
    """
    audit.record(bot.repo, actor or audit.current(), action, subject, detail)


def _bosses_of(run: dict) -> str:
    """The run's bosses as an audit line says them: `HStar + HFA`."""
    return formatting.format_bosses(run["bosses"])


#: Runtime config the portal may read and write, and how to validate each one.
#: Anything not listed here is refused -- `config set` is not a way to write
#: arbitrary rows into the `config` table.
CONFIG_KEYS = (
    "day_of_ping_time",
    "countdown_minutes",
    "paused",
    "extract_enabled",
    "quiet_mode",
    "chat_mode",
    "persona",
    behaviour_plugins.CONFIG_KEY,
    "chat_pilot_rate_count",
    "chat_pilot_rate_window_s",
    "chat_pilot_global_rate_count",
    "chat_pilot_global_rate_window_s",
)

#: The capacity settings, split by what a valid value looks like: a whole number
#: of answers, or a number of seconds. Both are checked here rather than trusted
#: from the request, because ``bossctl config set`` and the portal both arrive
#: as text and a window of zero would refuse everybody for ever.
COUNT_KEYS = ("chat_pilot_rate_count", "chat_pilot_global_rate_count")
WINDOW_KEYS = ("chat_pilot_rate_window_s", "chat_pilot_global_rate_window_s")


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
        "portrait": portrait_url(bot, detail["short"], PORTAL_PORTRAIT_SIZE),
        "monogram": monogram(detail["full"]),
    }


#: Every portrait the portal draws is a badge -- 26px beside a boss's name,
#: 38px in the boss grid, and nothing anywhere bigger -- so all of them ask for
#: the small render. The full file is artwork now: it is what Discord attaches
#: as a card's thumbnail (:func:`bot.formatting.lead_portrait`), and what
#: ``?size=full`` still serves to anything that wants it.
PORTAL_PORTRAIT_SIZE = "icon"


def portrait_url(bot: BossBot, short: str, size: str = "full") -> str | None:
    """The portal URL for a boss portrait, or ``None`` when there is no file.

    The size rides in the query rather than in the path so the two renders are
    two cache entries under one route -- and so a boss with no small file, which
    falls back to the big one, is still asked for under the URL the page wrote.
    """
    if bot.bosses.portrait_path(short, size) is None:
        return None
    return f"/static/portraits/{short}" + (f"?size={size}" if size != "full" else "")


def entry_art_url(bot: BossBot, short: str) -> str | None:
    """The portal URL for a boss's entry artwork, or ``None`` when there is none."""
    return f"/static/entry/{short}" if bot.bosses.entry_art_path(short) else None


#: How many pieces of entry artwork one card can wear. Two, because the sheet
#: splits its right edge diagonally between them and a third would have nowhere
#: to be -- and because a run here has never named more than two bosses anyway.
MAX_ENTRY_ART = 2


def run_entry_art(bot: BossBot, bosses: Sequence[str]) -> list[str]:
    """The artwork a run's cards wear, in the order the run names its bosses.

    The lead boss is first for the reason it always was: a run is named after
    the boss it leads with, which is the same choice
    :func:`bot.formatting.lead_portrait` makes for a card in Discord. The
    compact card on the board has room for one picture and uses that one; the
    sheet has room for the second and splits its right edge between them.

    A boss with no file is absent rather than a gap, so two bosses of which one
    has artwork make a one-layer card and not a half-empty two-layer one. An
    empty list is the ordinary case on a fresh clone, the artwork being
    git-ignored -- every surface that reads this has to render without it.
    """
    urls: list[str] = []
    for token in bosses:
        parts = bot.bosses.split(token)
        url = entry_art_url(bot, parts[1].short) if parts else None
        if url is not None:
            urls.append(url)
        if len(urls) == MAX_ENTRY_ART:
            break
    return urls


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
                "portrait": portrait_url(bot, boss.short, PORTAL_PORTRAIT_SIZE),
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
        # Up to two, lead boss first -- see `run_entry_art`. Read by the board's
        # compact cards, which take the first, and by the run card itself, which
        # takes both: the same run view rendered twice rather than two sets of
        # facts.
        "entry_art": run_entry_art(bot, run["bosses"]),
        "datetime": to_iso(run["datetime"]),
        "local_date": local.strftime("%Y-%m-%d"),
        "local_day": formatting.local_day(run["datetime"], bot.tz),
        "local_time": formatting.local_time(run["datetime"], bot.tz),
        "weekday": local.weekday(),
        "status": run["status"],
        "status_label": formatting.STATUS_LABEL.get(run["status"], run["status"]),
        # The label's own emoji, for the board's compact cards, where the words
        # do not fit and inventing a second vocabulary for them would be worse.
        "status_mark": formatting.STATUS_MARK.get(run["status"], "•"),
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


def kind_label(amendment: dict) -> str:
    """What a card is called, everywhere the portal names one.

    Three cards share the ``fix`` kind and are told apart only by the ``op``
    marker in their payload (:data:`bot.extract.commit.FIX_REMOVE` and
    ``FIX_EDIT``), so the kind by itself calls a card that retires a weekly
    timing -- or changes one -- "new weekly", which is its opposite. Exactly the
    rule :func:`bot.formatting.proposal_line` applies, so one card is called one
    thing whether it is read in Discord or in the portal.
    """
    payload = amendment["payload"] or {}
    verb = formatting.FIX_VERB.get(str(payload.get("op"))) if amendment["kind"] == "fix" else None
    return verb or formatting.KIND_VERB.get(amendment["kind"], amendment["kind"])


def when_label(bot: BossBot, amendment: dict) -> str:
    """The one-line "when" the inbox and the extraction table print.

    :func:`bot.formatting.when_text` with the card's bold taken off, except for
    a `fix`. A recurring night is a weekday and an HH:MM kept in the payload and
    never reaches ``new_datetime``, so ``when_text`` has nothing to find and all
    three weekly cards read **TBD** here -- silent about the single fact each one
    is proposing. Read from the payload for the same reason
    :func:`bot.formatting.weekly_text` and
    :func:`bot.formatting.weekly_change_text` read it from there, so a weekly
    timing is named the same way in the portal as on its card.

    Whichever night the card is *about*: the one being proposed where there is
    one, and otherwise the one it already has -- a card retiring a timing, or
    changing only its party, says what that night is rather than nothing. A
    `fix` with no night anywhere (a day with no time) falls through to the words
    that were actually written, which is what its card falls back to too.
    """
    payload = amendment["payload"] or {}
    if amendment["kind"] == "fix":
        weekday, hhmm = payload.get("weekday"), payload.get("time")
        if weekday is not None and hhmm:
            return f"every {WEEKDAY_NAMES[int(weekday)]} {hhmm}"
        if payload.get("weekly_when"):
            return f"every {payload['weekly_when']}"
    return formatting.when_text(amendment, bot.tz).replace("**", "")


def amendment_view(bot: BossBot, amendment: dict, with_evidence: bool = True) -> dict:
    run = bot.repo.get_run(amendment["run_id"]) if amendment["run_id"] else None
    view = {
        "id": amendment["id"],
        "short_id": short_id(amendment["id"]),
        "kind": amendment["kind"],
        "kind_label": kind_label(amendment),
        "status": amendment["status"],
        "bosses": amendment["bosses"],
        "boss_detail": [boss_view(bot, b) for b in amendment["bosses"]],
        "run_id": amendment["run_id"],
        "run": run_view(bot, run) if run else None,
        "new_datetime": to_iso(amendment["new_datetime"]) if amendment["new_datetime"] else None,
        "when": when_label(bot, amendment),
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


def created_cards(bot: BossBot, amendment_ids: Sequence[str]) -> list[dict]:
    """The proposal cards one tool call raised, named and linked.

    Only what a reader needs to click through: the short id the rest of the
    portal identifies a card by, what it proposes, whether anyone has answered
    it, and the deep link into Discord. An id whose amendment has since been
    deleted still appears -- the interaction really did create it, and silently
    dropping it would make a trace disagree with the log line beside it.
    """
    cards = []
    for amendment_id in amendment_ids:
        amendment = bot.repo.get_amendment(amendment_id)
        if amendment is None:
            cards.append({"id": str(amendment_id), "short_id": short_id(str(amendment_id))})
            continue
        cards.append(
            {
                "id": amendment["id"],
                "short_id": short_id(amendment["id"]),
                "kind": amendment["kind"],
                "kind_label": kind_label(amendment),
                "status": amendment["status"],
                "card_url": message_url(
                    bot, amendment["channel_id"], amendment["proposal_message_id"]
                ),
            }
        )
    return cards


def chat_interaction_view(bot: BossBot, interaction: dict, detail: bool = False) -> dict:
    """One handled chat interaction, as a row (or, with ``detail``, in full)."""
    tool_calls = interaction["tool_calls"]
    view = {
        "id": interaction["id"],
        "short_id": short_id(interaction["id"]),
        "at": to_iso(interaction["at"]),
        "local_time": interaction["at"].astimezone(bot.tz).strftime("%a %d %b %H:%M:%S"),
        "author_id": interaction["author_id"],
        "author_name": member_name(bot, interaction["author_id"]),
        "channel_id": interaction["channel_id"],
        "channel_name": channel_name(bot, interaction["channel_id"]),
        "model": interaction["model"],
        "outcome": interaction["outcome"],
        "rounds": interaction["rounds"],
        "latency_ms": interaction["latency_ms"],
        "tool_names": [call.get("name") or "?" for call in tool_calls],
        "tool_count": len(tool_calls),
        "prompt_tokens": interaction["prompt_tokens"],
        "completion_tokens": interaction["completion_tokens"],
        "url": message_url(bot, interaction["channel_id"], interaction["message_id"]),
    }
    if detail:
        view["question"] = render_mentions(bot, interaction["question"])
        view["reply"] = interaction["reply"]
        view["error"] = interaction["error"]
        view["model_ms"] = interaction["model_ms"]
        view["tools_ms"] = interaction["tools_ms"]
        view["message_id"] = interaction["message_id"]
        view["tool_calls"] = [
            {
                "name": call.get("name") or "?",
                # v9 added this to the persisted trace. A zero says this is an
                # older row, rather than inventing which round made the call.
                "round": int(call.get("round") or 0),
                "arguments": call.get("arguments") or "",
                "output": call.get("output") or "",
                "ms": call.get("ms"),
                "outcome": call.get("outcome") or "",
                "ok": call.get("outcome") == "ok",
                "created": created_cards(bot, call.get("created") or []),
                "posted": created_cards(bot, call.get("posted") or []),
            }
            for call in tool_calls
        ]
        view["model_rounds"] = [
            {
                "round": int(round_.get("round") or 0),
                "content": round_.get("content"),
                "thinking": round_.get("thinking"),
                "requested_tools": list(round_.get("requested_tools") or []),
            }
            for round_ in interaction.get("model_rounds") or []
        ]
    return view


def chat_interactions(bot: BossBot, limit: int = 50) -> list[dict]:
    """The most recent interactions, newest first -- what the Chat tab lists."""
    return [chat_interaction_view(bot, row) for row in bot.repo.recent_chat_interactions(limit)]


def load_chat_interaction(bot: BossBot, interaction_id: str) -> dict:
    resolved = _resolve(interaction_id, bot.repo.list_chat_interaction_ids(), "chat interaction")
    interaction = bot.repo.get_chat_interaction(resolved)
    if interaction is None:  # pragma: no cover
        raise NotFound(f"no chat interaction `{interaction_id}`")
    return interaction


def chat_summary(bot: BossBot) -> dict:
    """What the chatbot has cost and how well it has gone, per model.

    Over everything stored rather than a window: the log is capped at 500 rows,
    so "all of it" is already a recent period, and a window would put a second
    number on the page that has to be explained before it can be read.
    """
    stats = bot.repo.chat_interaction_stats()
    return {
        "models": stats,
        "count": sum(s["count"] for s in stats),
        "answered": sum(s["answered"] for s in stats),
        "failed": sum(s["failed"] for s in stats),
        "prompt_tokens": sum(s["prompt_tokens"] for s in stats),
        "completion_tokens": sum(s["completion_tokens"] for s in stats),
    }


def short_subject(subject: str | None) -> str | None:
    """A uuid subject as the eight characters the portal shows; anything else as is.

    An audit subject is a run, a card or a fixed timing -- but it is also a
    config key and a channel id, and ``short_id`` would happily cut those in
    half. Only a dashed uuid is shortened.
    """
    if not subject:
        return None
    return short_id(subject) if len(subject) == 36 and "-" in subject else subject


def audit_view(bot: BossBot, row: dict) -> dict:
    """One line of the audit trail: when, from where, who, what."""
    return {
        "id": row["id"],
        "at": to_iso(row["at"]),
        "local_time": row["at"].astimezone(bot.tz).strftime("%a %d %b %H:%M:%S"),
        "surface": row["surface"],
        "actor": row["actor"],
        "action": row["action"],
        "subject": row["subject"],
        "short_subject": short_subject(row["subject"]),
        "detail": row["detail"],
    }


def audit_log(bot: BossBot, limit: int = 200) -> list[dict]:
    """The most recent changes, newest first -- what the JSON API returns."""
    return [audit_view(bot, row) for row in bot.repo.list_audit(limit)]


# ---------------------------------------------------------------------------
# the table pages: one search box, one pager, six listings
# ---------------------------------------------------------------------------
#
# Every page that is a list of rows answers the same two questions -- "which
# rows" and "where in them am I" -- so they answer them the same way and the
# templates share one contract: ``rows``, ``q``, and the numbers a pager needs.
#
# Where the search happens differs, and deliberately. The three log tables are
# searched in SQL (:meth:`bot.db.Repo.list_audit` and friends), because their
# columns are the text being looked for and paging in SQL is the only way not
# to build two thousand views to show twenty. Reminders, members and fixed
# timings are searched here, over rows that have already been rendered, because
# what a reader searches for on those pages -- a boss's full name, a member's
# nickname, a channel's ``#name`` -- is not in the database at all: it comes
# from the boss table, the roster and the live guild.

#: Rows per page on the logs. Twenty fills a laptop window without the table
#: pane having to scroll at all; the pane's own scrollbar is a shock absorber
#: for a short window, not how the page is meant to be read.
PAGE_SIZE = 20

#: How far back the Reminders page looks. Unlike the logs this list is built by
#: joining every run to its reminders, so it is capped before it is filtered
#: rather than paged in SQL -- and a cap is honest here: nobody scrolls to a
#: ping from four months ago, they search for it.
REMINDER_SCAN = 1000


def _page_meta(total: int, page: int, per_page: int = PAGE_SIZE) -> dict:
    """Where in a result set we are, clamped to somewhere that exists.

    A ``?page=`` past the end is a stale bookmark, or a search that narrowed
    under somebody -- not an error worth a 404, so it lands on the last page
    there actually is.
    """
    pages = max(1, (total + per_page - 1) // per_page)
    current = min(max(int(page or 1), 1), pages)
    return {
        "page": current,
        "pages": pages,
        "total": total,
        "offset": (current - 1) * per_page,
        "per_page": per_page,
        "prev": current - 1 if current > 1 else None,
        "next": current + 1 if current < pages else None,
    }


def _listing(rows: list[dict], meta: dict, q: str) -> dict:
    return {"rows": rows, "q": q, **meta}


def _found(rows: list[dict], q: str) -> dict:
    """A search-only listing: everything that matched, and no pager.

    Same shape as a paged one so the templates need no branch -- one page of
    one, which is what "no paging" actually is.
    """
    return _listing(rows, _page_meta(len(rows), 1, max(len(rows), 1)), q)


def _matches(q: str, *fields: object) -> bool:
    """Does the query appear in any of these already-rendered strings?

    Case-insensitive substring, matching what the SQL side does, so the search
    box means one thing on all six pages.
    """
    term = (q or "").strip().casefold()
    if not term:
        return True
    return any(term in str(field).casefold() for field in fields if field is not None)


def audit_listing(bot: BossBot, page: int = 1, q: str = "") -> dict:
    meta = _page_meta(bot.repo.count_audit(q), page)
    rows = bot.repo.list_audit(limit=meta["per_page"], offset=meta["offset"], q=q)
    return _listing([audit_view(bot, row) for row in rows], meta, q)


def extractions_listing(bot: BossBot, page: int = 1, q: str = "") -> dict:
    meta = _page_meta(bot.repo.count_extractions(q), page)
    rows = bot.repo.recent_extractions(limit=meta["per_page"], offset=meta["offset"], q=q)
    return _listing([extraction_view(bot, row) for row in rows], meta, q)


def _named_ids(bot: BossBot, q: str) -> list[str]:
    """Members and channels whose *name* contains the query.

    The chat log stores both as ids and every page shows them as names, so a
    search box has to be answered against the name. Resolved here because this
    is the layer that knows what a name is: a channel's comes from the live
    guild and is not in the database at all, and a member's may be a nickname
    or one of the aliases the extractor matches on.
    """
    term = (q or "").strip().casefold()
    if not term:
        return []
    ids: list[str] = []
    for member in bot.repo.list_members(with_role=False):
        names = [member["display_name"], member["nickname"], *member["aliases"]]
        if any(term in str(name).casefold() for name in names if name):
            ids.append(str(member["user_id"]))
    for channel in bot.watched_text_channels():
        if term in f"#{getattr(channel, 'name', '')}".casefold():
            ids.append(str(channel.id))
    return ids


def chat_listing(bot: BossBot, page: int = 1, q: str = "") -> dict:
    ids = _named_ids(bot, q)
    meta = _page_meta(bot.repo.count_chat_interactions(q, ids), page)
    rows = bot.repo.recent_chat_interactions(
        limit=meta["per_page"], offset=meta["offset"], q=q, ids=ids
    )
    return _listing([chat_interaction_view(bot, row) for row in rows], meta, q)


def reminders_listing(bot: BossBot, page: int = 1, q: str = "", run_id: str | None = None) -> dict:
    """Queued and sent, both searched; only the sent half is paged.

    Queued is bounded by what has been materialised -- two boss weeks, so tens
    of rows -- and a pager over it would be a control that never has a second
    page. Sent grows for as long as the runs do, so that is the half that gets
    one.

    ``run_id`` is the narrower "just this run's pings", which the page offers no
    control for; it is here because ``/reminders?run_id=`` has always answered
    it and a URL that used to work should keep working.
    """
    rows = reminders(bot, run_id=run_id, limit=REMINDER_SCAN)
    kept = [
        row
        for row in rows
        if _matches(
            q,
            row["kind"],
            row["run_short_id"],
            " ".join(row["bosses"]),
            " ".join(row["party"]),
            row["local_fire_at"],
        )
    ]
    upcoming = sorted((r for r in kept if not r["sent_at"]), key=lambda r: r["fire_at"])
    sent = [r for r in kept if r["sent_at"]]
    meta = _page_meta(len(sent), page)
    listing = _listing(sent[meta["offset"] : meta["offset"] + meta["per_page"]], meta, q)
    listing["upcoming"] = upcoming
    return listing


def members_listing(bot: BossBot, q: str = "") -> dict:
    rows = [
        member
        for member in members(bot)
        if _matches(
            q,
            member["display_name"],
            member["nickname"],
            " ".join(member["aliases"]),
            member["user_id"],
        )
    ]
    return _found(rows, q)


def fixed_listing(bot: BossBot, q: str = "") -> dict:
    rows = [fixed_view(bot, f, with_grid=True) for f in bot.repo.list_fixed_runs()]
    kept = [
        row
        for row in rows
        if _matches(
            q,
            " ".join(row["bosses"]),
            " ".join(boss["full"] for boss in row["boss_detail"]),
            row["weekday_name"],
            row["time"],
            row["short_id"],
            " ".join(person["name"] for person in row["participants"]),
            row["channel_name"],
        )
    ]
    return _found(kept, q)


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
        # Who the ping is for, by the name the roster knows them by. The page
        # shows it and the search box matches on it: "which of Priya's pings
        # went out?" is the question this table is opened with.
        "party": [member_name(bot, uid) for uid in run["participants"]] if run else [],
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
# the week, as a board
# ---------------------------------------------------------------------------
#
# The rail drew the shape of a boss week in seven cells and then handed off to a
# list underneath it, which meant the shape and the runs were never on screen
# together: you looked at the pips, scrolled past them, and lost the week.
# On a desktop the seven cells grow into seven columns and become the page --
# the rail's job, done properly. A phone keeps the rail and the list, because
# seven columns on a 360px screen is seven columns nobody can read.


#: A day with nothing on it: wide enough for "Thu" stacked over "04" and for
#: nothing else. Six of these is the difference between a board that says "the
#: week is one busy Wednesday" and one that gives six sevenths of the canvas to
#: emptiness.
BOARD_SPINE = "3.5rem"

#: A day with runs. Never narrower than a card can be read at, and otherwise an
#: equal share of what the spines left behind -- equal because a day's *count*
#: is expressed down its column, not across it. A week busy enough that seven
#: minimums do not fit scrolls sideways rather than shrinking below legible.
#: Written without a space inside the ``minmax`` so the track list is a
#: whitespace-separated list of tracks and nothing else -- which is what CSS
#: reads it as, and what a reader (or a test) can split on.
BOARD_RUN_TRACK = "minmax(230px,1fr)"


def board_tracks(columns: Sequence[dict]) -> str:
    """The board's ``grid-template-columns``, sized to what each day holds.

    Built here rather than in the stylesheet because the stylesheet cannot
    count: which of the seven days have runs is a fact about this week, known
    at render time, and the alternative is measuring it in the browser.
    """
    return " ".join(BOARD_SPINE if not column["runs"] else BOARD_RUN_TRACK for column in columns)


def board_columns(bot: BossBot, week: str, runs: Sequence[dict]) -> list[dict]:
    """The week's runs in seven day columns, reset day first.

    Built from the *filtered* run views the page already has rather than from
    the database again, so the board narrows with the filter bar and cannot
    disagree with the list under it. The seven dates come from the week itself,
    so a day with nothing on it is still a column -- an empty Tuesday is a fact
    about the week, not a row to omit.
    """
    local_ws = week_for(bot, week).astimezone(bot.tz)
    today = utcnow().astimezone(bot.tz).date()
    by_date: dict[str, list[dict]] = {}
    for run in runs:
        by_date.setdefault(run["local_date"], []).append(run)

    columns = []
    for offset in range(7):
        date = (local_ws.replace(tzinfo=None) + timedelta(days=offset)).date()
        key = date.strftime("%Y-%m-%d")
        columns.append(
            {
                "date": key,
                "weekday": WEEKDAY_NAMES[date.weekday()],
                "day": date.day,
                "month": date.strftime("%b"),
                "is_today": date == today,
                "is_reset": offset == 0,
                "runs": sorted(by_date.get(key, []), key=lambda run: run["datetime"]),
            }
        )
    for column in columns:
        column["empty"] = not column["runs"]
    return columns


def countdown(target: datetime, now: datetime | None = None) -> str:
    """How long until something, in the coarsest unit that is still useful.

    Rendered on the server and left alone: it is read once, at a glance, on a
    page somebody opens to find out whether they have time to eat first. A
    ticking clock would be a second thing on the page that can be wrong.
    """
    seconds = int((target - (now or utcnow())).total_seconds())
    if seconds <= 0:
        return "now"
    days, rest = divmod(seconds, 86400)
    hours, rest = divmod(rest, 3600)
    minutes = rest // 60
    if days:
        return f"in {days}d {hours}h"
    if hours:
        return f"in {hours}h {minutes:02d}m"
    return f"in {minutes} min"


def week_now(bot: BossBot, runs: Sequence[dict]) -> dict:
    """The four things worth knowing before reading the board at all.

    What is next, who has not answered, what is waiting in the inbox, and
    whether the model is free -- the questions the page was opened with, in the
    order they get asked. All server-rendered: none of them changes fast enough
    to be worth a socket, and the page is a glance rather than a dashboard.
    """
    now = utcnow()
    ahead = sorted(
        (run for run in runs if run["status"] in LIVE_STATUSES and from_iso(run["datetime"]) > now),
        key=lambda run: run["datetime"],
    )
    nxt = ahead[0] if ahead else None
    model = limits(bot)["model"]
    return {
        "next": (
            {
                "id": nxt["id"],
                "short_id": nxt["short_id"],
                "when": f"{nxt['local_day']} {nxt['local_time']}",
                "countdown": countdown(from_iso(nxt["datetime"]), now),
                "bosses": formatting.format_bosses(nxt["bosses"]),
                "yes": nxt["yes"],
                "of": len(nxt["participants"]),
            }
            if nxt
            else None
        ),
        # Only the runs still ahead: nobody can answer for a night that has been.
        "unanswered": sum(run["unanswered"] for run in ahead),
        "pending": len(bot.repo.list_amendments(status="proposed")),
        "model_busy": model["busy"],
        "model_holder": model["holder"],
    }


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
    _audit(
        bot,
        "fixed_add",
        fixed_id,
        f"added the weekly {formatting.format_bosses(boss_list)} on "
        f"{WEEKDAY_NAMES[weekday]} {hhmm} for {len(people)} member(s)",
    )
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
    _audit(
        bot,
        "fixed_edit",
        fixed["id"],
        f"changed {', '.join(sorted(fields))} on the weekly "
        f"{formatting.format_bosses(updated['bosses'])} "
        f"({WEEKDAY_NAMES[updated['weekday']]} {updated['time']})",
    )
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
    cancelled = retire_fixed_run(
        bot.repo,
        fixed["id"],
        [week_for(bot, which) for which in ("this", "next")],
        bot.tz,
        bot.ping_time,
        bot.countdowns,
    )
    _audit(
        bot,
        "fixed_remove",
        fixed["id"],
        f"removed the weekly {formatting.format_bosses(fixed['bosses'])} "
        f"({WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']}); "
        f"{cancelled} upcoming run(s) cancelled",
    )
    who = audience(bot.repo, fixed["participants"], "fixed")
    await _announce(
        bot, formatting.fixed_notice(fixed, "removed", who), who.mentioned, fixed["channel_id"]
    )
    return {"id": fixed["id"], "short_id": short_id(fixed["id"]), "cancelled_runs": cancelled}


# ---------------------------------------------------------------------------
# run mutations
# ---------------------------------------------------------------------------


#: How far ahead a parsed date may land before it is treated as a misreading.
#: ``dateparser`` reads a bare ``2300`` as the *year* 2300, which reached a
#: proposal card three centuries out; nothing this guild schedules is more than
#: a boss week or two away, so anything past this is a parse that went wrong
#: rather than a plan.
MAX_HORIZON = timedelta(days=400)


def _day_words() -> list[tuple[str, str]]:
    """``[(what people type, what dateparser understands)]``, longest first.

    Built from the extractor's own day vocabulary
    (:mod:`bot.extract.resolve`) rather than a second table, so "tmr" means
    tomorrow in `/amend` and in chat for the same reason and at the same time.
    Deliberately *only* the day words: this is a parser for text somebody typed
    into a command, not the extractor's fuzzy keyword gate.
    """
    pairs = [
        *((word, "today") for word in resolve.TODAY_WORDS | resolve.SOON_WORDS),
        *((word, "tomorrow") for word in resolve.TOMORROW_WORDS),
        *((word, "yesterday") for word in resolve.YESTERDAY_WORDS),
    ]
    # Longest first so "tmr night" is replaced before "tmr" can be.
    return sorted(pairs, key=lambda pair: len(pair[0]), reverse=True)


_DAY_WORDS = _day_words()
_DAY_WORD_RE = re.compile(
    r"\b(" + "|".join(re.escape(word) for word, _ in _DAY_WORDS) + r")\b", re.IGNORECASE
)
_REPLACEMENTS = {word.lower(): standard for word, standard in _DAY_WORDS}

#: "tomorrow night 9pm" -- the night is already in the 9pm, and dateparser
#: returns nothing at all for the pair.
_NIGHT_AFTER_DAY_RE = re.compile(
    r"\b(today|tomorrow|yesterday)\s+(?:night|nite|evening)\b", re.IGNORECASE
)

#: A clock time and nothing else: ``2300``, ``930pm``, ``9:30 pm``, ``9+pm``,
#: ``1030~11+pm``. Tight on purpose -- ``2026-09-02 21:30`` must not match, or a
#: real date would be read as a time.
_CLOCK_ONLY_RE = re.compile(
    r"^\s*\d{1,2}[:.]?(?:\d{2})?\s*\+?\s*"
    r"(?:[~\-]|to|till|until)?\s*(?:\d{1,2}[:.]?(?:\d{2})?\s*\+?\s*)?"
    r"(?:a\.?m\.?|p\.?m\.?)?\s*$",
    re.IGNORECASE,
)


#: A day is already named, so a loose 3-4 digit number beside it is a clock time.
_HAS_DAY_RE = re.compile(
    r"\b(?:today|tomorrow|yesterday|"
    + "|".join(re.escape(word) for word in resolve.WEEKDAY_ALIASES)
    + r")\b",
    re.IGNORECASE,
)
#: ``2300`` / ``930``, not the ``2026`` of an ISO date and not a ``21:30`` half.
_COMPACT_TIME_RE = re.compile(r"(?<![\d:./-])(\d{3,4})(?![\d:./-])")


def _spell_out_compact_times(text: str) -> str:
    """``"tomorrow 2300"`` -> ``"tomorrow 23:00"``.

    dateparser reads ``tomorrow 2300`` as tomorrow at *the current time* -- it
    takes the day and quietly drops the digits -- which is worse than failing,
    because it produces a plausible wrong answer. Only done when a day is
    already named, so ``sep 2026`` is still a year.
    """
    if not _HAS_DAY_RE.search(text):
        return text

    def spell(match: re.Match[str]) -> str:
        clock = resolve.parse_clock(match.group(1))
        return match.group(1) if clock is None else clock[0].strftime("%H:%M")

    return _COMPACT_TIME_RE.sub(spell, text)


def normalise_when(text: str) -> str:
    """Rewrite the guild's day words into ones ``dateparser`` actually knows.

    Measured, not guessed: ``tonight 23:00``, ``tonight at 11pm``, ``tmr 2300``,
    ``tmr 9pm``, ``tomorrow night 9pm`` and ``ltr 9pm`` all return ``None`` from
    dateparser, while ``today 23:00`` parses fine. The words are the whole
    problem, so they are replaced before it ever sees them.
    """
    swapped = _DAY_WORD_RE.sub(
        lambda match: _REPLACEMENTS[match.group(1).lower()],
        text or "",
    )
    return _spell_out_compact_times(_NIGHT_AFTER_DAY_RE.sub(r"\1", swapped)).strip()


def _bare_clock(bot: BossBot, text: str, now: datetime) -> datetime | None:
    """``"2300"`` said in the evening -> 23:00 tonight, or tomorrow if it has gone.

    dateparser reads a bare ``2300`` as a year and ``930pm`` as nothing at all.
    :func:`bot.extract.resolve.parse_clock` already reads every form of these
    that this guild writes, so it is reused rather than re-taught here.
    """
    if not _CLOCK_ONLY_RE.match(text):
        return None
    clock = resolve.parse_clock(text)
    if clock is None:
        return None
    local = now.astimezone(bot.tz)
    at = datetime.combine(local.date(), clock[0], tzinfo=bot.tz)
    # "2300" at 23:30 means tomorrow: nobody schedules a run half an hour ago.
    return at + timedelta(days=1) if at <= local else at


def _resolve_when(text: str, now: datetime, tz: Any) -> datetime | None:
    """Parse ``text`` via the extractor's day/time resolver.

    ``dateparser`` returns ``None`` for ``"next tuesday 22:30"`` and similar
    ``next <weekday>`` forms. The extractor's :func:`bot.extract.resolve.resolve`
    already handles them through ``_NEXT_RE`` and ``_WEEKDAY_ALIASES``; rather
    than re-teach dateparser, split off the clock token (always the last
    whitespace-separated word) and pass the rest as ``day_ref``.
    """
    words = text.split()
    if len(words) < 2:
        return None
    time_part = words[-1]
    if resolve.parse_clock(time_part) is None:
        return None
    day_part = " ".join(words[:-1])
    return resolve.resolve(day_part, time_part, now, tz).at


def parse_when(bot: BossBot, text: str) -> datetime:
    """``"wed 21:30"`` / ``"tomorrow 9:45pm"`` -> an instant, exactly as ``/amend``.

    Shared by `/amend`, the portal, `bossctl` and the chatbot, so the guild's own
    shorthand has to work here or it works nowhere.
    """
    now = utcnow()
    cleaned = normalise_when(text)
    bare = _bare_clock(bot, cleaned, now)
    if bare is not None:
        return bare
    parsed = dateparser.parse(
        cleaned,
        settings={
            "RELATIVE_BASE": local_naive(now, bot.tz),
            "PREFER_DATES_FROM": "future",
            "TIMEZONE": bot.settings.tz,
            "RETURN_AS_TIMEZONE_AWARE": True,
        },
    )
    if parsed is None:
        # dateparser misses ``next <weekday> HH:MM`` -- fall back to the
        # extractor's own resolver, which handles it through _NEXT_RE.
        parsed = _resolve_when(cleaned, now, bot.tz)
    if parsed is None:
        raise BadRequest(
            f"couldn't read `{text}` as a date - try `wed 21:30` or `2026-09-02 21:30`"
        )
    if parsed > now + MAX_HORIZON:
        # A misreading, not a plan: this is how a bare `2300` became the year 2300.
        raise BadRequest(
            f"couldn't read `{text}` as a date - that lands in "
            f"{parsed.astimezone(bot.tz):%Y}. Try `wed 21:30` or `2026-09-02 21:30`"
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
    _audit(
        bot,
        "amend",
        run["id"],
        f"moved {_bosses_of(updated)} from "
        f"{formatting.local_day(old_at, bot.tz)} {formatting.local_time(old_at, bot.tz)} to "
        f"{formatting.local_day(parsed, bot.tz)} {formatting.local_time(parsed, bot.tz)}",
    )
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
    # `cancel`/`otot`/`restore` all land here, so the trail names the transition
    # rather than whichever shortcut was used to ask for it.
    _audit(
        bot,
        "cancel" if status == "cancelled" else "status",
        run["id"],
        f"{_bosses_of(fresh)} on {formatting.local_day(fresh['datetime'], bot.tz)}: "
        f"{previous} -> {status}",
    )
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
    changes = [f"-{member_name(bot, uid)}" for uid in leaving]
    changes += [f"+{member_name(bot, uid)}" for uid in joining]
    _audit(
        bot,
        "swap",
        run["id"],
        f"{_bosses_of(fresh)} on {formatting.local_day(fresh['datetime'], bot.tz)} "
        f"this week: {' '.join(changes)}",
    )

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
    _audit(
        bot,
        "rsvp",
        run["id"],
        f"{'cleared' if answer == 'clear' else answer} for {member_name(bot, uid)} on "
        f"{_bosses_of(fresh)} ({formatting.local_day(fresh['datetime'], bot.tz)})",
    )
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

    _audit(
        bot,
        "approve",
        amendment["id"],
        f"applied the {result.kind} on card {short_id(amendment['id'])}, "
        f"credited to {member_name(bot, actor)}",
    )
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
    _audit(
        bot,
        "reject",
        amendment["id"],
        f"rejected the {amendment['kind']} on card {short_id(amendment['id'])}",
    )
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
    _audit(
        bot,
        "member",
        str(user_id),
        f"{member_name(bot, user_id)} is now on `{level}` @mentions",
    )
    return member_view(bot, bot.repo.get_member(user_id), _run_counts(bot))


def set_nick(bot: BossBot, user_id: int | str, alias: str) -> dict:
    alias = (alias or "").strip()
    if not alias:
        raise BadRequest("alias can't be empty")
    member = bot.repo.get_member(user_id)
    if member is None:
        raise NotFound(f"no member {user_id} - the roster syncs from the bossing role")
    aliases = bot.repo.add_alias(user_id, alias)
    _audit(bot, "nick", str(user_id), f"{member_name(bot, user_id)} is also known as `{alias}`")
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


def reset_user_limit(bot: BossBot, user_id: int | str) -> dict:
    """Give one member their answers back, and forget the notice they were sent.

    Individual windows only, deliberately. There is no way here to clear the
    guild's pool or everybody at once: the pool is a fact about what the host
    can produce in an hour rather than about a person, and a "reset all" button
    is the one somebody presses instead of asking why the bot is busy.

    The roster is *not* consulted first, unlike :func:`set_nick` and
    :func:`update_member`. Holding the chat role does not require holding the
    bossing role, so somebody can be rate limited while not being on the roster
    at all -- and the window shown on the Limits page has to be clearable from
    the button next to it. Clearing a key with no window is harmless and says
    so; the name falls back to ``user 1002`` exactly as it does on the page.
    """
    bot.chat.forget_limit(user_id)
    events.notify()
    name = member_name(bot, user_id)
    _audit(bot, "limits", str(user_id), f"cleared {name}'s answer window")
    return {"user_id": str(user_id), "name": name}


def set_user_limit(bot: BossBot, user_id: int | str, count: int, window_s: float) -> dict:
    """Give one member their own allowance instead of the guild default.

    Stored, so it survives a restart, and pushed into the live limiter at once
    (:meth:`bot.chat.agent.ChatPilot.apply_limits`) -- an override that only took
    effect after a redeploy would be useless at the moment somebody asks for it.

    Not restricted to the roster, for the reason :func:`reset_user_limit` gives:
    the chat role and the bossing role are different things, and it must be
    possible to raise the allowance of somebody who has not asked yet today.
    """
    count = _whole_number(count, "count", minimum=1)
    window_s = _seconds(window_s, "window_s")
    bot.repo.set_rate_limit(user_id, count, window_s)
    bot.chat.apply_limits()
    events.notify()
    name = member_name(bot, user_id)
    _audit(
        bot,
        "limits",
        str(user_id),
        f"{name}'s allowance is now {count} answer(s) per {window_s:g}s",
    )
    return {"user_id": str(user_id), "name": name, "count": count, "window_s": window_s}


def clear_user_limit(bot: BossBot, user_id: int | str) -> dict:
    """Put one member back on the guild default, keeping their spent window.

    Idempotent: clearing an allowance nobody had is not an error, because the
    end state the caller asked for -- "this member is on the default" -- is the
    one they get either way.
    """
    had = bot.repo.clear_rate_limit(user_id)
    bot.chat.apply_limits()
    events.notify()
    name = member_name(bot, user_id)
    if had:
        _audit(bot, "limits", str(user_id), f"{name} is back on the guild's default allowance")
    return {"user_id": str(user_id), "name": name}


def _is_staff(bot: BossBot, member: Any) -> bool:
    """The same "who runs this bot" rule the chat gate applies to a message.

    Read from the live member object for the same reason
    :meth:`bot.chat.agent.ChatPilot._is_admin` does: staff are exempt from every
    budget, so a page offering to raise their allowance would be offering to
    change a number nothing reads.
    """
    permissions = getattr(member, "guild_permissions", None)
    guild = getattr(member, "guild", None)
    return is_bot_admin(
        bool(getattr(permissions, "administrator", False)),
        guild is not None and getattr(guild, "owner_id", None) == getattr(member, "id", None),
        [getattr(role, "id", role) for role in getattr(member, "roles", None) or ()],
        bot.settings.admin_role_id,
    )


def pilot_roster(bot: BossBot) -> list[dict]:
    """Everybody holding ``CHAT_PILOT_ROLE_ID``, and where each of them stands.

    A **live read** of Discord's role member cache -- the same source
    :func:`bot.util.roster_rows` uses for the bossing role, available because
    the members intent is already on. Nothing is stored: the answer to "who may
    talk to the bot" is the role, and a copy of it in SQLite would be a second
    answer that goes stale the moment somebody is given the role.

    Empty whenever it cannot be known -- no role configured, no guild (the bot
    is not connected, or a test never built one), or a role id that resolves to
    nothing. The page says so rather than showing an empty table as though the
    role had no holders.

    Bot accounts are dropped, exactly as ``roster_rows`` drops them: another bot
    holding the role is not somebody whose allowance anybody needs to tune.
    """
    role_id = bot.settings.chat_pilot_role_id
    guild = bot.get_guild(bot.settings.guild_id) if role_id is not None else None
    role = guild.get_role(int(role_id)) if guild is not None else None
    if role is None:
        return []

    limiter = bot.chat.limiter
    spent = limiter.snapshot()
    rows = []
    for member in getattr(role, "members", None) or ():
        if getattr(member, "bot", False):
            continue
        user_id = str(member.id)
        count, window = limiter.limit_for(user_id)
        used = spent.get(user_id, 0)
        rows.append(
            {
                "user_id": user_id,
                # The roster's name when it knows them, so one person reads the
                # same on this table and the one above it; the live display name
                # otherwise, since holding the chat role does not put anybody on
                # the bossing roster.
                "name": member_name(bot, user_id)
                if bot.repo.get_member(user_id)
                else getattr(member, "display_name", user_id),
                "staff": _is_staff(bot, member),
                "count": count,
                "window_s": window,
                "overridden": user_id in limiter.overrides(),
                "used": used,
                "remaining": max(count - used, 0),
                "has_window": used > 0,
            }
        )
    rows.sort(key=lambda row: (row["staff"], row["name"].casefold()))
    return rows


def _window_view(bot: BossBot, limiter: Any, user_id: str, used: int) -> dict:
    """One open window, against the allowance that member is actually on."""
    count, window = limiter.limit_for(user_id)
    return {
        "user_id": str(user_id),
        "name": member_name(bot, user_id),
        "used": used,
        "remaining": max(count - used, 0),
        "count": count,
        "window_s": window,
        #: So the page can mark the row rather than leaving a reader to spot
        #: that this one number differs from the heading.
        "overridden": str(user_id) in limiter.overrides(),
    }


def _pool_view(limiter: Any, key: str) -> dict:
    """One :class:`bot.chat.ratelimit.RateLimiter` window as used-of-total."""
    remaining = limiter.remaining(key)
    return {
        "count": limiter.count,
        "window_s": limiter.window,
        "used": max(limiter.count - remaining, 0),
        "remaining": remaining,
    }


def limits(bot: BossBot) -> dict:
    """What the host is doing right now, and how much of it is left.

    The one page that answers "why is the bot slow?" without reading the log.
    Everything here is *live* state rather than anything stored -- the lock, two
    sliding windows and a queue -- so it is read fresh on every request and there
    is nothing to keep in step with the database.

    Read-only by construction: the limiter is asked with
    :meth:`bot.chat.ratelimit.RateLimiter.remaining` and
    :meth:`~bot.chat.ratelimit.RateLimiter.snapshot`, never ``allow``, so opening
    the page cannot spend anybody's allowance -- including the pool it is
    reporting on.
    """
    from ..chat.gate import GLOBAL_KEY
    from ..modellock import EXTRACTOR, holder

    pilot = bot.chat
    model = holder()
    rescans = bot.rescans
    current = rescans.current
    per_user = pilot.limiter
    return {
        "model": model,
        "global_pool": _pool_view(pilot.global_limiter, GLOBAL_KEY),
        "per_user": {
            # The guild default. Each row below carries the allowance that
            # member is actually on, which is not always this one.
            "count": per_user.count,
            "window_s": per_user.window,
            # Only members mid-window, so the list is what is happening rather
            # than everybody who has ever asked.
            "windows": [
                _window_view(bot, per_user, user_id, used)
                for user_id, used in sorted(
                    per_user.snapshot().items(), key=lambda pair: (-pair[1], pair[0])
                )
            ],
            # Every member with their own allowance, whether or not they have
            # asked anything: the page has to be able to show -- and clear -- an
            # override belonging to somebody who is not mid-window.
            "overrides": [
                {
                    "user_id": str(user_id),
                    "name": member_name(bot, user_id),
                    "count": count,
                    "window_s": window,
                }
                for user_id, (count, window) in sorted(per_user.overrides().items())
            ],
        },
        # Who may talk to the bot at all, read live from the role. The page's
        # reason to exist is tuning these people's limits, and hunting for a
        # snowflake to paste into a form is what it replaces.
        "pilots": pilot_roster(bot),
        "jobs": {
            "answering": [
                {"channel_id": cid, "channel_name": channel_name(bot, cid) or f"channel {cid}"}
                for cid in pilot.answering()
            ],
            # Falls out of the holder label: an extraction is simply what has the
            # model, and a rescan's extractions are the same thing seen from the
            # other end -- which is why the queue below is next to it.
            "extracting": model["holder"] == EXTRACTOR,
            "rescan": {
                "worker_running": rescans.running,
                "queued": rescans.queued,
                "job": current.short_id if current is not None else None,
                "channel": current.current if current is not None else None,
            },
        },
    }


def get_config(bot: BossBot) -> dict:
    # Cached on the pilot after the first read, so asking here costs a page load
    # nothing and reports exactly what the bot is answering with.
    persona_source = bot.chat.persona_source()
    configured_role_plugins = behaviour_plugins.decode(
        bot.repo.get_config(behaviour_plugins.CONFIG_KEY, "[]")
    )
    return {
        "day_of_ping_time": bot.ping_time.strftime("%H:%M"),
        "countdown_minutes": ",".join(str(m) for m in bot.countdowns),
        "paused": bot.paused,
        "extract_enabled": bot.extract_enabled,
        "quiet_mode": bot.quiet_mode,
        "chat_mode": bot.chat_mode,
        # Whether the chatbot *could* answer at all: a chat role and at least
        # one chat channel. `chat_mode` on top of an unconfigured pilot answers
        # nobody, and the page needs to be able to say so.
        "chat_configured": bot.settings.chat_pilot_configured,
        # The four capacity numbers, live from the config table. Editable here
        # rather than only in `.env` because they are what an operator reaches
        # for while the guild is busy.
        "chat_pilot_rate_count": bot.chat_rate_count,
        "chat_pilot_rate_window_s": bot.chat_rate_window_s,
        "chat_pilot_global_rate_count": bot.chat_pool_count,
        "chat_pilot_global_rate_window_s": bot.chat_pool_window_s,
        "chat_channels": [str(c) for c in bot.settings.chat_pilot_channel_id_list],
        "chat_categories": [str(c) for c in bot.settings.chat_pilot_category_id_list],
        "chat_model": bot.settings.chat_pilot_model,
        # Which persona file is actually loaded, and whether it is the tracked
        # template. A deploy answering in the placeholder voice is a
        # misconfiguration, and it used to be visible only in a startup WARNING.
        "persona_file": persona_source.name,
        "persona_fallback": persona_source.fell_back,
        # The choice, and what there is to choose from. `persona` is what the
        # setting says; `persona_file` above is what the bot actually read, and
        # the two differ exactly when the chosen file has gone missing.
        "persona": bot.persona_name,
        "persona_choices": bot.persona_choices(),
        "chat_role_plugins": [item.as_dict() for item in configured_role_plugins],
        "behaviour_plugins": [item.as_dict() for item in behaviour_plugins.list_plugins()],
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
    before = bot.repo.get_config(key)
    if key == "day_of_ping_time":
        try:
            stored = parse_hhmm(str(value)).strftime("%H:%M")
        except ValueError as exc:
            raise BadRequest(str(exc)) from None
        if stored == before:
            return get_config(bot)
        now = utcnow()
        bot.repo.set_config(key, stored)
        reconcile_day_of(bot.repo, bot.tz, bot.ping_time, now=now)
    elif key == "countdown_minutes":
        minutes = [p.strip() for p in str(value).replace(";", ",").split(",") if p.strip()]
        try:
            parsed = [int(m) for m in minutes]
        except ValueError:
            raise BadRequest("countdown_minutes must be whole minutes, e.g. `60,15`") from None
        if not parsed or any(m <= 0 for m in parsed):
            raise BadRequest("countdown_minutes must be positive whole minutes, e.g. `60,15`")
        stored = ",".join(str(m) for m in sorted(set(parsed), reverse=True))
        if stored == before:
            return get_config(bot)
        now = utcnow()
        bot.repo.set_config(key, stored)
        for run in bot.repo.list_runs(statuses=LIVE_STATUSES):
            ensure_reminders(bot.repo, run, bot.tz, bot.ping_time, bot.countdowns, now=now)
    elif key == "persona":
        stored = _persona_choice(bot, value)
        bot.repo.set_config(key, stored)
        # The pilot reads the document once and keeps it. Without this the new
        # voice would wait for a restart, which is the whole thing this setting
        # exists to avoid: the next question is answered in it.
        bot.chat.reload_persona()
    elif key == behaviour_plugins.CONFIG_KEY:
        try:
            stored = behaviour_plugins.encode(value)
            missing = [
                item.plugin
                for item in behaviour_plugins.decode(stored)
                if behaviour_plugins.read(item.plugin) is None
            ]
        except (TypeError, ValueError) as exc:
            raise BadRequest(str(exc)) from None
        if missing:
            raise BadRequest(f"unknown behaviour plugin(s): {', '.join(missing)}")
        bot.repo.set_config(key, stored)
    elif key in COUNT_KEYS:
        stored = str(_whole_number(value, key, minimum=1))
        bot.repo.set_config(key, stored)
        # The limiters live for the whole process, so a new number means nothing
        # until they are told. Windows already open are reinterpreted under it.
        bot.chat.apply_limits()
        events.notify()
    elif key in WINDOW_KEYS:
        stored = str(_seconds(value, key))
        bot.repo.set_config(key, stored)
        bot.chat.apply_limits()
        events.notify()
    else:
        stored = _as_flag(value)
        bot.repo.set_config(key, stored)
    if key == behaviour_plugins.CONFIG_KEY:
        before_count = len(behaviour_plugins.decode(before))
        after_count = len(behaviour_plugins.decode(stored))
        detail = f"{key}: {before_count} assignment(s) -> {after_count} assignment(s)"
    else:
        detail = f"{key}: {before if before is not None else 'unset'} -> {stored}"
    _audit(bot, "config", key, detail)
    return get_config(bot)


def set_role_plugin(bot: BossBot, role_id: Any, plugin: Any) -> dict:
    """Add or update one role-to-plugin assignment while preserving its order."""
    try:
        candidate = behaviour_plugins.validate([{"role_id": role_id, "plugin": plugin}])[0]
    except (IndexError, TypeError, ValueError) as exc:
        raise BadRequest(str(exc) or "role id and plugin are required") from None
    configured = behaviour_plugins.decode(bot.repo.get_config(behaviour_plugins.CONFIG_KEY, "[]"))
    updated = [candidate if item.role_id == candidate.role_id else item for item in configured]
    if all(item.role_id != candidate.role_id for item in configured):
        updated.append(candidate)
    return set_config(bot, behaviour_plugins.CONFIG_KEY, [item.as_dict() for item in updated])


def set_behaviour_plugin(bot: BossBot, name: Any, instructions: Any) -> dict:
    """Create or edit one persisted behaviour plugin."""
    before = behaviour_plugins.read(str(name or ""))
    try:
        saved = behaviour_plugins.write(name, instructions)
    except (TypeError, ValueError) as exc:
        raise BadRequest(str(exc)) from None
    action = "updated" if before is not None else "created"
    _audit(bot, "config", behaviour_plugins.CONFIG_KEY, f"plugin `{saved.name}` {action}")
    return get_config(bot)


def delete_behaviour_plugin(bot: BossBot, name: Any) -> dict:
    """Delete an unused behaviour plugin from the persona bind mount."""
    try:
        safe = behaviour_plugins.plugin_name(name)
    except (TypeError, ValueError) as exc:
        raise BadRequest(str(exc)) from None
    configured = behaviour_plugins.decode(bot.repo.get_config(behaviour_plugins.CONFIG_KEY, "[]"))
    used_by = [item.role_id for item in configured if item.plugin == safe]
    if used_by:
        raise BadRequest(f"plugin `{safe}` is still used by role(s): {', '.join(used_by)}")
    try:
        behaviour_plugins.delete(safe)
    except ValueError as exc:
        raise NotFound(str(exc)) from None
    _audit(bot, "config", behaviour_plugins.CONFIG_KEY, f"plugin `{safe}` deleted")
    return get_config(bot)


def delete_role_plugin(bot: BossBot, role_id: Any) -> dict:
    """Remove one role-to-plugin assignment from the portal-managed list."""
    try:
        wanted = behaviour_plugins.validate([{"role_id": role_id, "plugin": "validate"}])[0].role_id
    except (IndexError, TypeError, ValueError) as exc:
        raise BadRequest(str(exc) or "a valid role id is required") from None
    configured = behaviour_plugins.decode(bot.repo.get_config(behaviour_plugins.CONFIG_KEY, "[]"))
    if all(item.role_id != wanted for item in configured):
        raise NotFound(f"no behaviour plugin is assigned to role {wanted}")
    remaining = [item.as_dict() for item in configured if item.role_id != wanted]
    return set_config(bot, behaviour_plugins.CONFIG_KEY, remaining)


def _persona_choice(bot: BossBot, value: Any) -> str:
    """One of the files actually in ``personas/``, by name.

    Validated by *membership* rather than by sanitising. The submitted string is
    compared against the real directory listing and is only ever joined to a
    path after it has matched one, so a separator, an absolute path and `..` are
    not things to strip -- they are simply not names in that list. The listing
    is read per call because the directory is a read-only bind mount that a
    person drops files into between page loads.

    Only filenames appear in the refusal. A persona's *contents* are the private
    half of this feature and never leave the file.
    """
    wanted = str(value or "").strip()
    choices = bot.persona_choices()
    if wanted in choices:
        return wanted
    offered = ", ".join(f"`{name}`" for name in choices) or "none -- personas/ is empty"
    raise BadRequest(f"`{wanted}` is not a persona in personas/ - one of: {offered}")


def _whole_number(value: Any, label: str, minimum: int = 1) -> int:
    try:
        parsed = int(str(value).strip())
    except (TypeError, ValueError):
        raise BadRequest(f"{label} must be a whole number, e.g. `4`") from None
    if parsed < minimum:
        raise BadRequest(f"{label} must be at least {minimum}")
    return parsed


def _seconds(value: Any, label: str) -> float:
    try:
        parsed = float(str(value).strip())
    except (TypeError, ValueError):
        raise BadRequest(f"{label} must be a number of seconds, e.g. `300`") from None
    if parsed <= 0:
        raise BadRequest(f"{label} must be more than zero seconds")
    return parsed


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
    _audit(
        bot,
        "digest",
        str(cid) if cid else None,
        f"posted the {week}-week digest in {channel_name(bot, cid) or f'channel {cid}'}",
    )
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
    _audit(
        bot,
        "rescan",
        job.id,
        f"queued a {window} re-read of {len(targets)} channel(s) from {source}",
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
    _audit(bot, "rescan_stop", job_id, "asked a running rescan to stop after this channel")
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
    _audit(bot, "ping", run["id"], f"posted a 🧪 TEST {kind} card for {_bosses_of(run)}")
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
    "audit_log",
    "audit_view",
    "boss_grid",
    "boss_view",
    "bosses_in_use",
    "monogram",
    "portrait_url",
    "entry_art_url",
    "run_entry_art",
    "approve",
    "cancel_run",
    "channel_is_watched",
    "chat_interaction_view",
    "chat_interactions",
    "chat_summary",
    "created_cards",
    "create_fixed",
    "debug_ping",
    "delete_fixed",
    "export_messages",
    "extraction_view",
    "fixed_view",
    "get_config",
    "limits",
    "pilot_roster",
    "clear_user_limit",
    "reset_user_limit",
    "set_user_limit",
    "load_amendment",
    "load_chat_interaction",
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
    "short_subject",
    "set_config",
    "set_nick",
    "set_rsvp",
    "week_for",
    "week_rail",
]
