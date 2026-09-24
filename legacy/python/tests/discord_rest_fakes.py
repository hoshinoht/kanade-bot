"""Offline Discord REST fakes: bounded history pages and injected HTTP failures.

Like ``fake_bot.py``, these record calls instead of reaching a gateway. Ids are
synthetic snowflakes.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from types import SimpleNamespace
from typing import Any

import discord

GUILD_ID = 111111111111111111
CHANNEL_ID = 222222222222222222
BOT_ID = 5555555555555555555
HUMAN_ID = 777777777777777777


def _response(status: int, reason: str) -> Any:
    return type("Response", (), {"status": status, "reason": reason})()


def forbidden() -> discord.Forbidden:
    return discord.Forbidden(_response(403, "Forbidden"), "missing access")


def not_found() -> discord.NotFound:
    return discord.NotFound(_response(404, "Not Found"), "unknown channel")


def http_error(status: int = 500) -> discord.HTTPException:
    return discord.HTTPException(_response(status, "Server Error"), "server error")


@dataclass
class RestMessage:
    id: int
    created_at: datetime
    author_id: int
    channel: Any
    content: str = "synthetic message"
    embeds: list[Any] = field(default_factory=list)
    attachments: list[Any] = field(default_factory=list)
    components: list[Any] = field(default_factory=list)
    mentions: list[Any] = field(default_factory=list)
    role_mentions: list[Any] = field(default_factory=list)
    mention_everyone: bool = False
    reference: Any = None

    @property
    def author(self) -> Any:
        return SimpleNamespace(id=self.author_id, bot=self.author_id == BOT_ID)


class HistoryChannel:
    """A text channel whose ``history`` honours limit/after/before like discord.py."""

    def __init__(
        self,
        channel_id: int = CHANNEL_ID,
        guild_id: int | None = GUILD_ID,
        *,
        recipient_id: int | None = None,
    ):
        self.id = channel_id
        self.guild = SimpleNamespace(id=guild_id) if guild_id is not None else None
        self.recipient = SimpleNamespace(id=recipient_id) if recipient_id is not None else None
        self.messages: list[RestMessage] = []
        self.history_calls: list[dict[str, Any]] = []
        #: Raise this after yielding ``fail_after`` messages (0 = before any page).
        self.error: BaseException | None = None
        self.fail_after = 0
        #: Ignore ``limit`` to model a misbehaving transport.
        self.overrun = False

    def add(self, message_id: int, created_at: datetime, *, author_id: int = BOT_ID, **extra):
        message = RestMessage(message_id, created_at, author_id, self, **extra)
        self.messages.append(message)
        return message

    async def history(
        self,
        *,
        limit: int | None = 100,
        before: datetime | None = None,
        after: datetime | None = None,
        around: datetime | None = None,
        oldest_first: bool | None = None,
    ):
        self.history_calls.append(
            {"limit": limit, "before": before, "after": after, "oldest_first": oldest_first}
        )
        selected = [
            message
            for message in sorted(self.messages, key=lambda item: (item.created_at, item.id))
            if (after is None or message.created_at > after)
            and (before is None or message.created_at < before)
        ]
        if not oldest_first:
            selected.reverse()
        if limit is not None and not self.overrun:
            selected = selected[:limit]
        for index, message in enumerate(selected):
            if self.error is not None and index == self.fail_after:
                raise self.error
            yield message
        if self.error is not None and self.fail_after >= len(selected):
            raise self.error
