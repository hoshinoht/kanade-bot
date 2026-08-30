"""A stand-in for :class:`bot.client.BossBot` for the API tests.

The API layer only duck-types the client -- it needs the repository, the boss
table, the settings, the runtime-config properties and a handful of async
methods that talk to Discord.  Faking those is far more useful than mocking
discord.py: every Discord side effect is *recorded*, so a test can assert that
approving a proposal annotated the right card, and nothing needs a gateway.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import time
from typing import Any
from zoneinfo import ZoneInfo

from bot.bosses import BossTable
from bot.config import Settings
from bot.db import Repo
from bot.weeks import parse_hhmm

GUILD_ID = 111111111111111111
OWNER_ID = 999999999999999999
WATCHED_CHANNEL = 222222222222222222
OTHER_CHANNEL = 333333333333333333
UNWATCHED_CHANNEL = 444444444444444444
ADMIN_TOKEN = "test-admin-token-0123456789abcdef"


def make_settings(**overrides: Any) -> Settings:
    """Settings that ignore the developer's real ``.env``."""
    values: dict[str, Any] = {
        "discord_token": "not-a-real-token",
        "guild_id": GUILD_ID,
        "bossing_role_id": 555555555555555555,
        "chat_channel_ids": f"{WATCHED_CHANNEL},{OTHER_CHANNEL}",
        "tz": "Asia/Kuala_Lumpur",
        "db_path": ":memory:",
        "admin_token": ADMIN_TOKEN,
        "post_channel_id": WATCHED_CHANNEL,
    }
    values.update(overrides)
    return Settings(_env_file=None, **values)


@dataclass
class Posted:
    """One thing the bot was asked to send, so a test can look at it."""

    channel_id: Any
    content: str
    mentions: list[str] = field(default_factory=list)
    kind: str = "plain"


class FakeMessage:
    def __init__(self, message_id: int, channel: Any):
        self.id = message_id
        self.channel = channel


class FakePermissions:
    def __init__(self, **flags: bool):
        self.view_channel = flags.get("view_channel", True)
        self.send_messages = flags.get("send_messages", True)
        self.read_message_history = flags.get("read_message_history", True)
        self.embed_links = flags.get("embed_links", True)
        self.add_reactions = flags.get("add_reactions", True)


class FakeChannel:
    def __init__(self, channel_id: int, name: str, category_id: int | None = None):
        self.id = channel_id
        self.name = name
        self.category_id = category_id
        self.parent = None
        self.guild = None
        #: Flip these to simulate a channel the bot can see but not post in.
        self.permissions = FakePermissions()

    def permissions_for(self, _member):
        return self.permissions


class FakeMe:
    id = 5555555555555555555
    name = "YuukiSakuna"


class FakeGuild:
    def __init__(self, channels: list[FakeChannel]):
        self.id = GUILD_ID
        self.owner_id = OWNER_ID
        self.text_channels = channels
        self.me = FakeMe()
        for channel in channels:
            channel.guild = self


