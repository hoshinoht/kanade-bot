"""The model-callable, read-only local boss strategy lookup."""

from __future__ import annotations

from bot.domain.boss_knowledge import BossKnowledgeError
from bot.domain.bosses import BossParseError, BossReference

from .contracts import ToolContext, ToolError


def _difficulty(ctx: ToolContext, value: object) -> str | None:
    """Normalize one explicitly requested difficulty without choosing one."""
    if value is None:
        return None
    if not isinstance(value, str) or not value.strip():
        raise ToolError("Difficulty must be a prefix or full difficulty name.")
    key = value.strip().lower()
    if key in ctx.bot.bosses.difficulties:
        return key
    words = {name.lower(): letter for letter, name in ctx.bot.bosses.difficulties.items()}
    if key in words:
        return words[key]
    choices = ", ".join(ctx.bot.bosses.difficulties.values())
    raise ToolError(f"Unknown difficulty `{value}`. Use one of: {choices}.")


def handle(ctx: ToolContext, args: dict) -> str:
    """Render checked-in strategy Markdown for one unambiguous boss reference."""
    raw_boss = args.get("boss")
    if not isinstance(raw_boss, str) or not raw_boss.strip():
        raise ToolError("Ask which boss they want strategy for.")
    try:
        reference = ctx.bot.bosses.resolve_reference(raw_boss)
    except BossParseError as exc:
        raise ToolError(str(exc)) from None

    explicit = _difficulty(ctx, args.get("difficulty"))
    if (
        explicit is not None
        and reference.difficulty is not None
        and explicit != reference.difficulty
    ):
        raise ToolError(
            "conflicting difficulties: "
            f"{ctx.bot.bosses.difficulty_name(reference.difficulty)} and "
            f"{ctx.bot.bosses.difficulty_name(explicit)}"
        )
    difficulty = explicit if explicit is not None else reference.difficulty
    boss = ctx.bot.bosses.bosses[reference.short]
    if difficulty is not None and difficulty not in boss.difficulties:
        raise ToolError(
            f"{boss.full} has no {ctx.bot.bosses.difficulty_name(difficulty)} difficulty - "
            f"available forms are {ctx.bot.bosses.valid_forms(reference.short)}."
        )

    knowledge = getattr(ctx.bot, "boss_knowledge", None)
    if knowledge is None:
        raise ToolError("Boss strategy knowledge is unavailable right now.")
    try:
        return knowledge.render(BossReference(reference.short, difficulty))
    except BossKnowledgeError:
        raise ToolError(f"No checked-in strategy guide is available for {boss.full}.") from None
