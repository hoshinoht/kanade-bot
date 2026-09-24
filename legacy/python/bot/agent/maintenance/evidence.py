"""Bounded, advisory Discord history listing for runtime attempt recovery.

The listing only proposes candidates to an operator. Zero, one, or several
candidates never decide an outcome, and any fetch failure is reported as a
failure rather than as absence.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta

import discord

from bot.infrastructure.maintenance.delivery.model import (
    Destination,
    ObservedAttachment,
    ObservedMessage,
    _snowflake,
)
from bot.infrastructure.maintenance.recovery.model import (
    EVIDENCE_CLOCK_SKEW,
    RecoveryObservation,
)

MAX_HISTORY_MESSAGES = 100
MAX_HISTORY_WINDOW = timedelta(minutes=15)


class EvidenceLookupError(RuntimeError):
    """History could not be read completely; this is not evidence that nothing was sent."""


@dataclass(frozen=True, slots=True)
class CandidateListing:
    candidates: tuple[RecoveryObservation, ...]
    scanned: int
    #: The message cap was reached, so later messages in the window were not read.
    truncated: bool
    after: datetime
    before: datetime


async def list_candidates(
    channel: object,
    destination: Destination,
    *,
    intended_at: datetime,
    bot_author_id: str | int,
    window: timedelta = MAX_HISTORY_WINDOW,
    limit: int = MAX_HISTORY_MESSAGES,
) -> CandidateListing:
    """List bot-authored messages near one attempt's intent in its exact destination."""
    if not isinstance(destination, Destination):
        raise ValueError("candidate listing requires the attempt destination")
    if intended_at.tzinfo is None or intended_at.utcoffset() is None:
        raise ValueError("intent time must be timezone-aware")
    if not timedelta(0) < window <= MAX_HISTORY_WINDOW:
        raise ValueError("history window must be positive and at most 15 minutes")
    if (
        isinstance(limit, bool)
        or not isinstance(limit, int)
        or not 0 < limit <= MAX_HISTORY_MESSAGES
    ):
        raise ValueError("history limit must be between 1 and 100 messages")
    author_id = _snowflake(bot_author_id, "configured bot author id")
    after = intended_at - EVIDENCE_CLOCK_SKEW
    before = intended_at + window

    candidates: list[RecoveryObservation] = []
    scanned = 0
    try:
        async for message in channel.history(  # type: ignore[attr-defined]
            limit=limit, after=after, before=before, oldest_first=True
        ):
            scanned += 1
            if scanned > limit:
                raise EvidenceLookupError("history returned more messages than requested")
            if str(getattr(message.author, "id", "")) != author_id:
                continue
            observation = _observation(message, destination)
            if not _same_destination(destination, observation.destination):
                raise EvidenceLookupError("history returned a message from another destination")
            candidates.append(observation)
    except EvidenceLookupError:
        raise
    except discord.HTTPException as exc:
        # Forbidden and NotFound are HTTPException subclasses; none prove absence.
        raise EvidenceLookupError(
            f"Discord history lookup failed ({type(exc).__name__}); outcome remains unknown"
        ) from exc
    except (AttributeError, TypeError, ValueError) as exc:
        raise EvidenceLookupError("Discord history returned an unreadable message") from exc
    return CandidateListing(tuple(candidates), scanned, scanned >= limit, after, before)


def _same_destination(requested: Destination, actual: Destination) -> bool:
    if requested.kind is not actual.kind or requested.guild_id != actual.guild_id:
        return False
    if requested.guild_id is not None:
        return requested.channel_id == actual.channel_id
    return requested.recipient_id == actual.recipient_id and (
        requested.channel_id is None or requested.channel_id == actual.channel_id
    )


def _observation(message: object, requested: Destination) -> RecoveryObservation:
    channel = message.channel  # type: ignore[attr-defined]
    guild = getattr(channel, "guild", None)
    if requested.guild_id is not None:
        destination = Destination.channel(guild.id, channel.id)  # type: ignore[union-attr]
    else:
        recipient = getattr(channel, "recipient", None)
        destination = Destination.dm(recipient.id, channel_id=channel.id)  # type: ignore[union-attr]
    reference = getattr(message, "reference", None)
    observed = ObservedMessage(
        content=message.content,  # type: ignore[attr-defined]
        embeds=tuple(embed.to_dict() for embed in message.embeds),  # type: ignore[attr-defined]
        user_mentions=tuple(str(user.id) for user in message.mentions),  # type: ignore[attr-defined]
        role_mentions=tuple(str(role.id) for role in message.role_mentions),  # type: ignore[attr-defined]
        everyone_mentioned=bool(message.mention_everyone),  # type: ignore[attr-defined]
        replied_user=False,
        reference=getattr(reference, "message_id", None),
        attachments=tuple(
            ObservedAttachment(item.filename, item.size, url=getattr(item, "url", None))
            for item in message.attachments  # type: ignore[attr-defined]
        ),
    )
    components = tuple(
        f"component:{index}" for index, _ in enumerate(getattr(message, "components", None) or ())
    )
    return RecoveryObservation(
        destination,
        str(message.id),  # type: ignore[attr-defined]
        str(message.author.id),  # type: ignore[attr-defined]
        message.created_at,  # type: ignore[attr-defined]
        observed,
        components,
    )
