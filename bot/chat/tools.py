"""The closed set of things the chatbot can do, and the dispatcher behind it.

Two rules hold this module together.

**Everything the model says is untrusted input.**  A tool call is a string the
model produced from a message a member wrote, so every argument is re-validated
here against the service layer -- run ids through :func:`bot.api.service.load_run`,
times through :func:`bot.api.service.parse_when` -- exactly as if it had arrived
over HTTP.  Nothing is passed through on the model's say-so.

**No write reaches the schedule.**  The five ``propose_*`` tools do not change
anything: they create a ``proposed`` amendment row and post the same ✅/❌ card
the extractor posts, through the same :meth:`bot.extract.pipeline.Pipeline.apply_plan`
call, and a human with the right to confirm it has to react. The chatbot cannot
approve, reject, edit or expire a card, and no other service function is
reachable from here -- a name that is not in :data:`TOOLS` returns an error
string rather than reaching :mod:`bot.api.service` at all.
"""

from __future__ import annotations

import json
import logging
import re
import time
from collections.abc import Sequence
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any

from ..api import service
from ..api.errors import BadRequest, NotFound
from ..extract.commit import FIX_REMOVE
from ..extract.pipeline import Planned
from ..extract.resolve import WEEKDAY_ALIASES, Resolved
from ..extract.schema import Amendment
from ..ids import short_id
from ..timeutil import utcnow
from ..util import resolve_participant_text
from ..weeks import WEEKDAY_NAMES, week_start

log = logging.getLogger(__name__)

#: How many runs a schedule answer lists before it is truncated. A boss week is
#: ten-ish runs; a model handed fifty lines starts summarising them wrongly.
MAX_RUNS = 20

#: What the model is told when it asks for something that is not a tool. Phrased
#: as an instruction because a bare "error" makes a small model retry the same
#: call; naming the real tools makes it pick one.
UNKNOWN_TOOL = (
    "There is no tool called {name}. The tools you have are: {known}. "
    "Use one of those, or answer from what you already know."
)

__all__ = [
    "FAILED",
    "MAX_RUNS",
    "REFUSED",
    "TOOLS",
    "UNKNOWN",
    "ToolContext",
    "ToolError",
    "ToolOutcome",
    "dispatch",
    "resolve_fixed",
    "resolve_run",
    "run",
    "tool_names",
]


class ToolError(Exception):
    """A refusal the model is allowed to read, and should say something about."""


@dataclass
class ToolContext:
    """Who is asking, from where -- everything a write needs for provenance.

    ``author_id`` is the Discord id of the person who wrote the triggering
    message, taken from the message rather than from anything the model said. It
    is what makes "RSVP for somebody else" impossible rather than merely
    discouraged.
    """

    bot: Any
    author_id: str
    channel_id: str
    message_id: str
    #: Amendment ids this turn created, so the agent can report accurately.
    created: list[str] = None  # type: ignore[assignment]

    def __post_init__(self) -> None:
        if self.created is None:
            self.created = []


# ---------------------------------------------------------------------------
# the schemas handed to ollama
# ---------------------------------------------------------------------------


def _tool(name: str, description: str, properties: dict, required: list[str]) -> dict:
    return {
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
        },
    }


_RUN_QUERY = {
    "type": "string",
    "description": (
        "Which run: its short id from an earlier tool result, or a boss and a day "
        "like 'hstar wednesday' or 'kalos tonight'."
    ),
}

