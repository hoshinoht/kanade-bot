"""A ``BossBot`` surface for the chat oracle: real v4 code, stubbed Discord transport.

Card posting runs the real ``Pipeline.apply_plan`` -> journal -> ``BossBot``
channel lookup bound to this host; only ``channel.send``/``fetch_message`` are
stood in for, with a deterministic message-ID counter. Settings are the real
pydantic ``Settings`` built with the process environment masked, so nothing
the developer exported can leak into a vector. ``ChatPilot`` is constructed
with its persona loader pointed at the tracked public Kanade bundle only.
"""

from __future__ import annotations

import asyncio
import copy
import os
from collections.abc import Iterator
from contextlib import contextmanager
from datetime import datetime, time
from types import SimpleNamespace
from typing import Any
from zoneinfo import ZoneInfo

import discord

from bot.agent import client as client_mod
from bot.agent import materialise
from bot.agent.client import BossBot
from bot.api import service
from bot.chat import agent, persona, persona_catalog, tools
from bot.domain import timeutil
from bot.domain.boss_knowledge import BossKnowledgeError
from bot.extract import commit as commit_mod
from bot.extract import pipeline as pipeline_mod
from bot.extract.pipeline import Pipeline
from bot.infrastructure.config import Settings
from bot.infrastructure.db import Repo

from ..extract.contract import ContractError
from ..scheduler import oracle
from ..scheduler.mutations.host import catalog

FIRST_MESSAGE_ID = 710000000000000001
CLOCK_MODULES = (tools, service, agent, pipeline_mod, commit_mod, client_mod, materialise, timeutil)


class _Response:
    status = 404
    reason = "Not Found"


class Message:
    def __init__(self, message_id: int, channel: Channel, content: str, kwargs: dict):
        self.id = message_id
        self.channel = channel
        self.content = content or ""
        embed = kwargs.get("embed")
        self.embeds = list(kwargs.get("embeds") or ([embed] if embed is not None else []))
        self.mentions: list[Any] = []
        self.role_mentions: list[Any] = []
        self.mention_everyone = False
        self.reference = None
        self.attachments: list[Any] = []
        self.reactions: list[str] = []

    async def add_reaction(self, emoji: str) -> None:
        self.reactions.append(emoji)

    async def edit(self, content: str | None = None, **_: Any) -> None:
        if content is not None:
            self.content = content


class Channel(discord.abc.Messageable):
    def __init__(self, host: Host, spec: dict[str, Any]):
        self.id = int(spec["id"])
        self.name = spec["name"]
        self.category_id = int(spec["category_id"]) if spec["category_id"] else None
        self.parent_id = int(spec["parent_id"]) if spec["parent_id"] else None
        self.guild = SimpleNamespace(id=host.guild_id, me=None)
        self._host = host

    @property
    def parent(self) -> Channel | None:
        return self._host.channels.get(self.parent_id) if self.parent_id else None

    async def _get_channel(self) -> Channel:
        return self

    async def send(self, content: str | None = None, **kwargs: Any) -> Message:  # type: ignore[override]
        self._host.next_message_id += 1
        message = Message(self._host.next_message_id - 1, self, content or "", kwargs)
        self._host.messages[message.id] = message
        return message

    async def fetch_message(self, message_id: int) -> Message:
        message = self._host.messages.get(int(message_id))
        if message is None:
            raise discord.NotFound(_Response(), "unknown message")
        return message


class Guides:
    """Stand-in strategy store: guide rendering itself is not a chat contract."""

    def __init__(self, shorts: list[str]):
        self.shorts = set(shorts)

    def render(self, reference: Any, include_sources: bool = False) -> str:
        if reference.short not in self.shorts:
            raise BossKnowledgeError(f"no guide for {reference.short}")
        return f"<guide {reference.short} difficulty={reference.difficulty}>"


@contextmanager
def masked_environment() -> Iterator[None]:
    """Hide every process variable ``Settings`` would read."""
    fields = {name.lower() for name in Settings.model_fields}
    hidden = {key: os.environ.pop(key) for key in list(os.environ) if key.lower() in fields}
    try:
        yield
    finally:
        os.environ.update(hidden)


