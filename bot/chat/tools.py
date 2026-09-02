"""The closed set of things the chatbot can do, and the dispatcher behind it.

Two rules hold this module together.

**Everything the model says is untrusted input.**  A tool call is a string the
model produced from a message a member wrote, so every argument is re-validated
here against the service layer -- run ids through :func:`bot.api.service.load_run`,
times through :func:`bot.api.service.parse_when` -- exactly as if it had arrived
over HTTP.  Nothing is passed through on the model's say-so.

**A write is scoped to the person asking.**  The tools that target something
that already exists refuse unless the asker is on it (or owns the weekly timing
behind it) and is asking from the channel it lives in -- see
:func:`_require_authority`, which is the one place that rule lives.  Otherwise
one member could raise cards about every other party's evenings from their own
channel.

**No write reaches the schedule.**  The six ``propose_*`` tools do not change
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

from .. import formatting
from ..api import service
from ..api.errors import BadRequest, NotFound
from ..extract.commit import FIX_EDIT, FIX_REMOVE
from ..extract.pipeline import Planned
from ..extract.resolve import WEEKDAY_ALIASES, Resolved
from ..extract.schema import Amendment
from ..ids import short_id
from ..timeutil import utcnow
from ..util import resolve_participant_text
from ..weeks import WEEKDAY_NAMES, parse_hhmm, parse_weekday, week_start
from . import gate

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
    "READ_ONLY_TURN",
    "REFUSED",
    "TOOLS",
    "UNKNOWN",
    "ToolContext",
    "ToolError",
    "ToolOutcome",
    "dispatch",
    "read_tools",
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
    #: Does the author run this bot (:func:`bot.util.is_bot_admin`)? Decided from
    #: the live member object by :meth:`bot.chat.agent.ChatPilot._is_admin` and
    #: carried here as a fact, never re-derived from anything the model said. It
    #: is the one exemption from :func:`_require_authority`; it defaults to
    #: ``False`` so any context built without one is the least-privileged one.
    is_admin: bool = False
    #: No card may be posted in this turn, whatever the model asks for. Set for
    #: the bot's *own* turns -- the rejection follow-up
    #: (:mod:`bot.chat.followup`), which is generated from a card rather than
    #: from anything a member said and must only ever produce a question. It is
    #: enforced in :func:`run`, not by withholding the schemas alone, so a model
    #: that names a write tool from memory is refused rather than obeyed.
    read_only: bool = False
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
        "answered. Call this for any question about what is on. Runs marked 'already "
        "happened' are in the past: never offer one as the next or upcoming run. If "
        "nothing upcoming is left, say so plainly instead of reaching for a past run.",
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
        "Post a card proposing that ONE dated run moves to a new day and time -- that "
        "night only, leaving the rest of the schedule alone. If they mean the recurring "
        'weekly itself ("change the weekly to 23:30", "we do it on Wednesdays now"), use '
        "propose_change_fixed instead. This does NOT move the run: somebody has to react "
        "✅ on the card first.",
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
        "Post a card proposing a NEW run that is not on the schedule yet. By default it is "
        "a ONE-TIME run that week -- only `weekly` makes it recurring. Never use this to "
        "change a weekly that already exists: it would leave a second one beside it and "
        "the party on neither. Use propose_change_fixed for that. This does NOT create "
        "it: somebody has to react ✅ on the card first.",
        {
            "boss": {
                "type": "string",
                "description": (
                    "One complete difficulty-qualified boss. Use EITHER a canonical token such as "
                    "'XBM', 'HBellona', or 'XKalos', OR words with the difficulty first, such as "
                    "'Extreme Black Mage' or 'Hard Bellona'. Do not combine forms or add a second "
                    "difficulty: 'XBM Hard' is invalid; 'extreme bm' means 'XBM'. A bare boss name "
                    "without a difficulty is refused -- ask which one they mean."
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
                    "for just the person asking -- which is the default. When you do fill "
                    "it in, the person asking goes in it too whenever they put themselves "
                    "on the run -- 'for me', 'for us', 'I'll come', 'count me in'. Every "
                    "line you are shown is labelled with who said it, so their name is one "
                    "you can write; the word 'me' works as well. 'Schedule a run for me "
                    "and kanon' is a run for BOTH of them: never leave out the person "
                    "asking for it."
                ),
            },
            "weekly": {
                "type": "boolean",
                "description": (
                    "Optional, default false, meaning ONE run on that day only. Set it true "
                    "ONLY when they explicitly say it repeats -- 'weekly', 'every week', "
                    "'every Tuesday', 'recurring', 'fixed'. A separate sentence about the "
                    "run they just asked for counts as saying it: 'tonight 1900, this is "
                    "fixed', 'make it fixed', 'make it weekly' are all true, even though "
                    "the rest of the line reads one-time. Asking for a run that ALREADY "
                    "exists to repeat -- 'make this weekly', 'make it run every week' -- is "
                    "this argument too, not propose_change_fixed: pass the run's boss and "
                    "the day and time it should keep, and the scheduler folds this week's "
                    "run into the new weekly instead of leaving a duplicate beside it. "
                    "'Schedule a run', 'add a run tonight', 'can we do HStar friday' are "
                    "all one-time: leave it out. If their wording is unclear, do NOT ask "
                    "which they mean -- leave it out. "
                    "One-time is the safe default and the card says which one it is."
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
        "propose_change_fixed",
        "Post a card proposing that an EXISTING recurring weekly timing changes: the day "
        "and time it happens every week, who is on it, or both. This is the tool for "
        '"change the weekly to 23:30", "we do the fixed run on Wednesdays now" and "add '
        "Priya to the weekly\". It is NOT for one week's run on its own -- propose_move "
        "does that -- and never reach for propose_add instead: adding another weekly "
        "leaves a duplicate and the party split across the two. This does NOT change "
        "anything: somebody has to react ✅ on the card first.",
        {
            "query": {
                "type": "string",
                "description": (
                    "Which weekly timing: its short id from an earlier tool result, or the "
                    "boss and the day it runs on NOW, like 'hlimbo monday'. A channel can "
                    "have several weekly timings -- even two for the same boss on different "
                    "nights -- so give the boss AND its current day, and the time too if "
                    "that is still not enough. If you cannot tell which one they mean, ask "
                    "them; never pick one yourself."
                ),
            },
            "day": {
                "type": "string",
                "description": (
                    "Optional. The new day of the week it should happen on, e.g. "
                    "'wednesday'. Leave it out when only the time changes."
                ),
            },
            "time": {
                "type": "string",
                "description": (
                    "Optional. The new start time, e.g. '23:30' or '9:30pm'. Leave it out "
                    "when only the day changes."
                ),
            },
            "participants": {
                "type": "string",
                "description": (
                    "Optional. The WHOLE party it should have from now on, by name, comma "
                    "separated -- not only the people joining or leaving. That includes "
                    "the person asking whenever they put themselves on it: 'add me to the "
                    "weekly' means the party it has now plus them, and every line you are "
                    "shown is labelled with who said it. Leave it out to keep the party "
                    "exactly as it is."
                ),
            },
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


def _is_over(run: dict) -> bool:
    """Is this run finished or in the past?

    The arithmetic is done here rather than left to the model. Live, asked "when
    is the next boss run here?", it answered with a run that had finished hours
    earlier: it had the timestamp and it had the clock in its system prompt, and
    it still did not compare them.
    """
    return run["status"] == "done" or run["datetime"] <= utcnow()


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
        f"{formatting.boss_labels(run['bosses'])} "
        f"({run['status']}, {yes}/{len(run['participants'])} yes)"
        + (f" in {where}" if where else "")
        + (" -- already happened" if _is_over(run) else "")
    )


def _run_detail(bot: Any, run: dict) -> str:
    view = service.run_view(bot, run)
    people = ", ".join(f"{p['name']} ({p['rsvp'] or 'no answer'})" for p in view["participants"])
    return (
        f"Run {view['short_id']}: {formatting.boss_labels(view['bosses'])} on "
        f"{view['local_day']} {view['local_time']}, status {view['status']}. "
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
    answer = "\n".join([heading, *lines]) + (f"\n(and {more} more)" if more > 0 else "")
    if all(_is_over(run) for run in runs):
        # The per-line markers are enough when only some are past; a week with
        # nothing left at all is what made the model pick a finished run as "the
        # next one", so that case is stated outright.
        answer += (
            "\nEvery run listed has already happened -- nothing upcoming is left in "
            f"{week} boss week."
        )
    return answer


def _get_run(ctx: ToolContext, args: dict) -> str:
    return _run_detail(ctx.bot, resolve_run(ctx.bot, str(args.get("query") or "")))


def _list_bosses(ctx: ToolContext, args: dict) -> str:
    """The table, in both vocabularies.

    Each difficulty is given as the token to pass back to a tool *and* the words
    to say out loud, because the model has to do both: "XKalos" is what
    ``propose_add`` accepts, "Extreme Kalos" is what a member reads.
    """
    rows = [
        f"{boss.short} ({boss.full}, lv {boss.level}): "
        + ", ".join(
            f"{boss.canonical(letter)} = {formatting.boss_label(boss.canonical(letter))}"
            for letter in boss.difficulties
        )
        for boss in ctx.bot.bosses.ordered()
    ]
    return "\n".join(["Bosses this guild runs:", *rows])


def _get_pending(ctx: ToolContext, args: dict) -> str:
    open_cards = service.pending(ctx.bot)
    if not open_cards:
        return "There are no proposal cards waiting."
    lines = [
        f"[{card['short_id']}] {card['kind_label']} "
        f"{formatting.boss_labels(card['bosses'])} -> {card['when']}"
        for card in open_cards[:MAX_RUNS]
    ]
    return "\n".join(["Waiting for a ✅:", *lines])


# ---------------------------------------------------------------------------
# who may have a card drafted for them
# ---------------------------------------------------------------------------

#: Refusing a change to somebody else's run. It names the run because the asker
#: already named it, and nothing else: who is on it is not this refusal's to say.
NOT_THEIRS_RUN = (
    "They are not on run {sid} and do not own the weekly timing behind it, so it is not "
    "theirs to change. Say that only the people on a run -- or the owner of the weekly "
    "timing it comes from -- can propose a change to it, and that putting somebody on a "
    "run is not something you can do. Do not name anybody on it."
)

#: The same rule for a weekly timing, which is owned as well as attended.
NOT_THEIRS_FIXED = (
    "They are not on the weekly timing {sid} and do not own it, so it is not theirs to "
    "remove. Say that only the people on it, or whoever owns it, can propose that. Do "
    "not name anybody on it."
)

#: Refusing a change to a run that belongs to a different channel. The channel
#: name is the *only* thing this may leak: it is what makes the refusal
#: actionable ("ask there"), and the bot answers in that channel too.
ELSEWHERE = (
    "That {noun} lives in {where}, and changes to it are proposed from its own channel. "
    "Tell them which channel it lives in and to ask there. Say nothing else about it."
)


def _fixed_owner(bot: Any, run: dict) -> str | None:
    """Who owns the weekly timing this run was materialised from, if any.

    The same lookup `/amend` does (:func:`bot.commands._owner_of`): a run does
    not carry an owner, only the baseline behind it does.
    """
    fixed_id = run.get("fixed_run_id")
    if not fixed_id:
        return None
    fixed = bot.repo.get_fixed_run(str(fixed_id))
    return str(fixed["owner_id"]) if fixed else None


def _pilot_channel(bot: Any, channel_id: str) -> bool:
    """Is this a channel the pilot answers questions in?

    Asked through :func:`bot.chat.gate.is_chat_channel`, so "somewhere the bot
    can be asked" means exactly what the gate means by it -- category and thread
    resolution included. A channel the bot cannot see is not one of its own.
    """
    try:
        channel = bot.get_channel(int(channel_id))
    except (TypeError, ValueError):  # pragma: no cover - defensive
        return False
    return gate.is_chat_channel(channel, bot.settings)


def _require_authority(
    ctx: ToolContext, *, run: dict | None = None, fixed: dict | None = None
) -> None:
    """May this asker have a card drafted about this run (or weekly timing)?

    The threat is cross-channel card drafting. :func:`resolve_run` searches every
    channel's runs, and :meth:`bot.extract.pipeline.Pipeline.apply_plan` posts
    the card in the channel the *question* came from -- so without this, anybody
    holding the chat role could sit in their own channel and raise cards about
    another party's evenings: pinging that party, and retiring the live cards
    they were about to press, because a new proposal for a run supersedes the
    older ones (:func:`bot.extract.commit.supersede`).

    Two conditions, both required of everybody except an admin
    (:attr:`ToolContext.is_admin`):

    * they are on the run, or own the weekly timing behind it -- the same rule
      `/amend` applies through :func:`bot.util.can_modify_run`; and
    * the run is at home in the channel they are asking from.

    The second is applied only when the run's own channel is one the pilot
    answers in, because that is the whole content of the refusal: "ask in its
    own channel" is advice, not a rule, if the bot does not listen there. A
    deployment that gives the pilot one dedicated channel is asked about runs
    that live elsewhere by design; a deployment that turns it on across the
    party channels -- the one this attack was written against -- is not.

    This is the *first* of two locks and the only one the model can reach. The
    card's ✅ is the second and is independent: :func:`bot.extract.commit.may_commit`
    still decides who may confirm what this drafts, and it is enforced against
    the reacting member rather than the asking one.
    """
    if ctx.is_admin:
        return
    subject = run if run is not None else fixed
    if subject is None:  # pragma: no cover - every caller names one
        return
    owner = _fixed_owner(ctx.bot, run) if run is not None else str(fixed["owner_id"])
    if ctx.author_id not in [str(p) for p in subject["participants"]] and ctx.author_id != owner:
        sid = short_id(subject["id"])
        raise ToolError(
            NOT_THEIRS_RUN.format(sid=sid) if run is not None else NOT_THEIRS_FIXED.format(sid=sid)
        )
    home = str(subject["channel_id"] or "")
    if home and home != str(ctx.channel_id) and _pilot_channel(ctx.bot, home):
        raise ToolError(
            ELSEWHERE.format(
                noun="run" if run is not None else "weekly timing",
                where=service.channel_name(ctx.bot, home) or "another channel",
            )
        )


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

    The one thing said differently is who the audit trail credits: a rescan's
    card is the extractor's, and this one belongs to the member whose question
    raised it -- taken from the message, like every other use of ``author_id``.
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
    created = await bot.extractor.apply_plan(
        ctx.channel_id, [], [planned], week, summary, actor=ctx.author_id
    )
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
        f"Card {', '.join(short_id(a) for a in created)} posted: "
        f"{_card_text(ctx, kind, amendment, at, run, payload)}. "
        "Nothing has changed yet: it takes effect only when somebody reacts ✅ on it. "
        "Tell them the card is up and needs a ✅. The people named above are the whole "
        "party on it -- name those and nobody else, and never say you are on a run: you "
        "are a bot and cannot go to one."
    )


def _card_when(
    ctx: ToolContext, kind: str, at: datetime | None, run: dict | None, payload: dict
) -> str:
    """The day and time a card shows, in the words the card itself uses."""
    if kind == "fix":
        if payload.get("op") == FIX_EDIT:
            return _changed_when(payload)
        weekly = payload.get("weekly_when")
        if weekly:
            return f"every {weekly}"
        weekday, hhmm = payload.get("weekday"), payload.get("time")
        if weekday is not None and hhmm:
            return f"every {WEEKDAY_NAMES[int(weekday)]} {hhmm}"
    when = at if at is not None else (run["datetime"] if run is not None else None)
    return f"{when.astimezone(ctx.bot.tz):%a %d %b %H:%M}" if when is not None else ""


def _changed_when(payload: dict) -> str:
    """A change-the-weekly card's night, as ``was → is``.

    The old night is always named, even when only the party is moving, because
    the sentence the model reads back has to identify *which* weekly timing it
    just carded -- there may well be two for that boss.
    """
    was = payload.get("weekly_when") or ""
    weekday, hhmm = payload.get("weekday"), payload.get("time")
    if weekday is not None and hhmm:
        return f"every {was} → every {WEEKDAY_NAMES[int(weekday)]} {hhmm}"
    return f"every {was} (same night)"


def _card_text(
    ctx: ToolContext,
    kind: str,
    amendment: Amendment,
    at: datetime | None,
    run: dict | None,
    payload: dict | None,
) -> str:
    """What the card says, so the model's reply can only repeat it.

    Live, the model told a channel a run was "with @<the bot> and @<a member>":
    it was narrating the arguments it had *sent*, not the card that came back,
    so it put itself on the run and left the person who asked off it. The party
    here is the resolved one -- the ids actually written to the row -- by display
    name, and the bosses are spelled out the way the card spells them.
    """
    payload = dict(payload or {})
    people = list(amendment.participants) or (list(run["participants"]) if run else [])
    party = _names(ctx, people) or "nobody yet"
    # A card that changes the party says both sides of it, for the same reason
    # the night is said both ways: the row's own participants are the party as it
    # stands, and reading that back as "the party on it" would be the old one.
    joining = (
        [str(uid) for uid in payload.get("participants") or []]
        if payload.get("op") == FIX_EDIT
        else []
    )
    if joining:
        party = f"{party} → {_names(ctx, joining)}"
    when = _card_when(ctx, kind, at, run, payload)
    return f"{formatting.boss_labels(amendment.bosses)}{f' {when}' if when else ''} — {party}"


def _names(ctx: ToolContext, user_ids: Sequence[str]) -> str:
    """A party by display name, never as mentions -- the model must not learn to ping."""
    return ", ".join(service.member_name(ctx.bot, uid) for uid in user_ids)


async def _propose_move(ctx: ToolContext, args: dict) -> str:
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    _require_authority(ctx, run=run)
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
        summary=(
            f"move {formatting.boss_labels(run['bosses'])} "
            f"to {at.astimezone(ctx.bot.tz):%a %d %b %H:%M}"
        ),
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
            f"{_spell_out(ctx.bot, exc.message)}. Ask them which one they mean -- do not "
            "choose a difficulty for them. Ask in words ('Easy, Normal or Hard Bellona?') "
            "and pass the short form (HBellona) back to the tool. The short forms are "
            "for the tool only -- never show them to a member."
        ) from None


#: Anything shaped like a canonical boss token; the table decides which of them
#: actually is one.
_TOKENISH_RE = re.compile(r"\b[A-Za-z][A-Za-z0-9]+\b")


def _spell_out(bot: Any, message: str) -> str:
    """Annotate the boss tokens in a refusal with the words a member would say.

    Both forms, deliberately. The model has to do two things with this sentence
    -- ask a person which difficulty they meant, and then call the tool again --
    and it can only do the first from "Hard Bellona" and the second from
    "HBellona".
    """

    def annotate(match: re.Match) -> str:
        token = match.group(0)
        if bot.bosses.detail(token) is None:
            return token
        return f"{token} ({formatting.boss_label(token)})"

    return _TOKENISH_RE.sub(annotate, message)


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


def _without_the_bot(ctx: ToolContext, text: str) -> tuple[str, bool]:
    """Drop every reference to the bot from a participants field; say if there was one.

    The trigger mention is part of the message the model is reading, and it duly
    passed it straight back as a participant. ``validate_participants`` then
    refused with "not in the bossing role: user 5555 (…)", and the model relayed
    that to the channel in the first person -- "I'm not in the bossing role" --
    which is both untrue and baffling to read.

    The bot is not a member of anything. It is the thing being spoken to, so its
    id, its mention markup and its name are stripped before anybody tries to
    resolve them to a person.

    The second half of the return value is why the caller cares: a member who
    writes "@bot schedule it for me and @kanon" is on that run, and the model
    copies the trigger mention across in place of the word "me".
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
    return cleaned, cleaned != text


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

    A stripped bot reference then puts the asker *back*. Live: "schedule it for
    me and @kanon" reached the tool as the trigger mention plus kanon, the bot
    was stripped, kanon survived -- so the empty-field default never fired and
    the card went up for kanon alone, with the bot narrated as the other
    attendee. Whatever the model meant by naming the bot, it was reading a
    sentence in which the asker had put themselves on the run. Somebody who
    genuinely wants out of a run they asked for can ❌ the card, which is a much
    smaller failure than quietly dropping the person who asked.
    """
    raw = ", ".join(str(t) for t in text) if isinstance(text, (list, tuple)) else str(text or "")
    without_bot, named_the_bot = _without_the_bot(ctx, raw)
    raw = _FIRST_PERSON_RE.sub(f"<@{ctx.author_id}>", without_bot)
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
        named = service.validate_participants(ctx.bot, people)
    except BadRequest as exc:
        raise ToolError(f"{exc.message}. Ask them who should be on it.") from None
    if named_the_bot and ctx.author_id not in named:
        # Added after validation, exactly as the empty-field default is: the
        # asker is on this run because they asked for it, not because the model
        # named them, and the same is true whether or not they hold a role.
        named.insert(0, ctx.author_id)
    return named


#: What a model writes when it means yes. The parameter is declared boolean, but
#: a small model routinely answers a boolean with the word -- and `bool("false")`
#: is True, which would silently turn a one-off into a standing commitment.
_TRUTHY = frozenset({"true", "yes", "y", "1", "weekly", "recurring", "fixed"})


def _is_true(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    return str(value or "").strip().lower() in _TRUTHY


async def _propose_add(ctx: ToolContext, args: dict) -> str:
    """Post a card proposing a run that does not exist yet.

    The `add` kind and its commit handler are the extractor's, already used for
    "wanna do trio ncarling saturday?" read out of chat. Nothing new is written
    here: this only builds the same amendment from a sentence addressed to the
    bot instead of one overheard in a party channel.

    ``weekly`` swaps the kind for `fix`, which is the same card `/fixed add`
    would produce. It is off unless the member actually said the run repeats:
    the two are a fortnight apart in consequence, and a one-off proposed as a
    weekly commits the party to every Tuesday until somebody notices.
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
    if _is_true(args.get("weekly")):
        return await _propose_weekly(ctx, bosses, at, people)
    return await _propose(
        ctx,
        kind="add",
        run=None,
        at=at,
        bosses=bosses,
        participants=people,
        summary=(
            f"new run: {formatting.boss_labels(bosses)} "
            f"on {at.astimezone(ctx.bot.tz):%a %d %b %H:%M}"
        ),
    )