TOOLS: list[dict] = [
    _tool(
        "get_schedule",
        "Runs for a boss week: day, time, bosses, status and how many people have "
        "answered. Call this for any question about what is on.",
        {
            "week": {
                "type": "string",
                "enum": ["this", "next"],
                "description": "'this' for the current boss week, 'next' for the one after.",
            },
            "scope": {
                "type": "string",
                "enum": ["all", "channel"],
                "description": (
                    "Use 'channel' when they say 'this channel', 'here', 'our runs' or "
                    "anything else meaning the channel you are talking in. Use 'all' "
                    "(the default) for the whole group. When you answer from 'all', "
                    "never claim the runs are channel-specific -- say which channel "
                    "each one is in, or say it is the whole group's week."
                ),
            },
        },
        ["week"],
    ),
    _tool(
        "get_run",
        "One run in full, including who is on it and what each of them answered.",
        {"query": _RUN_QUERY},
        ["query"],
    ),
    _tool(
        "list_bosses",
        "The bosses this guild runs, with their difficulties. Use it to check a name.",
        {},
        [],
    ),
    _tool(
        "get_pending",
        "Proposal cards that are waiting for somebody to react ✅ or ❌.",
        {},
        [],
    ),
    _tool(
        "propose_move",
        "Post a card proposing that a run moves to a new day and time. This does NOT "
        "move the run: somebody has to react ✅ on the card first.",
        {
            "run_query": _RUN_QUERY,
            "to_when": {
                "type": "string",
                "description": "The new day and time, e.g. 'wed 21:30' or 'tomorrow 9:45pm'.",
            },
        },
        ["run_query", "to_when"],
    ),
    _tool(
        "propose_add",
        "Post a card proposing a NEW run that is not on the schedule yet. This does NOT "
        "create it: somebody has to react ✅ on the card first.",
        {
            "boss": {
                "type": "string",
                "description": (
                    "The boss, with its difficulty, e.g. 'HBellona' or 'XKalos'. A bare "
                    "name without a difficulty is refused -- ask which one they mean."
                ),
            },
            "when": {
                "type": "string",
                "description": "The day and time, e.g. 'today 21:30' or 'sat 9pm'.",
            },
            "participants": {
                "type": "string",
                "description": (
                    "Optional. Who the run is for, by name, comma separated. Leave it out "
                    "for just the person asking -- which is the default."
                ),
            },
        },
        ["boss", "when"],
    ),
    _tool(
        "propose_cancel",
        "Post a card proposing that ONE dated run is cancelled -- a single night off. "
        'For the recurring weekly baseline ("remove the fixed run", "stop doing this '
        'every week") use propose_remove_fixed instead. This does NOT cancel anything: '
        "somebody has to react ✅ on the card first.",
        {"run_query": _RUN_QUERY},
        ["run_query"],
    ),
    _tool(
        "propose_remove_fixed",
        "Post a card proposing that a RECURRING weekly timing is removed, so the boss "
        "stops being scheduled every week. This is not the same as cancelling one night "
        "-- for a single dated run use propose_cancel. If it is unclear which they mean, "
        'ask: "just this week\'s run, or the weekly one?" This does NOT remove anything: '
        "somebody has to react ✅ on the card first.",
        {
            "query": {
                "type": "string",
                "description": (
                    "Which weekly timing: its short id, or a boss and (if there are "
                    "several) a day, like 'weekly hbellona' or 'bellona tuesday'."
                ),
            }
        },
        ["query"],
    ),
    _tool(
        "propose_rsvp",
        "Post a card recording the answer of the person you are talking to for one run. "
        "Only ever for them -- you cannot answer on anybody else's behalf.",
        {
            "run_query": _RUN_QUERY,
            "answer": {
                "type": "string",
                "enum": ["yes", "no"],
                "description": "Whether the person speaking to you can make that run.",
            },
        },
        ["run_query", "answer"],
    ),
]


def tool_names() -> list[str]:
    return [t["function"]["name"] for t in TOOLS]


# ---------------------------------------------------------------------------
# resolving a run from something a person said
# ---------------------------------------------------------------------------

_RELATIVE_DAYS = {"today": 0, "tonight": 0, "tomorrow": 1, "tmr": 1, "tmrw": 1}


def _boss_words(bot: Any, token: str) -> list[str]:
    """The names a person might use for one canonical boss token."""
    detail = bot.bosses.detail(token)
    if detail is None:
        return [token.lower()]
    return [str(detail["short"]).lower(), str(detail["full"]).lower(), token.lower()]


def _says(query: str, word: str) -> bool:
    """Is ``word`` actually written in ``query``, as a word?

    Whole words only. Several bosses have two-letter short names (``FA``,
    ``BM``), and a bare substring test matches those inside anything at all --
    inside "faster", and inside a run id like ``152fa345``, which is how a
    `propose_cancel` for a mistyped id came to resolve to somebody's HFA night.
    """
    return re.search(rf"\b{re.escape(word)}\b", query) is not None


