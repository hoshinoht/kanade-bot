"""The model-callable recurring weekly timing lookup."""

from __future__ import annotations

from .contracts import MAX_RUNS, ToolContext
from .resolution import _fixed_line


def handle(ctx: ToolContext, args: dict) -> str:
    """List recurring weekly timings, so the model stops guessing them."""
    fixed = ctx.bot.repo.list_fixed_runs()
    if not fixed:
        return "There are no recurring weekly timings."
    lines = [_fixed_line(ctx.bot, row) for row in fixed[:MAX_RUNS]]
    more = len(fixed) - len(lines)
    return "\n".join(["**Weekly timings**", "", *lines]) + (
        f"\n*(and {more} more)*" if more > 0 else ""
    )