class FakeExtractor:
    """Stands in for :class:`bot.extract.pipeline.Pipeline`."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, str, bool]] = []
        self.plan: Any = None
        self.report: Any = None

    async def rescan(self, channel_id, hours=24, post=True):
        self.calls.append((str(channel_id), f"{hours}h", post))
        return self.plan

    async def rescan_window(self, channel_id, window="week", post=True):
        from bot.extract.pipeline import RescanReport
        from bot.timeutil import utcnow

        self.calls.append((str(channel_id), window, post))
        if self.report is not None:
            return self.report
        return RescanReport(channel_id=str(channel_id), window=window, since=utcnow())


class FakeBot:
    """Everything :mod:`bot.api` reaches for, and nothing else."""

    def __init__(self, repo: Repo, bosses: BossTable, settings: Settings | None = None):
        self.repo = repo
        self.bosses = bosses
        self.settings = settings or make_settings()
        self.tz: ZoneInfo = self.settings.zoneinfo
        self.extractor = FakeExtractor()

        self.channels = {
            WATCHED_CHANNEL: FakeChannel(WATCHED_CHANNEL, "hstar-party"),
            OTHER_CHANNEL: FakeChannel(OTHER_CHANNEL, "xkalos-party"),
            UNWATCHED_CHANNEL: FakeChannel(UNWATCHED_CHANNEL, "off-topic"),
        }
        self.guild = FakeGuild([self.channels[WATCHED_CHANNEL], self.channels[OTHER_CHANNEL]])
        self.user = FakeMe()

        # what the bot was asked to do, in order
        self.posts: list[Posted] = []
        self.annotations: list[tuple[Any, Any, str]] = []
        self.declines: list[tuple[str, str]] = []
        self.retractions: list[tuple[str, str]] = []
        self.materialised = 0
        self.backfills: list[tuple[str, Any, Any]] = []
        self.backfill_count = 0
        self.digest_channel: Any = "unset"
        self.digest_fails = False
        self._next_message_id = 700000000000000000

    # -- runtime config ----------------------------------------------------
    @property
    def ping_time(self) -> time:
        return parse_hhmm(self.repo.get_config("day_of_ping_time", "09:00"))

    @property
    def countdowns(self) -> list[int]:
        raw = self.repo.get_config("countdown_minutes", "60,15") or ""
        return sorted({int(p) for p in raw.split(",") if p.strip()}, reverse=True)

    @property
    def paused(self) -> bool:
        return (self.repo.get_config("paused", "0") or "0") == "1"

    @property
    def extract_enabled(self) -> bool:
        default = "1" if self.settings.extract_enabled else "0"
        return (self.repo.get_config("extract_enabled", default) or default) == "1"

    @property
    def portal_actor_id(self) -> str:
        if self.settings.portal_actor_id is not None:
            return str(self.settings.portal_actor_id)
        return str(OWNER_ID)

    # -- discord lookups ---------------------------------------------------
    def get_guild(self, guild_id):
        return self.guild if int(guild_id) == GUILD_ID else None

    def get_channel(self, channel_id):
        try:
            return self.channels.get(int(channel_id))
        except (TypeError, ValueError):
            return None

    def get_user(self, user_id):
        return None

    def is_watched(self, channel) -> bool:
        from bot.watch import is_watched

        return is_watched(
            channel,
            self.settings.chat_channel_id_list,
            self.settings.chat_category_id_list,
        )

    def has_bossing_role(self, user) -> bool:
        return self.repo.has_role(getattr(user, "id", user))

    def watched_text_channels(self):
        """Delegates to the real implementation, so the fake cannot drift from it."""
        from bot.client import BossBot

        return BossBot.watched_text_channels(self)

    def resolve_channel(self, channel_id):
        from bot.client import BossBot

        return BossBot.resolve_channel(self, channel_id)

    def materialise_weeks(self) -> None:
        self.materialised += 1

    async def backfill(self, channel, since, until=None) -> int:
        if not self.is_watched(channel):
            return 0
        self.backfills.append((str(getattr(channel, "id", channel)), since, until))
        return self.backfill_count

    async def backfill_channel(self, channel_id, since, until=None) -> int:
        channel = self.get_channel(channel_id)
        if channel is None:
            return 0
        return await self.backfill(channel, since, until)

    # -- discord side effects ---------------------------------------------
    def _message(self, channel) -> FakeMessage:
        self._next_message_id += 1
        return FakeMessage(self._next_message_id, channel)

    async def find_channel(self, channel_id=None):
        """Mirrors :meth:`bot.client.BossBot.find_channel`, including its reasons."""
        from bot.client import ChannelLookup

        problems = []
        for candidate in (channel_id, self.settings.post_channel_id):
            if candidate is None:
                continue
            channel = self.get_channel(candidate)
            if channel is None:
                problems.append(
                    f"channel {candidate} does not exist, or the bot is not in that server"
                )
                continue
            if not self.can_send_in(channel):
                problems.append(
                    f"the bot has no access to #{channel.name} - grant the YuukiSakuna role "
                    "View Channel + Send Messages there"
                )
                continue
            return ChannelLookup(channel=channel, problem=None)
        if not problems:
            problems.append("no channel was given and POST_CHANNEL_ID is not set in .env")
        return ChannelLookup(channel=None, problem="; ".join(problems))

    def can_send_in(self, channel):
        from bot.client import BossBot

        return BossBot.can_send_in(self, channel)

    def access_report(self):
        from bot.client import BossBot

        return BossBot.access_report(self)

    async def post_channel(self, channel_id=None):
        return (await self.find_channel(channel_id)).channel

    async def post_plain(self, channel, content, mention_users, reference_id=None):
        self.posts.append(
            Posted(getattr(channel, "id", None), content, list(mention_users), "plain")
        )
        return self._message(channel)

    async def _post(self, channel, card, mention_users=None):
        content = card if isinstance(card, str) else card.content
        self.posts.append(
            Posted(getattr(channel, "id", None), content, list(mention_users or []), "card")
        )
        return self._message(channel)

    async def annotate_message(self, channel_id, message_id, notice) -> bool:
        if channel_id is None or message_id is None:
            return False
        self.annotations.append((str(channel_id), str(message_id), notice))
        return True

    async def _mark_superseded(self, retired) -> None:
        for amendment in retired:
            await self.annotate_message(
                amendment.get("channel_id"),
                amendment.get("proposal_message_id"),
                "↪ superseded by a newer card",
            )

    async def notify_decline(self, run, user_id, display_name, channel_id=None, reference_id=None):
        self.declines.append((run["id"], str(user_id)))

    async def retract_decline(self, run, user_id):
        self.retractions.append((run["id"], str(user_id)))

    async def post_digest(self, channel_id=None, week="this"):
        self.digest_channel = channel_id
        if self.digest_fails:
            return None
        channel = await self.post_channel(channel_id)
        if channel is None:
            return None
        return self._message(channel)


__all__ = [
    "ADMIN_TOKEN",
    "GUILD_ID",
    "OTHER_CHANNEL",
    "OWNER_ID",
    "UNWATCHED_CHANNEL",
    "WATCHED_CHANNEL",
    "FakeBot",
    "FakeChannel",
    "Posted",
    "make_settings",
]