def _day_matches(query: str, run: dict, bot: Any) -> bool:
    local = run["datetime"].astimezone(bot.tz)
    weekday = local.weekday()
    if any(_says(query, word) for word, index in WEEKDAY_ALIASES.items() if index == weekday):
        return True
    today = utcnow().astimezone(bot.tz).date()
    return any(
        _says(query, word) and (local.date() - today).days == offset
        for word, offset in _RELATIVE_DAYS.items()
    )


def _names_a_day(query: str) -> bool:
    """Does the query pick out a day at all? Decides whether to narrow by one."""
    return any(_says(query, word) for word in (*WEEKDAY_ALIASES, *_RELATIVE_DAYS))


def resolve_run(bot: Any, query: str) -> dict:
    """A run from a short id, or from a boss name and (optionally) a day.

    Ids are tried first and through :func:`bot.api.service.load_run`, which is
    the same prefix resolution the portal and ``bossctl`` use -- including its
    "that matches several runs" error, which is a far better answer than picking
    one. Only if the text is not an id at all does the boss/day search run, over
    this boss week and the next; a query that matches several nights is refused
    with the candidates named, because guessing moves the wrong party's evening.
    """
    text = (query or "").strip()
    if not text:
        raise ToolError("Ask them which run they mean -- a boss and a day, like 'hstar wednesday'.")
    try:
        return service.load_run(bot, text)
    except (NotFound, BadRequest):
        pass

    low = text.lower()
    candidates = [
        run
        for which in ("this", "next")
        for run in bot.repo.list_runs(week_start=service.week_for(bot, which))
        if run["status"] not in ("cancelled", "done")
    ]
    by_boss = [
        run
        for run in candidates
        if any(_says(low, word) for token in run["bosses"] for word in _boss_words(bot, token))
    ]
    named_day = _names_a_day(low)
    if not by_boss and not named_day:
        # Nothing in the text locates a run. Falling back to "every run" here
        # would resolve gibberish to the only run in a quiet week, and a
        # `propose_cancel` built on that guess cancels the wrong night.
        raise ToolError(
            f"No run matches `{text}`. Call get_schedule to see what is on, then ask them "
            "which one they mean. Do not guess."
        )
    matches = by_boss or candidates
    if named_day:
        narrowed = [run for run in matches if _day_matches(low, run, bot)]
        if narrowed:
            matches = narrowed
        elif by_boss:
            # The boss is real and the day is not one it runs on. Saying so beats
            # silently answering about a different night.
            raise ToolError(
                f"No run matches `{text}`. " + _listing(bot, by_boss, "That boss is on")
            )
    if not matches:
        raise ToolError(
            f"No run matches `{text}`. Call get_schedule to see what is on, then ask them "
            "which one they mean. Do not guess."
        )
    if len(matches) > 1:
        raise ToolError(
            f"`{text}` matches more than one run. " + _listing(bot, matches, "Ask which one:")
        )
    return matches[0]


def _listing(bot: Any, runs: list[dict], lead: str) -> str:
    return lead + " " + "; ".join(_run_line(bot, run) for run in runs[:MAX_RUNS])


# ---------------------------------------------------------------------------
# rendering, in the fewest tokens that stay unambiguous
# ---------------------------------------------------------------------------


def _run_line(bot: Any, run: dict, with_channel: bool = False) -> str:
    local = run["datetime"].astimezone(bot.tz)
    rsvps = bot.repo.get_rsvps(run["id"])
    yes = sum(1 for uid in run["participants"] if rsvps.get(uid) == "yes")
    # Only on a guild-wide listing, and only when the bot can actually see the
    # channel: `channel_name` returns None off the gateway, and "#None" would be
    # worse than saying nothing.
    where = service.channel_name(bot, run["channel_id"]) if with_channel else None
    return (
        f"[{short_id(run['id'])}] {local.strftime('%a %d %b %H:%M')} "
        f"{'+'.join(run['bosses'])} ({run['status']}, {yes}/{len(run['participants'])} yes)"
        + (f" in {where}" if where else "")
    )


