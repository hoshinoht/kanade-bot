"""Resolve untrusted run and weekly-timing descriptions without guessing."""

from __future__ import annotations

import re
from datetime import datetime, timedelta
from typing import Any

from bot.agent import formatting
from bot.domain.ids import short_id
from bot.domain.weeks import WEEKDAY_NAMES, current_week_start, next_week_start

from ...api import service
from ...api.errors import BadRequest, NotFound
from ...extract.resolve import WEEKDAY_ALIASES
from .clock import utcnow
from .contracts import MAX_RUNS, ToolError
from .rendering import run_line

_RELATIVE_DAYS = {"today": 0, "tonight": 0, "tomorrow": 1, "tmr": 1, "tmrw": 1}


def _boss_shorts(bot: Any, tokens) -> set[str]:
    """The boss keys a list of canonical tokens names, via the alias table.

    Old spellings keep working because the table does: ``hstar`` still names
    ``MaleficStar`` through the kept ``star`` alias, exactly as ``parse`` reads
    it. Matching on whole words against the short/full/token strings alone
    would lose those spellings the moment a key is renamed.
    """
    shorts: set[str] = set()
    for token in tokens:
        shorts.update(bot.bosses.names_in(token))
    return shorts


def _says(query: str, word: str) -> bool:
    """Whether ``word`` is actually written in ``query``, as a whole word."""
    return re.search(rf"\b{re.escape(word)}\b", query) is not None


def _referenced_dates(query: str, bot: Any, now: datetime) -> set:
    """The concrete local dates named by a run query, from one captured clock."""
    today = now.astimezone(bot.tz).date()
    dates = {
        today + timedelta(days=(weekday - today.weekday()) % 7)
        for word, weekday in WEEKDAY_ALIASES.items()
        if _says(query, word)
    }
    dates.update(
        today + timedelta(days=offset)
        for word, offset in _RELATIVE_DAYS.items()
        if _says(query, word)
    )
    return dates


def _names_a_day(query: str) -> bool:
    """Whether a query picks out a day at all, to decide whether to narrow by one."""
    return any(_says(query, word) for word in (*WEEKDAY_ALIASES, *_RELATIVE_DAYS))


def _listing(bot: Any, runs: list[dict], lead: str) -> str:
    return lead + " " + "; ".join(run_line(bot, run) for run in runs[:MAX_RUNS])


def resolve_run(bot: Any, query: str) -> dict:
    """Resolve a run from a short id or an unambiguous boss/day description."""
    text = (query or "").strip()
    if not text:
        raise ToolError("Ask them which run they mean -- a boss and a day, like 'hstar wednesday'.")
    try:
        return service.load_run(bot, text)
    except (NotFound, BadRequest):
        pass

    low = text.lower()
    now = utcnow()
    dates = _referenced_dates(low, bot, now)
    if len(dates) > 1:
        raise ToolError(
            f"`{text}` names more than one day. Ask them which one they mean; do not guess."
        )
    candidates = [
        run
        for start in (
            current_week_start(bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, now),
            next_week_start(bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, now),
        )
        for run in bot.repo.list_runs(week_start=start)
        if run["status"] not in ("cancelled", "done")
    ]
    named = set(bot.bosses.names_in(low))
    by_boss = [
        run
        for run in candidates
        if named and not _boss_shorts(bot, run["bosses"]).isdisjoint(named)
    ]
    named_day = bool(dates)
    if not by_boss and not named_day:
        # Nothing in the text locates a run. Falling back to "every run" here
        # would resolve gibberish to the only run in a quiet week, and a
        # `propose_cancel` built on that guess cancels the wrong night.
        raise ToolError(
            f"No run matches `{text}`. Check what is scheduled, then ask them "
            "which one they mean. Do not guess."
        )
    matches = by_boss or candidates
    if named_day:
        narrowed = [run for run in matches if run["datetime"].astimezone(bot.tz).date() in dates]
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
            f"No run matches `{text}`. Check what is scheduled, then ask them "
            "which one they mean. Do not guess."
        )
    if len(matches) > 1:
        raise ToolError(
            f"`{text}` matches more than one run. " + _listing(bot, matches, "Ask which one:")
        )
    return matches[0]


def _fixed_line(bot: Any, fixed: dict) -> str:
    """One weekly timing, with enough on it to distinguish another."""
    party = ", ".join(service.member_name(bot, uid) for uid in fixed["participants"])
    return (
        f"[{short_id(fixed['id'])}] every {WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']} "
        f"{formatting.boss_labels(fixed['bosses'])}" + (f" with {party}" if party else "")
    )


def fixed_when(fixed: dict) -> str:
    """Render a fixed timing as the weekly day and time that identify it."""
    return f"{WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']}"


def _no_weekly_for(bot: Any, text: str, candidates: list[dict]) -> None:
    """Refuse a boss query when it has no weekly timing instead of day-matching others."""
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
    """Resolve a weekly timing from a short id or unambiguous boss/day description."""
    text = (query or "").strip()
    if not text:
        raise ToolError("Ask them which weekly timing they mean -- a boss, and a day if needed.")
    try:
        return service.load_fixed(bot, text)
    except (NotFound, BadRequest):
        pass

    low = text.lower()
    candidates = bot.repo.list_fixed_runs()
    named = set(bot.bosses.names_in(low))
    by_boss = [
        fixed
        for fixed in candidates
        if named and not _boss_shorts(bot, fixed["bosses"]).isdisjoint(named)
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
        listed = "; ".join(_fixed_line(bot, fixed) for fixed in matches[:MAX_RUNS])
        raise ToolError(
            f"`{text}` matches more than one weekly timing. Ask which one they mean -- name "
            "the boss and the night each one is on, and do not pick one yourself. Their "
            "answer comes back as a normal message and you can try again then, "
            f"with the short id in brackets if that is clearer: {listed}"
        )
    return matches[0]
