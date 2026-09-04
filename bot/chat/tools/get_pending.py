"""The model-callable lookup for proposal cards awaiting confirmation."""

from __future__ import annotations

from bot.agent import formatting

from ...api import service
from .contracts import MAX_RUNS, ToolContext


def handle(ctx: ToolContext, args: dict) -> str:
    """List still-open proposal cards, or say when there are none."""
    open_cards = service.pending(ctx.bot)
    if not open_cards:
        return "There are no proposal cards waiting."
    lines = [
        f"`[{card['short_id']}]` **{card['kind_label']}** "
        f"**{formatting.boss_labels(card['bosses'])}** → *{card['when']}*"
        for card in open_cards[:MAX_RUNS]
    ]
    return "\n".join(["**Waiting for a ✅**", "", *lines])