def _run_detail(bot: Any, run: dict) -> str:
    view = service.run_view(bot, run)
    people = ", ".join(f"{p['name']} ({p['rsvp'] or 'no answer'})" for p in view["participants"])
    return (
        f"Run {view['short_id']}: {'+'.join(view['bosses'])} on {view['local_day']} "
        f"{view['local_time']}, status {view['status']}. "
        f"On it: {people or 'nobody'}."
    )


# ---------------------------------------------------------------------------
# the read tools
# ---------------------------------------------------------------------------


def _get_schedule(ctx: ToolContext, args: dict) -> str:
    """The week, for the whole guild or for the channel the question came from.

    ``scope="all"`` is the default and what the tool always used to do. It now
    names each run's channel, because a guild-wide answer that reads like a
    channel-specific one is exactly the bug this argument exists to fix: asked
    "what runs are in this channel?", the model got the whole group's week and
    dutifully relabelled it "in this channel".
    """
    week = str(args.get("week") or "this").strip().lower()
    if week not in ("this", "next"):
        raise ToolError("week must be 'this' or 'next'. Ask them which week they mean.")
    scope = str(args.get("scope") or "all").strip().lower()
    if scope not in ("all", "channel"):
        raise ToolError("scope must be 'all' or 'channel'.")

    everything = [
        run
        for run in ctx.bot.repo.list_runs(week_start=service.week_for(ctx.bot, week))
        if run["status"] != "cancelled"
    ]
    here = ctx.channel_id
    runs = (
        [run for run in everything if str(run["channel_id"]) == str(here)]
        if scope == "channel"
        else everything
    )
    if not runs:
        if scope == "channel":
            # Never let "nothing here" be reported as "nothing at all".
            elsewhere = (
                f" The group has {len(everything)} run(s) in other channels this week -- "
                "call get_schedule again with scope='all' if they want those."
                if everything
                else ""
            )
            return f"No runs are scheduled in this channel for {week} boss week.{elsewhere}"
        return f"Nothing is scheduled for {week} boss week."

    runs.sort(key=lambda run: run["datetime"])
    lines = [_run_line(ctx.bot, run, with_channel=scope == "all") for run in runs[:MAX_RUNS]]
    more = len(runs) - len(lines)
    heading = (
        f"{week.capitalize()} boss week, in this channel only:"
        if scope == "channel"
        else f"{week.capitalize()} boss week, ALL channels (say which channel each run is in):"
    )
    return "\n".join([heading, *lines]) + (f"\n(and {more} more)" if more > 0 else "")


def _get_run(ctx: ToolContext, args: dict) -> str:
    return _run_detail(ctx.bot, resolve_run(ctx.bot, str(args.get("query") or "")))


def _list_bosses(ctx: ToolContext, args: dict) -> str:
    rows = [
        f"{boss.short} ({boss.full}, lv {boss.level}): "
        + ", ".join(boss.canonical(letter) for letter in boss.difficulties)
        for boss in ctx.bot.bosses.ordered()
    ]
    return "\n".join(["Bosses this guild runs:", *rows])


def _get_pending(ctx: ToolContext, args: dict) -> str:
    open_cards = service.pending(ctx.bot)
    if not open_cards:
        return "There are no proposal cards waiting."
    lines = [
        f"[{card['short_id']}] {card['kind_label']} {'+'.join(card['bosses'])} -> {card['when']}"
        for card in open_cards[:MAX_RUNS]
    ]
    return "\n".join(["Waiting for a ✅:", *lines])


# ---------------------------------------------------------------------------
# the write tools -- each one posts a card and changes nothing
# ---------------------------------------------------------------------------


