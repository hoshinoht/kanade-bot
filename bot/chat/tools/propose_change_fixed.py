"""The model-callable card proposal for editing a recurring weekly timing."""

from __future__ import annotations

from typing import Any

from bot.agent import formatting
from bot.domain.weeks import WEEKDAY_NAMES, parse_hhmm, parse_weekday

from ...api import service
from ...extract.commit import FIX_EDIT
from .authority import require_authority
from .contracts import ToolContext, ToolError
from .participants import new_party
from .proposals import names, propose
from .resolution import fixed_when, resolve_fixed


def _new_slot(args: dict, fixed: dict) -> tuple[int, str]:
    """Return the changed weekday and time, retaining the omitted side of a change."""
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


async def handle(ctx: ToolContext, args: dict) -> str:
    """Propose the requested weekly time, day, and/or full party replacement."""
    fixed = resolve_fixed(ctx.bot, str(args.get("query") or ""))
    require_authority(ctx, fixed=fixed)
    weekday, hhmm = _new_slot(args, fixed)
    party = new_party(ctx, args.get("participants"))

    people = [str(participant) for participant in fixed["participants"]]
    moves = (weekday, hhmm) != (int(fixed["weekday"]), str(fixed["time"]))
    reparties = party is not None and party != people
    if not moves and not reparties:
        raise ToolError(
            f"Nothing about the weekly {formatting.boss_labels(fixed['bosses'])} "
            f"({fixed_when(fixed)}) would change. Ask them what should change about it -- "
            "the day, the time, or who is on it."
        )

    was, becomes = fixed_when(fixed), f"{WEEKDAY_NAMES[weekday]} {hhmm}"
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
        changes.append(f"party {names(ctx, people)} → {names(ctx, party)}")
    return await propose(
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