async def _propose_weekly(
    ctx: ToolContext, bosses: list[str], at: datetime, people: list[str]
) -> str:
    """The recurring half of ``propose_add``: a `fix` card, as `/fixed add` posts.

    The day and time the member said are a dated instant by the time
    :func:`bot.api.service.parse_when` is done with them, and a baseline is a
    weekday plus an HH:MM -- so they are reduced here exactly as
    :func:`bot.extract.pipeline._fix_payload` reduces the extractor's, in the
    guild's timezone, because "every Tuesday 21:30" is a local claim.

    The payload is built from that instant alone. Nothing the model wrote in the
    arguments reaches it, which is what stops a `weekly` call from carrying, say,
    an ``op: remove`` that would retire somebody else's baseline on the ✅.
    """
    local = at.astimezone(ctx.bot.tz)
    when = f"{WEEKDAY_NAMES[local.weekday()]} {local:%H:%M}"
    return await _propose(
        ctx,
        kind="fix",
        run=None,
        at=at,
        bosses=bosses,
        participants=people,
        payload={"weekday": local.weekday(), "time": local.strftime("%H:%M")},
        summary=f"new weekly: {formatting.boss_labels(bosses)} every {when}",
    )


def _fixed_line(bot: Any, fixed: dict) -> str:
    """One weekly timing, with enough on it to be told from another.

    The party is named as well as the night because a guild has several weekly
    timings and the same boss can be on twice a week: "every Mon 21:30 Hard Star"
    is not, on its own, a question a member can answer. The short id leads, since
    passing it back is how the model ends the ambiguity for good.
    """
    party = ", ".join(service.member_name(bot, uid) for uid in fixed["participants"])
    return (
        f"[{short_id(fixed['id'])}] every {WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']} "
        f"{formatting.boss_labels(fixed['bosses'])}" + (f" with {party}" if party else "")
    )