async def _propose(
    ctx: ToolContext,
    *,
    kind: str,
    run: dict | None,
    at: datetime | None,
    summary: str,
    bosses: Sequence[str] | None = None,
    rsvp: str | None = None,
    participants: list[str] | None = None,
    payload: dict | None = None,
    week: datetime | None = None,
) -> str:
    """Create the proposal row and its card, through the extractor's own path.

    Routed through :meth:`bot.extract.pipeline.Pipeline.apply_plan` rather than
    writing the row here, so a card raised in chat is the same object as a card
    raised by a rescan: same supersede rules, same wording, same ✅ handler, same
    24 h expiry. The evidence is the message that asked for it, which is what
    makes the card's "why" link back to a real line in the channel.

    ``run`` is ``None`` for an ``add``, which creates a run rather than changing
    one; ``bosses`` then says what it is for. Everything else -- superseding an
    older card for the same bosses, the card wording, the ✅ handler -- is the
    extractor's, unchanged.
    """
    bot = ctx.bot
    local = at.astimezone(bot.tz) if at is not None else None
    resolved = Resolved(
        day=local.date() if local else None,
        clock=local.time() if local else None,
        at=at,
    )
    amendment = Amendment(
        kind=kind,
        bosses=list(bosses if bosses is not None else run["bosses"]),
        participants=list(participants or []),
        rsvp=rsvp,
        # Stated by a person and read back to them before anything happens, so
        # there is no uncertainty for a confidence score to express.
        confidence=1.0,
        evidence_message_ids=[str(ctx.message_id)],
    )
    if at is not None:
        week = week_start(at, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time)
    elif run is not None:
        week = run["week_start"]
    elif week is None:  # pragma: no cover - callers with no run pass one
        raise ToolError("Ask them which day and time they mean.")
    planned = Planned(
        amendment=amendment,
        resolved=resolved,
        run=run,
        summary=summary,
        payload=dict(payload or {}),
    )
    created = await bot.extractor.apply_plan(ctx.channel_id, [], [planned], week, summary)
    ctx.created.extend(created)
    if not created:  # pragma: no cover - apply_plan returns a row per proposal
        raise ToolError("The card could not be created. Tell them to try again in a moment.")
    posted = any((bot.repo.get_amendment(aid) or {}).get("proposal_message_id") for aid in created)
    if not posted:
        raise ToolError(
            "The change was recorded but the card could not be posted to the channel. "
            "Tell them to check with an admin."
        )
    return (
        f"Posted a proposal card ({', '.join(short_id(a) for a in created)}). "
        "Nothing has changed yet: it takes effect only when somebody reacts ✅ on it. "
        "Tell them the card is up and needs a ✅."
    )


async def _propose_move(ctx: ToolContext, args: dict) -> str:
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    raw = str(args.get("to_when") or "").strip()
    if not raw:
        raise ToolError("Ask them what day and time it should move to.")
    try:
        at = service.parse_when(ctx.bot, raw)
    except BadRequest as exc:
        raise ToolError(f"{exc.message}. Ask them for the day and time again.") from None
    if at <= utcnow():
        raise ToolError(f"`{raw}` is in the past. Ask them which day they mean.")
    if at == run["datetime"]:
        raise ToolError("That run is already at that time; nothing to propose.")
    return await _propose(
        ctx,
        kind="move",
        run=run,
        at=at,
        summary=f"move {'+'.join(run['bosses'])} to {at.astimezone(ctx.bot.tz):%a %d %b %H:%M}",
    )


def _validate_bosses(ctx: ToolContext, text: str) -> list[str]:
    """Boss tokens for a new run, or a refusal that says what to ask.

    :meth:`bot.bosses.BossTable.parse` already produces exactly the right
    sentence for the commonest mistake -- "`bellona` is missing a difficulty
    prefix (e/n/h) - try NBellona, HBellona" -- because a bare boss name is
    ambiguous in game, not just here. It is passed through with an instruction
    wrapped round it, so the model asks rather than picking a difficulty.
    """
    if not (text or "").strip():
        raise ToolError("Ask them which boss they mean.")
    try:
        return service.validate_bosses(ctx.bot, text)
    except BadRequest as exc:
        raise ToolError(
            f"{exc.message}. Ask them which one they mean -- do not choose a difficulty for them."
        ) from None


#: "put me on it" -- the one name the model can be certain of, and the one the
#: roster cannot resolve. Word-bounded so `i` does not eat the `i` in a nickname.
_FIRST_PERSON_RE = re.compile(r"\b(?:me|myself|i)\b", re.IGNORECASE)

#: Words that join names rather than being one. ``resolve_participant_text`` was
#: written for a slash-command field where people type only names, so it reads
#: "me and kanon" as three of them and refuses over "and". These are forgiven
#: only when they matched *nobody*, so a member actually nicknamed "Us" still
#: resolves to themselves.
_JOINING_WORDS = frozenset(
    {
        "a",
        "add",
        "along",
        "also",
        "and",
        "both",
        "for",
        "it",
        "just",
        "me",
        "on",
        "party",
        "please",
        "plus",
        "run",
        "team",
        "the",
        "then",
        "too",
        "us",
        "with",
        "&",
        "+",
    }
)