def settings(input_: dict[str, Any]) -> Settings:
    raw = input_["settings"]
    values: dict[str, Any] = {
        "discord_token": "synthetic-token",
        "guild_id": int(raw["guild_id"]),
        "bossing_role_id": 1,
        "chat_channel_ids": raw["watched_channel_ids"],
        "tz": input_["timezone"],
        "boss_week_reset_weekday": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"][
            input_["reset_weekday"]
        ],
        "boss_week_reset_time": input_["reset_time"],
        "db_path": ":memory:",
        "post_channel_id": int(raw["post_channel_id"]) if raw["post_channel_id"] else None,
        "chat_pilot_role_id": int(raw["chat_pilot_role_id"]) if raw["chat_pilot_role_id"] else None,
        "chat_pilot_channel_ids": raw["chat_pilot_channel_ids"],
        "chat_pilot_category_ids": raw["chat_pilot_category_ids"],
        "chat_pilot_model": raw["chat_pilot_model"],
        "model_context_tokens": raw["model_context_tokens"],
        "chat_pilot_history_ttl_s": raw["chat_pilot_history_ttl_s"],
        "chat_pilot_timeout": raw["chat_pilot_timeout"],
        "chat_pilot_temperature": raw["chat_pilot_temperature"],
        "chat_pilot_think": raw["chat_pilot_think"],
        "persona_path": "",
        "staging_path": "",
        "staging_profiles_dir": "",
    }
    with masked_environment():
        return Settings(_env_file=None, **values)


class Host:
    """Scheduling state lives in the repo; Discord is the stub channels above."""

    find_channel = BossBot.find_channel
    post_channel = BossBot.post_channel
    can_send_in = BossBot.can_send_in
    no_access = BossBot.no_access
    annotate_message = BossBot.annotate_message
    _prepared = BossBot._prepared
    _embed = staticmethod(BossBot._embed)
    materialise_weeks = BossBot.materialise_weeks

    quiet_mode = False
    paused = False
    chat_mode = True
    extract_enabled = True
    persona_name = ""
    chat_rate_count = 4
    chat_rate_window_s = 300.0
    chat_pool_count = 12
    chat_pool_window_s = 900.0
    portal_actor_id = "1"
    ping_time = time(1, 0)
    countdowns = [60]

    def __init__(self, repo: Repo, input_: dict[str, Any]):
        self.repo = repo
        self.settings = settings(input_)
        self.tz = ZoneInfo(input_["timezone"])
        self.bosses = catalog(input_["catalog"])
        self.guild_id = int(input_["settings"]["guild_id"])
        self.user = SimpleNamespace(
            id=int(input_["bot_user"]["id"]),
            name=input_["bot_user"]["name"],
            display_name=input_["bot_user"]["name"],
            bot=True,
        )
        self.boss_knowledge = Guides(input_["guides"])
        self.next_message_id = FIRST_MESSAGE_ID
        self.messages: dict[int, Message] = {}
        self.channels = {int(spec["id"]): Channel(self, spec) for spec in input_["channels"]}
        # The pipeline's own model client is never used by ``apply_plan``.
        self.extractor = Pipeline(self, extractor=object())  # type: ignore[arg-type]

    def get_channel(self, channel_id: int) -> Channel | None:
        return self.channels.get(int(channel_id))

    async def fetch_channel(self, channel_id: int) -> Channel:
        raise discord.NotFound(_Response(), "unknown channel")

    def get_user(self, user_id: int) -> None:
        return None

    def get_guild(self, guild_id: int) -> None:
        return None

    async def notify_decline(self, *args: Any, **kwargs: Any) -> None:
        raise ContractError("notify_decline is not modelled")

    async def retract_decline(self, *args: Any, **kwargs: Any) -> None:
        raise ContractError("retract_decline is not modelled")


class Ids(oracle.Ids):
    """``oracle.Ids`` plus ``pin``: the next generated ID is the given one."""

    def __init__(self, ids: list[str]):
        super().__init__(ids)
        self.pinned: list[str] = []

    def pin(self, value: str) -> None:
        self.pinned.append(value)

    def __call__(self) -> str:
        if self.pinned:
            return self.pinned.pop(0)
        return super().__call__()


def at(value: str | None) -> datetime | None:
    return datetime.fromisoformat(value) if value is not None else None