def _fixed_when(fixed: dict) -> str:
    return f"{WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']}"


def _no_weekly_for(bot: Any, text: str, candidates: list[dict]) -> None:
    """Refuse a query whose boss is on no weekly schedule at all; else say nothing.

    The live failure: a member with a one-off Hard Jupiter run on Tuesday asked
    to move it to 23:00 "and make it run for every week". No Jupiter weekly
    exists, so the boss search above found nothing, the day token "tue" survived,
    and the tool answered with three unrelated Tuesday weeklies -- Seren,
    Carling, Kalos -- which the model duly relayed as "which Hard Jupiter weekly
    are you tweaking?". Nonsense to read, and one ✅ away from moving a party's
    night on the strength of a shared weekday.

    So a query that names a boss is answered about that boss or not at all. The
    refusal also points at the tool the member actually wanted, because "make
    this run weekly" is a *creation* with an adoption behind it
    (:func:`bot.materialise.materialise_week`), not a change to something that
    exists.

    Silent when the query named no boss -- day-only and vague queries still
    disambiguate among candidates below -- and silent when the named boss does
    have a weekly that the search merely failed to spell the same way, where the
    ordinary refusals say something truer than this one would.
    """
    named = bot.bosses.names_in(text)
    if not named:
        return
    scheduled = {
        short
        for fixed in candidates
        for token in fixed["bosses"]
        for short in bot.bosses.names_in(token)
    }
    missing = [short for short in named if short not in scheduled]
    if len(missing) != len(named):
        return
    label = ", ".join(bot.bosses.bosses[short].full for short in missing)
    raise ToolError(
        f"No weekly timing for {label} exists, so there is nothing to change. If they are "
        "asking for an existing one-off run to happen every week, that is propose_add with "
        "weekly = true -- the scheduler folds this week's run into the new weekly instead of "
        "leaving a duplicate beside it. If they meant a different boss's weekly, ask them "
        "which one; do not offer them somebody else's."
    )


