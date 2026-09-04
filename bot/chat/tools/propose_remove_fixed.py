"""The model-callable card proposal for removing a recurring weekly timing."""

from __future__ import annotations

from bot.agent import formatting

from ...api import service
from ...extract.commit import FIX_REMOVE
from .authority import require_authority
from .contracts import ToolContext
from .proposals import propose
from .resolution import fixed_when, resolve_fixed


async def handle(ctx: ToolContext, args: dict) -> str:
    """Propose retiring a weekly baseline so future weeks stop materialising it."""
    fixed = resolve_fixed(ctx.bot, str(args.get("query") or ""))
    require_authority(ctx, fixed=fixed)
    return await propose(
        ctx,
        kind="fix",
        run=None,
        at=None,
        bosses=list(fixed["bosses"]),
        participants=[str(participant) for participant in fixed["participants"]],
        week=service.week_for(ctx.bot, "this"),
        payload={
            "op": FIX_REMOVE,
            "fixed_run_id": fixed["id"],
            # Carried so the card can say which night it is retiring without
            # looking the row up again -- by ✅ time it may be gone.
            "weekly_when": fixed_when(fixed),
        },
        summary=(
            f"stop scheduling {formatting.boss_labels(fixed['bosses'])} every {fixed_when(fixed)}"
        ),
    )