def _without_the_bot(ctx: ToolContext, text: str) -> str:
    """Drop every reference to the bot itself from a participants field.

    The trigger mention is part of the message the model is reading, and it duly
    passed it straight back as a participant. ``validate_participants`` then
    refused with "not in the bossing role: user 5555 (…)", and the model relayed
    that to the channel in the first person -- "I'm not in the bossing role" --
    which is both untrue and baffling to read.

    The bot is not a member of anything. It is the thing being spoken to, so its
    id, its mention markup and its name are stripped before anybody tries to
    resolve them to a person.
    """
    user = getattr(ctx.bot, "user", None)
    cleaned = text
    bot_id = str(getattr(user, "id", "") or "")
    if bot_id:
        cleaned = re.sub(rf"<@!?{re.escape(bot_id)}>", " ", cleaned)
        cleaned = re.sub(rf"\b{re.escape(bot_id)}\b", " ", cleaned)
    for name in {getattr(user, "name", ""), getattr(user, "display_name", "")}:
        if name:
            cleaned = re.sub(rf"\b{re.escape(str(name))}\b", " ", cleaned, flags=re.IGNORECASE)
    return cleaned


def _validate_participants(ctx: ToolContext, text: Any) -> list[str]:
    """Who the new run is for: the asker by default, never whoever the model names.

    An empty field means the person asking, taken from the message rather than
    from anything the model said. Names that are supplied are resolved against
    the roster the same way `/fixed add` resolves them, so the model cannot
    invent a member or put a bare snowflake on a run.

    Two substitutions happen first, both about the two "people" a roster lookup
    can never resolve: the bot itself is stripped out entirely
    (:func:`_without_the_bot`), and "me"/"myself"/"I" become the asker's own id,
    taken from the message rather than from the model.
    """
    raw = ", ".join(str(t) for t in text) if isinstance(text, (list, tuple)) else str(text or "")
    raw = _FIRST_PERSON_RE.sub(f"<@{ctx.author_id}>", _without_the_bot(ctx, raw))
    if not raw.strip():
        # Empty to begin with, or nothing left once the bot was removed -- which
        # is what "@YuukiSakuna schedule a run" looks like by the time the model
        # has copied the trigger mention into the participants field.
        return [ctx.author_id]
    resolution = resolve_participant_text(raw, ctx.bot.repo.list_members())
    strangers = [word for word in resolution.unknown if word.lower() not in _JOINING_WORDS]
    if strangers:
        raise ToolError(
            f"Nobody on the roster matches {', '.join(strangers)}. "
            "Ask them who should be on it, or leave it as just them."
        )
    if resolution.ambiguous:
        options = "; ".join(f"{k}: {', '.join(v)}" for k, v in resolution.ambiguous.items())
        raise ToolError(f"Ask them which they mean -- {options}.")
    # Belt and braces: a bare id that survived the strip above is still not a
    # person, and must never reach `validate_participants` to be reported as a
    # member who lacks a role.
    bot_id = str(getattr(getattr(ctx.bot, "user", None), "id", "") or "")
    people = [uid for uid in resolution.ids if uid != bot_id]
    if not people:
        return [ctx.author_id]
    try:
        return service.validate_participants(ctx.bot, people)
    except BadRequest as exc:
        raise ToolError(f"{exc.message}. Ask them who should be on it.") from None


async def _propose_add(ctx: ToolContext, args: dict) -> str:
    """Post a card proposing a run that does not exist yet.

    The `add` kind and its commit handler are the extractor's, already used for
    "wanna do trio ncarling saturday?" read out of chat. Nothing new is written
    here: this only builds the same amendment from a sentence addressed to the
    bot instead of one overheard in a party channel.
    """
    bosses = _validate_bosses(ctx, str(args.get("boss") or ""))
    raw = str(args.get("when") or "").strip()
    if not raw:
        raise ToolError("Ask them what day and time the run should be.")
    try:
        at = service.parse_when(ctx.bot, raw)
    except BadRequest as exc:
        raise ToolError(f"{exc.message}. Ask them for the day and time again.") from None
    if at <= utcnow():
        raise ToolError(f"`{raw}` is in the past. Ask them which day they mean.")
    people = _validate_participants(ctx, args.get("participants"))
    return await _propose(
        ctx,
        kind="add",
        run=None,
        at=at,
        bosses=bosses,
        participants=people,
        summary=f"new run: {'+'.join(bosses)} on {at.astimezone(ctx.bot.tz):%a %d %b %H:%M}",
    )


