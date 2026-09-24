"""The model-callable card proposal for the asking member's RSVP."""

from __future__ import annotations

from bot.domain.ids import short_id

from ...api import service
from .authority import require_authority
from .contracts import ToolContext, ToolError
from .proposals import propose
from .resolution import resolve_run


async def handle(ctx: ToolContext, args: dict) -> str:
    """Propose only the message author's yes/no RSVP for one run."""
    run = resolve_run(ctx.bot, str(args.get("run_query") or ""))
    require_authority(ctx, run=run)
    answer = str(args.get("answer") or "").strip().lower()
    if answer not in ("yes", "no"):
        raise ToolError("answer must be 'yes' or 'no'. Ask them whether they can make it.")
    if ctx.author_id not in run["participants"]:
        raise ToolError(
            f"They are not on run {short_id(run['id'])}, so they have nothing to answer. "
            "Only somebody on a run can RSVP for it."
        )
    return await propose(
        ctx,
        kind="rsvp",
        run=run,
        at=None,
        rsvp=answer,
        participants=[ctx.author_id],
        summary=f"{service.member_name(ctx.bot, ctx.author_id)} says {answer}",
    )