def seed(repo: Repo, ids: Ids, world: dict[str, Any]) -> None:
    for member in world["members"]:
        repo.upsert_member(
            member["user_id"], member["display_name"], member["nickname"], member["has_role"]
        )
    for fixed in world["fixed"]:
        ids.pin(fixed["id"])
        repo.add_fixed_run(
            fixed["owner_id"],
            fixed["bosses"],
            fixed["weekday"],
            fixed["time"],
            fixed["participants"],
            channel_id=fixed["channel_id"],
        )
    for run in world["runs"]:
        ids.pin(run["id"])
        repo.create_run(
            at(run["week_start"]),
            run["bosses"],
            at(run["at"]),
            run["participants"],
            run["status"],
            "fixed" if run["fixed_run_id"] else "manual",
            fixed_run_id=run["fixed_run_id"],
            channel_id=run["channel_id"],
        )
    for rsvp in world["rsvps"]:
        repo.set_rsvp(rsvp["run_id"], rsvp["user_id"], rsvp["state"], "reaction")


class Pilot(agent.ChatPilot):
    """The v4 pilot with its persona source fixed to the tracked public bundle."""

    def _load_configured_persona_runtime(self) -> persona_catalog.PersonaRuntime:
        bundle = persona_catalog.load_example_bundle(persona.PERSONA_DIR)
        return persona_catalog.PersonaRuntime(bundle, {})


class Monotonic:
    """The pilot's monotonic clock, moved only by explicit steps."""

    def __init__(self, value: float = 1000.0):
        self.value = value

    def __call__(self) -> float:
        return self.value


@contextmanager
def session(case: dict[str, Any]) -> Iterator[SimpleNamespace]:
    """One clean seeded repo and host per replay, under the pinned seams."""
    input_ = case["input"]
    clock = oracle.Clock(input_["clock"])
    ids = Ids(list(input_["uuid_sequence"]))
    with oracle.seams(clock, ids, CLOCK_MODULES), oracle.quiet_logs():
        repo = Repo(":memory:")
        try:
            seed(repo, ids, input_["world"])
            host = Host(repo, input_)
            yield SimpleNamespace(
                case_id=case["case_id"], repo=repo, host=host, ids=ids, clock=clock
            )
            if ids.exhausted:
                raise ContractError(f"{case['case_id']}: uuid_sequence exhausted")
        finally:
            repo.close()


def tool_context(host: Host, step: dict[str, Any], input_: dict[str, Any]) -> tools.ToolContext:
    return tools.ToolContext(
        bot=host,
        author_id=step["author_id"],
        channel_id=step["channel_id"],
        message_id=step.get("message_id", "990001"),
        is_admin=step.get("is_admin", False),
        read_only=step.get("read_only", False),
        bot_user_id=input_["bot_user"]["id"],
        self_role_id=input_["self_role_id"],
        force_all_channels=step.get("force_all_channels", False),
        force_channel_scope=step.get("force_channel_scope", False),
        force_group_schedule=step.get("force_group_schedule", False),
        upcoming_only=step.get("upcoming_only", False),
    )


def outcome(value: tools.ToolOutcome) -> dict[str, Any]:
    """Everything a ``ToolOutcome`` records except its wall-clock duration."""
    return {
        "name": value.name,
        "output": value.output,
        "arguments": copy.deepcopy(value.arguments),
        "ok": value.ok,
        "error": value.error,
        "created": list(value.created),
        "posted": list(value.posted),
    }


def run_tool(host: Host, step: dict[str, Any], input_: dict[str, Any]) -> dict[str, Any]:
    ctx = tool_context(host, step, input_)
    return outcome(asyncio.run(tools.run(ctx, step["tool"], step["arguments"])))


def amendments(repo: Repo) -> list[dict[str, Any]]:
    """Every amendment row a proposal wrote, in creation order, minus timestamps."""
    return [
        {
            "id": row["id"],
            "kind": row["kind"],
            "status": row["status"],
            "week_start": row["week_start"].isoformat(),
            "run_id": row["run_id"],
            "new_datetime": row["new_datetime"].isoformat() if row["new_datetime"] else None,
            "bosses": list(row["bosses"]),
            "participants": list(row["participants"]),
            "rsvp": row["rsvp"],
            "confidence": row["confidence"],
            "channel_id": row["channel_id"],
            "is_question": bool(row["is_question"]),
            "evidence_msg_ids": list(row["evidence_msg_ids"]),
            "summary": row["summary"],
            "payload": row["payload"],
            "card_posted": bool(row["proposal_message_id"]),
        }
        for row in repo.list_amendments()
    ]