def _fixed_line(bot: Any, fixed: dict) -> str:
    return (
        f"[{short_id(fixed['id'])}] every {WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']} "
        f"{'+'.join(fixed['bosses'])}"
    )


def _fixed_when(fixed: dict) -> str:
    return f"{WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']}"


def resolve_fixed(bot: Any, query: str) -> dict:
    """A weekly timing from a short id, or a boss name and (optionally) a day.

    The same shape as :func:`resolve_run` and for the same reason: ids first,
    through the service layer's prefix resolution, then a boss/day search that
    refuses rather than guesses. Removing the wrong baseline is worse than
    cancelling the wrong night -- nothing re-materialises it.
    """
    text = (query or "").strip()
    if not text:
        raise ToolError("Ask them which weekly timing they mean -- a boss, and a day if needed.")
    try:
        return service.load_fixed(bot, text)
    except (NotFound, BadRequest):
        pass

    low = text.lower()
    candidates = bot.repo.list_fixed_runs()
    by_boss = [
        fixed
        for fixed in candidates
        if any(_says(low, word) for token in fixed["bosses"] for word in _boss_words(bot, token))
    ]
    named_day = _names_a_day(low)
    if not by_boss and not named_day:
        raise ToolError(
            f"No weekly timing matches `{text}`. Ask them which boss's weekly run they mean."
        )
    matches = by_boss or candidates
    if named_day:
        narrowed = [
            fixed
            for fixed in matches
            if any(
                _says(low, word)
                for word, index in WEEKDAY_ALIASES.items()
                if index == fixed["weekday"]
            )
        ]
        if narrowed:
            matches = narrowed
    if not matches:
        raise ToolError(f"No weekly timing matches `{text}`.")
    if len(matches) > 1:
        listed = "; ".join(_fixed_line(bot, f) for f in matches[:MAX_RUNS])
        raise ToolError(f"`{text}` matches more than one weekly timing. Ask which one: {listed}")
    return matches[0]


async def _propose_remove_fixed(ctx: ToolContext, args: dict) -> str:
    """Post a card proposing that a weekly timing stops existing.

    The opposite of the extractor's `fix`, and the thing that was missing when
    somebody said "remove the fixed run" live: the pilot cancelled the two runs
    it had already produced, and the baseline quietly went on producing more.
    """
    fixed = resolve_fixed(ctx.bot, str(args.get("query") or ""))
    return await _propose(
        ctx,
        kind="fix",
        run=None,
        at=None,
        bosses=list(fixed["bosses"]),
        participants=[str(p) for p in fixed["participants"]],
        week=service.week_for(ctx.bot, "this"),
        payload={
            "op": FIX_REMOVE,
            "fixed_run_id": fixed["id"],
            # Carried so the card can say which night it is retiring without
            # looking the row up again -- by ✅ time it may be gone.
            "weekly_when": _fixed_when(fixed),
        },
        summary=(f"stop scheduling {'+'.join(fixed['bosses'])} every {_fixed_when(fixed)}"),
    )


async def _propose_cancel(ctx: ToolContext, args: dict) -> str:
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    if run["status"] == "cancelled":
        raise ToolError("That run is already cancelled.")
    return await _propose(
        ctx,
        kind="cancel",
        run=run,
        at=None,
        summary=f"cancel {'+'.join(run['bosses'])}",
    )


