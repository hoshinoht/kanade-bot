"""The model-callable boss and difficulty catalogue."""

from __future__ import annotations

from bot.agent import formatting

from .contracts import ToolContext


def handle(ctx: ToolContext, args: dict) -> str:
    """List every guild boss in display and canonical-token vocabularies."""
    rows = [
        f"**{boss.short}** ({boss.full}, lv `{boss.level}`): "
        + ", ".join(
            f"`{boss.canonical(letter)}` = {formatting.boss_label(boss.canonical(letter))}"
            for letter in boss.difficulties
        )
        for boss in ctx.bot.bosses.ordered()
    ]
    return "\n".join(["**Bosses this guild runs**", "", *rows])
