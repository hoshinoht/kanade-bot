"""A ``BossBot`` surface whose scheduling methods are the real v4 ones.

Only Discord is stood in for: ``get_channel`` returns a stub text channel for
the IDs a case declares available, ``fetch_channel`` raises ``NotFound`` for
the rest, and the stub records nothing itself -- intents are captured from the
journal's ``SendPlan`` (:mod:`.delivery`). Message IDs are a deterministic
counter. Channel resolution, fallback, card building, grouping and binding all
run through ``BossBot`` code bound to this host.
"""

from __future__ import annotations

from datetime import time
from types import SimpleNamespace
from typing import Any
from zoneinfo import ZoneInfo

import discord

from bot.agent.client import BossBot
from bot.infrastructure.db import Repo

from .mutations.host import catalog

FIRST_MESSAGE_ID = 700000000000000001


class _Response:
    status = 404
    reason = "Not Found"


class StubMessage:
    def __init__(self, message_id: int, channel: StubChannel, content: str, kwargs: dict):
        self.id = message_id
        self.channel = channel
        self.content = content or ""
        embed = kwargs.get("embed")
        self.embeds = list(kwargs.get("embeds") or ([embed] if embed is not None else []))
        users = getattr(kwargs.get("allowed_mentions"), "users", False)
        roles = getattr(kwargs.get("allowed_mentions"), "roles", False)
        self.mentions = [] if isinstance(users, bool) else list(users)
        self.role_mentions = [] if isinstance(roles, bool) else list(roles)
        self.mention_everyone = False
        self.reference = None
        self.attachments: list[Any] = []

    async def add_reaction(self, emoji: str) -> None:
        return None


class StubChannel(discord.abc.Messageable):
    def __init__(self, host: Host, channel_id: int):
        self.id = channel_id
        self.name = f"channel-{channel_id}"
        self.guild = SimpleNamespace(id=host.guild_id, me=None)
        self._host = host

    async def _get_channel(self) -> StubChannel:
        return self

    async def send(self, content: str | None = None, **kwargs: Any) -> StubMessage:  # type: ignore[override]
        if self._host.fail_sends:
            raise TimeoutError("scripted ambiguous send")
        self._host.next_message_id += 1
        return StubMessage(self._host.next_message_id - 1, self, content or "", kwargs)

    async def fetch_message(self, message_id: int) -> Any:
        self._host.unexpected.append(f"fetch_message({message_id}) is not modelled")
        raise discord.NotFound(_Response(), "unknown message")


class Host:
    """Scheduling state lives in the repo; this holds only config and the stubs."""

    dispatch_reminders = BossBot.dispatch_reminders
    _send_day_of = BossBot._send_day_of
    _send_countdown = BossBot._send_countdown
    day_of_card_for = BossBot.day_of_card_for
    countdown_card_for = BossBot.countdown_card_for
    find_channel = BossBot.find_channel
    post_channel = BossBot.post_channel
    can_send_in = BossBot.can_send_in
    no_access = BossBot.no_access
    _prepared = BossBot._prepared
    _embed = staticmethod(BossBot._embed)
    materialise_weeks = BossBot.materialise_weeks
    post_week_digest = BossBot.post_week_digest
    _post_week_digest = BossBot._post_week_digest
    post_digest = BossBot.post_digest
    _post_digest = BossBot._post_digest
    _digest_guard = BossBot._digest_guard

    quiet_mode = False
    user = None

    def __init__(self, repo: Repo, input_: dict[str, Any]):
        self.repo = repo
        self.tz = ZoneInfo(input_["timezone"])
        self.bosses = catalog(input_["catalog"])
        self.guild_id = int(input_["guild_id"])
        post = input_["post_channel_id"]
        self.settings = SimpleNamespace(
            reset_weekday=input_["reset_weekday"],
            reset_time=time.fromisoformat(input_["reset_time"]),
            post_channel_id=int(post) if post is not None else None,
        )
        self.ping_time = time.fromisoformat(input_["ping_time"])
        self.countdowns = list(input_["countdowns"])
        self.available = {int(item) for item in input_["available_channel_ids"]}
        self.fail_sends = False
        self.next_message_id = FIRST_MESSAGE_ID
        self.unexpected: list[str] = []

    def get_channel(self, channel_id: int) -> StubChannel | None:
        return StubChannel(self, channel_id) if channel_id in self.available else None

    async def fetch_channel(self, channel_id: int) -> StubChannel:
        raise discord.NotFound(_Response(), "unknown channel")
