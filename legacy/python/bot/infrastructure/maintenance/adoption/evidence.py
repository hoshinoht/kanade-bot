"""Synthetic whole-message evidence values for private adoption resolution.

Constructing these values does not prove remote authorship. J4b never fetches
Discord; only a later trusted reconciler may populate this boundary from a
complete authoritative observation.
"""

from __future__ import annotations

import re
from collections.abc import Mapping
from dataclasses import dataclass

from bot.infrastructure.maintenance.delivery.model import (
    Destination,
    ObservedAttachment,
    ObservedMessage,
)

_SNOWFLAKE = re.compile(r"[1-9][0-9]*\Z")
_SHA256 = re.compile(r"[0-9a-f]{64}\Z")


def _snowflake(value: str | int, label: str) -> str:
    if isinstance(value, bool) or not isinstance(value, (str, int)):
        raise ValueError(f"{label} must be a canonical Discord ID")
    normalized = str(value)
    if not _SNOWFLAKE.fullmatch(normalized):
        raise ValueError(f"{label} must be a canonical Discord ID")
    return normalized


@dataclass(frozen=True, slots=True)
class CompleteAttachmentEvidence:
    """Recoverable attachment metadata plus a hash of its fully read bytes."""

    name: str
    size: int
    url: str | None
    content_sha256: str

    def __post_init__(self) -> None:
        if not isinstance(self.content_sha256, str) or not _SHA256.fullmatch(self.content_sha256):
            raise ValueError("complete attachment evidence requires a lowercase content SHA-256")
        validated = ObservedAttachment(self.name, self.size, self.url, self.content_sha256)
        object.__setattr__(self, "name", validated.name)
        object.__setattr__(self, "size", validated.size)
        object.__setattr__(self, "url", validated.url)


@dataclass(frozen=True, slots=True)
class MessageObservation:
    """Complete synthetic message observation; there is no verified boolean.

    Every recoverable field is required. Unsupported components are explicit
    so a future reconciler cannot silently fingerprint only part of a message.
    """

    guild_id: str
    channel_id: str
    message_id: str
    author_id: str
    content: str | None
    embeds: tuple[Mapping[str, object], ...]
    user_mentions: tuple[str, ...]
    role_mentions: tuple[str, ...]
    everyone_mentioned: bool
    replied_user: bool
    reference: str | None
    attachments: tuple[CompleteAttachmentEvidence, ...]
    unsupported_components: tuple[str, ...]

    def __post_init__(self) -> None:
        destination = Destination.channel(self.guild_id, self.channel_id)
        message_id = _snowflake(self.message_id, "message id")
        author_id = _snowflake(self.author_id, "message author id")
        if not isinstance(self.embeds, tuple) or not isinstance(self.attachments, tuple):
            raise ValueError("message embeds and attachments must be immutable tuples")
        if not isinstance(self.unsupported_components, tuple) or any(
            not isinstance(item, str) or not item.strip() for item in self.unsupported_components
        ):
            raise ValueError("unsupported message components must be named explicitly")
        if any(not isinstance(item, CompleteAttachmentEvidence) for item in self.attachments):
            raise ValueError("message attachments require complete evidence values")

        observed = ObservedMessage(
            content=self.content,
            embeds=self.embeds,
            user_mentions=self.user_mentions,
            role_mentions=self.role_mentions,
            everyone_mentioned=self.everyone_mentioned,
            replied_user=self.replied_user,
            reference=self.reference,
            attachments=tuple(
                ObservedAttachment(item.name, item.size, item.url, item.content_sha256)
                for item in self.attachments
            ),
        )
        if not observed.content and not observed.embeds and not observed.attachments:
            raise ValueError("message observation contains no recoverable content")

        object.__setattr__(self, "guild_id", destination.guild_id)
        object.__setattr__(self, "channel_id", destination.channel_id)
        object.__setattr__(self, "message_id", message_id)
        object.__setattr__(self, "author_id", author_id)
        object.__setattr__(self, "embeds", observed.embeds)
        object.__setattr__(self, "user_mentions", observed.user_mentions)
        object.__setattr__(self, "role_mentions", observed.role_mentions)
        object.__setattr__(self, "reference", observed.reference)

    @property
    def destination(self) -> Destination:
        return Destination.channel(self.guild_id, self.channel_id)

    @property
    def observed(self) -> ObservedMessage:
        return ObservedMessage(
            content=self.content,
            embeds=self.embeds,
            user_mentions=self.user_mentions,
            role_mentions=self.role_mentions,
            everyone_mentioned=self.everyone_mentioned,
            replied_user=self.replied_user,
            reference=self.reference,
            attachments=tuple(
                ObservedAttachment(item.name, item.size, item.url, item.content_sha256)
                for item in self.attachments
            ),
        )
