"""The model-callable card proposal for a one-off or recurring new run."""

from __future__ import annotations

from datetime import datetime

from bot.agent import formatting
from bot.domain.weeks import WEEKDAY_NAMES

from ...api import service
from ...api.errors import BadRequest
from .clock import utcnow
from .contracts import ToolContext, ToolError
from .participants import is_true, validate_bosses, validate_participants
from .proposals import propose


async def handle(ctx: ToolContext, args: dict) -> str:
    """Propose a new run, making it recurring only when ``weekly`` is true."""
    bosses = validate_bosses(ctx, str(args.get("boss") or ""))
    raw = str(args.get("when") or "").strip()
    if not raw:
        raise ToolError("Ask them what day and time the run should be.")
    try:
        at = service.parse_when(ctx.bot, raw)
    except BadRequest as exc:
        raise ToolError(f"{exc.message}. Ask them for the day and time again.") from None
    if at <= utcnow():
        raise ToolError(f"`{raw}` is in the past. Ask them which day they mean.")
    people = validate_participants(ctx, args.get("participants"))
    if is_true(args.get("weekly")):
        return await _propose_weekly(ctx, bosses, at, people)
    return await propose(
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
    """Build the recurring half of ``propose_add`` as the extractor's ``fix`` card."""
    local = at.astimezone(ctx.bot.tz)
    when = f"{WEEKDAY_NAMES[local.weekday()]} {local:%H:%M}"
    return await propose(
        ctx,
        kind="fix",
        run=None,
        at=at,
        bosses=bosses,
        participants=people,
        payload={"weekday": local.weekday(), "time": local.strftime("%H:%M")},
        summary=f"new weekly: {formatting.boss_labels(bosses)} every {when}",
    )
