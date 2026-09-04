"""The model-callable card proposal for cancelling one dated run."""

from __future__ import annotations

from bot.agent import formatting

from .authority import require_authority
from .contracts import ToolContext, ToolError
from .proposals import propose
from .resolution import resolve_run


async def handle(ctx: ToolContext, args: dict) -> str:
    """Propose a one-night cancellation without changing the run immediately."""
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    require_authority(ctx, run=run)
    if run["status"] == "cancelled":
        raise ToolError("That run is already cancelled.")
    return await propose(
        ctx,
        kind="cancel",
        run=run,
        at=None,
        summary=f"cancel {formatting.boss_labels(run['bosses'])}",
    )
