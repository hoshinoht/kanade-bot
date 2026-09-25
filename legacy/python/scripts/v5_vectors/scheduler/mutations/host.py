"""The duck-typed ``BossBot`` surface that ``bot.api.service`` mutations reach.

Only Discord transport is stood in for: channel lookup resolves any channel ID
and ``post_plain`` records the effect it was asked to journal. Materialisation
is the real ``BossBot.materialise_weeks`` bound to this host.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import time
from types import SimpleNamespace
from typing import Any
from zoneinfo import ZoneInfo

from bot.agent.client import BossBot, ChannelLookup
from bot.domain.bosses import BossTable
from bot.infrastructure.db import Repo


def catalog(raw: dict[str, Any]) -> BossTable:
    """Rebuild a synthetic catalog from ordered arrays, as the domain vectors do."""
    return BossTable.from_dict(
        {
            "difficulties": {entry["prefix"]: entry["label"] for entry in raw["difficulties"]},
            "bosses": {
                entry["short"]: {key: value for key, value in entry.items() if key != "short"}
                for entry in raw["bosses"]
            },
        }
    )


@dataclass
class Settings:
    tz: str
    reset_weekday: int
    reset_time: time
    chat_channel_id_list: list[int]
    chat_category_id_list: list[int] = field(default_factory=list)
    post_channel_id: int | None = None
    portal_actor_id: str | None = None


class Host:
    def __init__(self, repo: Repo, input_: dict[str, Any]):
        self.repo = repo
        self.bosses = catalog(input_["catalog"])
        self.tz = ZoneInfo(input_["timezone"])
        self.settings = Settings(
            tz=input_["timezone"],
            reset_weekday=input_["reset_weekday"],
            reset_time=time.fromisoformat(input_["reset_time"]),
            chat_channel_id_list=[int(item) for item in input_["watched_channel_ids"]],
        )
        self.ping_time = time.fromisoformat(input_["ping_time"])
        self.countdowns = list(input_["countdowns"])
        self.portal_actor_id = input_["portal_actor_id"]
        self.effects: list[dict[str, Any]] = []
        self.unexpected: list[str] = []

    def get_channel(self, channel_id: int) -> None:
        return None

    def get_user(self, user_id: int) -> None:
        return None

    def materialise_weeks(self) -> None:
        BossBot.materialise_weeks(self)  # type: ignore[arg-type]

    async def find_channel(
        self, channel_id: int | str | None = None, *, allow_fallback: bool = True
    ) -> ChannelLookup:
        if channel_id is None:
            return ChannelLookup(None, "no channel id")
        return ChannelLookup(SimpleNamespace(id=str(channel_id)), None)

    async def post_plain(
        self,
        channel: Any,
        content: str,
        mention_users: list[str],
        *,
        effect_kind: str,
        effect_context: tuple[str | int, ...],
        **extra: Any,
    ) -> None:
        # `_announce` swallows every post failure, so surprises are recorded, not raised.
        if extra:
            self.unexpected.append(f"post_plain received unmodelled arguments {sorted(extra)}")
        try:
            self.effects.append(
                {
                    "channel_id": channel.id,
                    "mentions": list(mention_users),
                    "effect_kind": effect_kind,
                    "effect_context": list(effect_context),
                }
            )
        except Exception as exc:
            self.unexpected.append(f"post_plain could not record its effect: {exc!r}")
            raise