async def _propose_rsvp(ctx: ToolContext, args: dict) -> str:
    """Record the *asker's* answer. The author id comes from Discord, never the model.

    ``propose_rsvp`` takes no user argument at all: there is nothing for a
    message to say that could point the answer at somebody else. A model told
    "say no for kanon" can at most produce a card recording the *speaker's* no,
    which is visible on the card and reversible with a ❌.
    """
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    answer = str(args.get("answer") or "").strip().lower()
    if answer not in ("yes", "no"):
        raise ToolError("answer must be 'yes' or 'no'. Ask them whether they can make it.")
    if ctx.author_id not in run["participants"]:
        raise ToolError(
            f"They are not on run {short_id(run['id'])}, so they have nothing to answer. "
            "Only somebody on a run can RSVP for it."
        )
    return await _propose(
        ctx,
        kind="rsvp",
        run=run,
        at=None,
        rsvp=answer,
        participants=[ctx.author_id],
        summary=f"{service.member_name(ctx.bot, ctx.author_id)} says {answer}",
    )


# ---------------------------------------------------------------------------
# dispatch
# ---------------------------------------------------------------------------

_READ = {
    "get_schedule": _get_schedule,
    "get_run": _get_run,
    "list_bosses": _list_bosses,
    "get_pending": _get_pending,
}

_WRITE = {
    "propose_add": _propose_add,
    "propose_remove_fixed": _propose_remove_fixed,
    "propose_move": _propose_move,
    "propose_cancel": _propose_cancel,
    "propose_rsvp": _propose_rsvp,
}


def _arguments(raw: Any) -> dict:
    """The model's arguments as a dict, whatever shape the client handed back.

    ollama returns them already parsed, but a model that emits a JSON *string*
    is common enough that failing on it would look like the tool is broken.
    """
    if isinstance(raw, dict):
        return raw
    if isinstance(raw, str):
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            return {}
        return parsed if isinstance(parsed, dict) else {}
    return {}


#: What went wrong with a tool call, in the words the log uses.
REFUSED = "refused"
UNKNOWN = "unknown tool"
FAILED = "failed"


@dataclass
class ToolOutcome:
    """One tool call, as the log wants to describe it.

    Exists because "why did it propose that?" is answered by the *arguments* the
    model passed and whether the tool did as it was told -- neither of which
    survives being flattened into the answer string the model reads.
    """

    name: str
    output: str
    arguments: dict = field(default_factory=dict)
    #: The tool did what was asked. A refusal is a *successful* refusal only in
    #: the sense that it did not crash; `ok` is False for all three failures.
    ok: bool = True
    error: str | None = None
    duration_ms: int = 0
    #: Amendment ids this one call created, so a card can be traced to the call.
    created: list[str] = field(default_factory=list)

    @property
    def outcome(self) -> str:
        return "ok" if self.ok else (self.error or FAILED)


async def run(ctx: ToolContext, name: str, arguments: Any) -> ToolOutcome:
    """Run one tool call and describe what happened.

    Never raises: a tool that fails has to come back as text the model can
    apologise about, because the alternative is an exception unwinding through
    the agent and the member getting silence.
    """
    started = time.monotonic()
    already = len(ctx.created)
    args = _arguments(arguments)

    def done(output: str, ok: bool = True, error: str | None = None) -> ToolOutcome:
        return ToolOutcome(
            name=name,
            output=output,
            arguments=args,
            ok=ok,
            error=error,
            duration_ms=int((time.monotonic() - started) * 1000),
            created=list(ctx.created[already:]),
        )

    handler = _READ.get(name) or _WRITE.get(name)
    if handler is None:
        log.warning("chat: the model asked for an unknown tool %r", name)
        return done(UNKNOWN_TOOL.format(name=name, known=", ".join(tool_names())), False, UNKNOWN)
    try:
        output = await handler(ctx, args) if name in _WRITE else handler(ctx, args)
        return done(output)
    except ToolError as exc:
        log.info("chat: %s refused: %s", name, exc)
        return done(str(exc), False, REFUSED)
    except (BadRequest, NotFound) as exc:
        log.info("chat: %s rejected by the service layer: %s", name, exc.message)
        return done(str(exc.message), False, REFUSED)
    except Exception:  # noqa: BLE001 - a tool must never take the answer down
        log.exception("chat: %s failed", name)
        return done(
            "That lookup failed. Say you could not reach the schedule just now.", False, FAILED
        )


async def dispatch(ctx: ToolContext, name: str, arguments: Any) -> str:
    """Just the text the model reads, for callers that do not log."""
    return (await run(ctx, name, arguments)).output
