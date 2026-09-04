"""The model-callable lookup for one run in full."""

from __future__ import annotations

from .contracts import ToolContext
from .rendering import run_detail
from .resolution import resolve_run


def handle(ctx: ToolContext, args: dict) -> str:
    """Return the selected run's full schedule view."""
    return run_detail(ctx.bot, resolve_run(ctx.bot, str(args.get("query") or "")))
