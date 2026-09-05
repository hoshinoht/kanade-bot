"""Build and post extractor-owned proposal cards for chatbot write tools."""

from __future__ import annotations

from collections.abc import Sequence
from datetime import datetime

from bot.agent import formatting
from bot.domain.ids import short_id
from bot.domain.weeks import WEEKDAY_NAMES, week_start

from ...api import service
from ...extract.commit import FIX_EDIT
from ...extract.pipeline import Planned
from ...extract.resolve import Resolved
from ...extract.schema import Amendment
from .contracts import ToolContext, ToolError


async def propose(
    ctx: ToolContext,
    *,
    kind: str,
    run: dict | None,
    at: datetime | None,
    summary: str,
    bosses: Sequence[str] | None = None,
    rsvp: str | None = None,
    participants: list[str] | None = None,
    payload: dict | None = None,
    week: datetime | None = None,
) -> str:
    """Create an amendment row and card through the extractor's ``apply_plan`` path."""
    bot = ctx.bot
    local = at.astimezone(bot.tz) if at is not None else None
    resolved = Resolved(
        day=local.date() if local else None,
        clock=local.time() if local else None,
        at=at,
    )
    amendment = Amendment(
        kind=kind,
        bosses=list(bosses if bosses is not None else run["bosses"]),
        participants=list(participants or []),
        rsvp=rsvp,
        # Stated by a person and read back to them before anything happens, so
        # there is no uncertainty for a confidence score to express.
        confidence=1.0,
        evidence_message_ids=[str(ctx.message_id)],
    )
    if at is not None:
        week = week_start(at, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time)
    elif run is not None:
        week = run["week_start"]
    elif week is None:  # pragma: no cover - callers with no run pass one
        raise ToolError("Ask them which day and time they mean.")
    planned = Planned(
        amendment=amendment,
        resolved=resolved,
        run=run,
        summary=summary,
        payload=dict(payload or {}),
    )
    created = await bot.extractor.apply_plan(
        ctx.channel_id, [], [planned], week, summary, actor=ctx.author_id
    )
    ctx.created.extend(created)
    if not created:  # pragma: no cover - apply_plan returns a row per proposal
        raise ToolError("The card could not be created. Tell them to try again in a moment.")
    posted = [
        amendment_id
        for amendment_id in created
        if (bot.repo.get_amendment(amendment_id) or {}).get("proposal_message_id")
    ]
    if not posted:
        raise ToolError(
            "The change was recorded but the card could not be posted to the channel. "
            "Tell them to check with an admin."
        )
    ctx.posted.extend(posted)
    label, when, party = _card_facts(ctx, kind, amendment, at, run, payload)
    ids = ", ".join(short_id(amendment_id) for amendment_id in created)
    facts = [
        "Card ready -- facts for your reply, not a sentence to copy:",
        f"- bosses: {label}",
        f"- party: {party}",
        f"- cards: {ids}",
    ]
    if when:
        facts.insert(2, f"- when: {when}")
    facts.extend(
        [
            "NOT DONE - nothing has changed yet: it takes effect only when somebody "
            "reacts ✅ on it.",
            "Reply in your own voice saying the card is up and needs a ✅, keeping each "
            "fact exact. Do not copy the labels or formatting above into your reply, do "
            'not start with "Card posted:", and do not stick an emoji on a flat sentence '
            "to sound in-character. The people named above are the whole party on it -- "
            "name those and nobody else, and never say you are on a run: you are a bot "
            "and cannot go to one. Never say it is done, moved, or confirmed.",
        ]
    )
    return "\n".join(facts)


def card_when(
    ctx: ToolContext, kind: str, at: datetime | None, run: dict | None, payload: dict
) -> str:
    """The day and time a card shows, in the words the card itself uses."""
    if kind == "fix":
        if payload.get("op") == FIX_EDIT:
            return changed_when(payload)
        weekly = payload.get("weekly_when")
        if weekly:
            return f"every {weekly}"
        weekday, hhmm = payload.get("weekday"), payload.get("time")
        if weekday is not None and hhmm:
            return f"every {WEEKDAY_NAMES[int(weekday)]} {hhmm}"
    when = at if at is not None else (run["datetime"] if run is not None else None)
    return f"{when.astimezone(ctx.bot.tz):%a %d %b %H:%M}" if when is not None else ""


def changed_when(payload: dict) -> str:
    """A change-the-weekly card's night, as ``was → is``."""
    was = payload.get("weekly_when") or ""
    weekday, hhmm = payload.get("weekday"), payload.get("time")
    if weekday is not None and hhmm:
        return f"every {was} → every {WEEKDAY_NAMES[int(weekday)]} {hhmm}"
    return f"every {was} (same night)"


def _card_facts(
    ctx: ToolContext,
    kind: str,
    amendment: Amendment,
    at: datetime | None,
    run: dict | None,
    payload: dict | None,
) -> tuple[str, str, str]:
    """Plain boss/when/party facts backing the card line and the tool reply."""
    data = dict(payload or {})
    people = list(amendment.participants) or (list(run["participants"]) if run else [])
    party = names(ctx, people) or "nobody yet"
    # A card that changes the party says both sides of it, for the same reason
    # the night is said both ways: the row's own participants are the party as it
    # stands, and reading that back as "the party on it" would be the old one.
    joining = (
        [str(uid) for uid in data.get("participants") or []] if data.get("op") == FIX_EDIT else []
    )
    if joining:
        party = f"{party} → {names(ctx, joining)}"
    return formatting.boss_labels(amendment.bosses), card_when(ctx, kind, at, run, data), party


def card_text(
    ctx: ToolContext,
    kind: str,
    amendment: Amendment,
    at: datetime | None,
    run: dict | None,
    payload: dict | None,
) -> str:
    """What the card says, in the channel's own formatting."""
    bosses, when, party = _card_facts(ctx, kind, amendment, at, run, payload)
    return f"**{bosses}**{f' *{when}*' if when else ''} — {party}"


def names(ctx: ToolContext, user_ids: Sequence[str]) -> str:
    """A party by display name, never as mentions -- the model must not learn to ping."""
    return ", ".join(service.member_name(ctx.bot, user_id) for user_id in user_ids)