def resolve_fixed(bot: Any, query: str) -> dict:
    """A weekly timing from a short id, or a boss name and (optionally) a day.

    The same shape as :func:`resolve_run` and for the same reason: ids first,
    through the service layer's prefix resolution, then a boss/day search that
    refuses rather than guesses. Removing the wrong baseline is worse than
    cancelling the wrong night -- nothing re-materialises it, and changing the
    wrong one moves a party's evening without anybody having asked.

    A query that still matches several timings ends here, in a refusal that names
    all of them (:func:`_fixed_line`) and tells the model to ask. There is no
    first-match fallback and there must not be one: a guild runs several weekly
    timings, sometimes the same boss twice a week, and picking one of those is
    picking somebody's night at random.

    A query that names a boss with no weekly at all ends here too, and earlier:
    see :func:`_no_weekly_for`. The day-token fallback below is for queries that
    never named a boss, and letting a named one reach it is how "hard jupiter
    tue" came back as three other parties' Tuesday nights.
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
    if not by_boss:
        _no_weekly_for(bot, text, candidates)
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
        raise ToolError(
            f"`{text}` matches more than one weekly timing. Ask which one they mean -- name "
            "the boss and the night each one is on, and do not pick one yourself. Their "
            "answer comes back as a normal message and you can call the tool again then, "
            f"with the short id in brackets if that is clearer: {listed}"
        )
    return matches[0]


async def _propose_remove_fixed(ctx: ToolContext, args: dict) -> str:
    """Post a card proposing that a weekly timing stops existing.

    The opposite of the extractor's `fix`, and the thing that was missing when
    somebody said "remove the fixed run" live: the pilot cancelled the two runs
    it had already produced, and the baseline quietly went on producing more.
    """
    fixed = resolve_fixed(ctx.bot, str(args.get("query") or ""))
    _require_authority(ctx, fixed=fixed)
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
        summary=(
            f"stop scheduling {formatting.boss_labels(fixed['bosses'])} every {_fixed_when(fixed)}"
        ),
    )


def _new_slot(args: dict, fixed: dict) -> tuple[int, str]:
    """The weekday and HH:MM the timing should have, keeping whatever is not changing.

    Each half is optional because half a change is the common request -- "same
    night, half an hour later" -- and defaulting the other half to the row's own
    value is the only reading of that sentence. Both are read through the
    parsers `/fixed edit` uses, so "weds" and "9:30pm" mean here what they mean
    in the slash command.
    """
    raw_day = str(args.get("day") or "").strip()
    raw_time = str(args.get("time") or "").strip()
    try:
        weekday = parse_weekday(raw_day) if raw_day else int(fixed["weekday"])
    except ValueError as exc:
        raise ToolError(f"{exc}. Ask them which day of the week it should be.") from None
    try:
        hhmm = parse_hhmm(raw_time).strftime("%H:%M") if raw_time else str(fixed["time"])
    except ValueError as exc:
        raise ToolError(f"{exc}. Ask them what time it should start.") from None
    return weekday, hhmm


def _new_party(ctx: ToolContext, value: Any) -> list[str] | None:
    """The party the timing should have, or ``None`` for "leave it alone".

    The empty field means the opposite of what it means to ``propose_add``, where
    it is the asker. Here the party already exists, and a model that omits the
    argument -- or copies the trigger mention into it, which is what
    :func:`_without_the_bot` exists to catch -- must not thereby cut a weekly
    timing down to whoever happened to ask about it.
    """
    raw = ", ".join(str(t) for t in value) if isinstance(value, (list, tuple)) else str(value or "")
    without_bot, _ = _without_the_bot(ctx, raw)
    if not without_bot.strip():
        return None
    return _validate_participants(ctx, raw)


async def _propose_change_fixed(ctx: ToolContext, args: dict) -> str:
    """Post a card proposing that an existing weekly timing changes.

    The gap this closes, live: asked to "update this hard limbo timing to 23:30",
    the model had only ``propose_move`` -- which moves the one night already
    materialised -- and ``propose_add``. It tried the first, was told that was
    not what was meant, and then created a *second* weekly at 23:30 beside the
    21:30 one: a duplicate nothing removed, and the other two members left off it.

    Which row is changed comes from :func:`resolve_fixed` and from nothing else
    the model wrote, exactly as in :func:`_propose_remove_fixed`; the arguments
    only ever say what the new night or party should be, and are re-validated
    here before either reaches the payload.
    """
    fixed = resolve_fixed(ctx.bot, str(args.get("query") or ""))
    _require_authority(ctx, fixed=fixed)
    weekday, hhmm = _new_slot(args, fixed)
    party = _new_party(ctx, args.get("participants"))

    people = [str(p) for p in fixed["participants"]]
    moves = (weekday, hhmm) != (int(fixed["weekday"]), str(fixed["time"]))
    reparties = party is not None and party != people
    if not moves and not reparties:
        raise ToolError(
            f"Nothing about the weekly {formatting.boss_labels(fixed['bosses'])} "
            f"({_fixed_when(fixed)}) would change. Ask them what should change about it -- "
            "the day, the time, or who is on it."
        )

    was, becomes = _fixed_when(fixed), f"{WEEKDAY_NAMES[weekday]} {hhmm}"
    payload: dict[str, Any] = {
        "op": FIX_EDIT,
        "fixed_run_id": fixed["id"],
        # What it is now, so the card and the ✅ handler can both say which night
        # is being changed without looking the row up again.
        "weekly_when": was,
    }
    changes = []
    if moves:
        payload["weekday"] = weekday
        payload["time"] = hhmm
        changes.append(f"{was} → {becomes}")
    if reparties:
        payload["participants"] = party
        changes.append(f"party {_names(ctx, people)} → {_names(ctx, party)}")
    return await _propose(
        ctx,
        kind="fix",
        run=None,
        at=None,
        bosses=list(fixed["bosses"]),
        # The party it has *now*, not the one proposed: this is the row
        # `bot.extract.commit.may_commit` reads when a card names no run, so it
        # decides who may press ✅ -- and that is the people the timing already
        # affects, never somebody the call has just written onto it.
        participants=people,
        week=service.week_for(ctx.bot, "this"),
        payload=payload,
        summary=(
            f"change the weekly {formatting.boss_labels(fixed['bosses'])}: " + "; ".join(changes)
        ),
    )


async def _propose_cancel(ctx: ToolContext, args: dict) -> str:
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    _require_authority(ctx, run=run)
    if run["status"] == "cancelled":
        raise ToolError("That run is already cancelled.")
    return await _propose(
        ctx,
        kind="cancel",
        run=run,
        at=None,
        summary=f"cancel {formatting.boss_labels(run['bosses'])}",
    )


async def _propose_rsvp(ctx: ToolContext, args: dict) -> str:
    """Record the *asker's* answer. The author id comes from Discord, never the model.

    ``propose_rsvp`` takes no user argument at all: there is nothing for a
    message to say that could point the answer at somebody else. A model told
    "say no for kanon" can at most produce a card recording the *speaker's* no,
    which is visible on the card and reversible with a ❌.

    The participant check below survives :func:`_require_authority` rather than
    being folded into it: an admin passes the authority check for any run, and
    an admin who is not on a run still has nothing to answer for it.
    """
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    _require_authority(ctx, run=run)
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
    "propose_change_fixed": _propose_change_fixed,
    "propose_move": _propose_move,
    "propose_cancel": _propose_cancel,
    "propose_rsvp": _propose_rsvp,
}


def read_tools() -> list[dict]:
    """:data:`TOOLS` with the six ``propose_*`` schemas taken out.

    What a read-only turn (:attr:`ToolContext.read_only`) is offered. Deriving
    it from :data:`TOOLS` rather than listing the read schemas again means a
    tool added tomorrow is write-shaped here unless somebody puts it in
    :data:`_READ`, which is the safe way round to be wrong.
    """
    return [t for t in TOOLS if t["function"]["name"] in _READ]


#: What a read-only turn tells the model when it reaches for a card anyway.
#: Phrased as the thing to do instead, for the reason :data:`UNKNOWN_TOOL` is:
#: a bare refusal makes a small model try the same call again.
READ_ONLY_TURN = (
    "You cannot post a card in this message. Ask them in words what it should be "
    "instead, and stop there -- their answer comes back to you as a normal "
    "message and you can post the card then."
)


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

    if ctx.read_only and name in _WRITE:
        # Structural, not advisory: the schemas were withheld from this turn
        # too, but a model that names one from memory must be refused by the
        # dispatcher rather than trusted not to try.
        log.info("chat: %s is not available in a read-only turn", name)
        return done(READ_ONLY_TURN, False, REFUSED)

    handler = _READ.get(name) or _WRITE.get(name)
    if handler is None:
        log.warning("chat: the model asked for an unknown tool %r", name)
        return done(UNKNOWN_TOOL.format(name=name, known=", ".join(tool_names())), False, UNKNOWN)
    try:
        output = await handler(ctx, args) if name in _WRITE else handler(ctx, args)
        log.debug("chat: %s response %r", name, output)
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
