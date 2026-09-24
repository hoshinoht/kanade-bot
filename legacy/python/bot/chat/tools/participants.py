"""Validation for boss tokens and parties supplied by an untrusted model."""

from __future__ import annotations

import re
from typing import Any

from bot.agent import formatting
from bot.agent.util import resolve_participant_text

from ...api import service
from ...api.errors import BadRequest
from .contracts import ToolContext, ToolError


def validate_bosses(ctx: ToolContext, text: str) -> list[str]:
    """Return difficulty-qualified boss tokens or a model-readable refusal."""
    if not (text or "").strip():
        raise ToolError("Ask them which boss they mean.")
    try:
        return service.validate_bosses(ctx.bot, text)
    except BadRequest as exc:
        raise ToolError(
            f"{_spell_out(ctx.bot, exc.message)}. Ask them which one they mean -- do not "
            "choose a difficulty for them. Ask in words ('Easy, Normal or Hard Bellona?') "
            "and pass the short form (HBellona) back to the tool. The short forms are "
            "for the tool only -- never show them to a member."
        ) from None


#: Anything shaped like a canonical boss token; the table decides which of them
#: actually is one.
_TOKENISH_RE = re.compile(r"\b[A-Za-z][A-Za-z0-9]+\b")


def _spell_out(bot: Any, message: str) -> str:
    """Annotate boss tokens in a refusal with the words a member would say."""

    def annotate(match: re.Match) -> str:
        token = match.group(0)
        if bot.bosses.detail(token) is None:
            return token
        return f"{token} ({formatting.boss_label(token)})"

    return _TOKENISH_RE.sub(annotate, message)


#: "put me on it" -- the one name the model can be certain of, and the one the
#: roster cannot resolve. Word-bounded so `i` does not eat the `i` in a nickname.
_FIRST_PERSON_RE = re.compile(r"\b(?:me|myself|i)\b", re.IGNORECASE)

#: Words that join names rather than being one. ``resolve_participant_text`` was
#: written for a slash-command field where people type only names, so it reads
#: "me and kanon" as three of them and refuses over "and". These are forgiven
#: only when they matched *nobody*, so a member actually nicknamed "Us" still
#: resolves to themselves.
_JOINING_WORDS = frozenset(
    {
        "a",
        "add",
        "along",
        "also",
        "and",
        "both",
        "for",
        "it",
        "just",
        "me",
        "on",
        "party",
        "please",
        "plus",
        "run",
        "team",
        "the",
        "then",
        "too",
        "us",
        "with",
        "&",
        "+",
    }
)


def _without_the_bot(ctx: ToolContext, text: str) -> tuple[str, bool]:
    """Drop bot references from a party field and report whether one was present."""
    user = getattr(ctx.bot, "user", None)
    cleaned = text
    bot_id = str(getattr(user, "id", "") or "")
    if bot_id:
        cleaned = re.sub(rf"<@!?{re.escape(bot_id)}>", " ", cleaned)
        cleaned = re.sub(rf"\b{re.escape(bot_id)}\b", " ", cleaned)
    for name in {getattr(user, "name", ""), getattr(user, "display_name", "")}:
        if name:
            cleaned = re.sub(rf"\b{re.escape(str(name))}\b", " ", cleaned, flags=re.IGNORECASE)
    return cleaned, cleaned != text


def validate_participants(ctx: ToolContext, text: Any) -> list[str]:
    """Resolve a new party, defaulting to the message author and never the model."""
    raw = (
        ", ".join(str(term) for term in text)
        if isinstance(text, (list, tuple))
        else str(text or "")
    )
    without_bot, named_the_bot = _without_the_bot(ctx, raw)
    raw = _FIRST_PERSON_RE.sub(f"<@{ctx.author_id}>", without_bot)
    if not raw.strip():
        # Empty to begin with, or nothing left once the bot was removed -- which
        # is what "@YuukiSakuna schedule a run" looks like by the time the model
        # has copied the trigger mention into the participants field.
        return [ctx.author_id]
    resolution = resolve_participant_text(raw, ctx.bot.repo.list_members())
    strangers = [word for word in resolution.unknown if word.lower() not in _JOINING_WORDS]
    if strangers:
        raise ToolError(
            f"Nobody on the roster matches {', '.join(strangers)}. "
            "Ask them who should be on it, or leave it as just them."
        )
    if resolution.ambiguous:
        options = "; ".join(
            f"{key}: {', '.join(value)}" for key, value in resolution.ambiguous.items()
        )
        raise ToolError(f"Ask them which they mean -- {options}.")
    # Belt and braces: a bare id that survived the strip above is still not a
    # person, and must never reach `validate_participants` to be reported as a
    # member who lacks a role.
    bot_id = str(getattr(getattr(ctx.bot, "user", None), "id", "") or "")
    people = [uid for uid in resolution.ids if uid != bot_id]
    if not people:
        return [ctx.author_id]
    try:
        named = service.validate_participants(ctx.bot, people)
    except BadRequest as exc:
        raise ToolError(f"{exc.message}. Ask them who should be on it.") from None
    if named_the_bot and ctx.author_id not in named:
        # Added after validation, exactly as the empty-field default is: the
        # asker is on this run because they asked for it, not because the model
        # named them, and the same is true whether or not they hold a role.
        named.insert(0, ctx.author_id)
    return named


#: What a model writes when it means yes. The parameter is declared boolean, but
#: a small model routinely answers a boolean with the word -- and `bool("false")`
#: is True, which would silently turn a one-off into a standing commitment.
_TRUTHY = frozenset({"true", "yes", "y", "1", "weekly", "recurring", "fixed"})


def is_true(value: Any) -> bool:
    """Interpret the model's tolerant boolean spelling for ``weekly``."""
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    return str(value or "").strip().lower() in _TRUTHY


def new_party(ctx: ToolContext, value: Any) -> list[str] | None:
    """Resolve a replacement fixed-run party, or ``None`` to keep the current one."""
    raw = (
        ", ".join(str(term) for term in value)
        if isinstance(value, (list, tuple))
        else str(value or "")
    )
    without_bot, _ = _without_the_bot(ctx, raw)
    if not without_bot.strip():
        return None
    return validate_participants(ctx, raw)
