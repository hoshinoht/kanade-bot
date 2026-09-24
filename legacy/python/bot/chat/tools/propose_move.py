"""The model-callable card proposal for moving one dated run."""

from __future__ import annotations

from bot.agent import formatting

from ...api import service
from ...api.errors import BadRequest
from .authority import require_authority
from .clock import utcnow
from .contracts import ToolContext, ToolError
from .proposals import propose
from .resolution import resolve_run


async def handle(ctx: ToolContext, args: dict) -> str:
    """Propose one run's new day and time without changing the schedule."""
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    require_authority(ctx, run=run)
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
    return await propose(
        ctx,
        kind="move",
        run=run,
        at=at,
        summary=(
            f"move {formatting.boss_labels(run['bosses'])} "
            f"to {at.astimezone(ctx.bot.tz):%a %d %b %H:%M}"
        ),
    )
