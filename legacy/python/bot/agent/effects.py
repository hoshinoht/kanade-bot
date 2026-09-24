"""Lease-gated Discord effects and journalled card sends."""

from __future__ import annotations

import hashlib
import io
import json
import logging
from datetime import UTC, datetime
from pathlib import Path

import discord

from bot.infrastructure.maintenance.coordinator import MaintenanceClosedError
from bot.infrastructure.maintenance.delivery import (
    DedupePolicy,
    DeliveryOutcomeKind,
    DeliveryReceipt,
    DeliveryTarget,
    Destination,
    ObservedAttachment,
    ObservedMessage,
    SendFile,
    SendPayload,
    SendPlan,
)
from bot.infrastructure.maintenance.delivery.model import thaw_json

from . import formatting

log = logging.getLogger(__name__)

# Distinguish same-basename portrait and splash attachments in embed URLs.
IMAGE_PREFIX = "image-"


def _destination(channel: object) -> Destination:
    guild = getattr(channel, "guild", None)
    channel_id = getattr(channel, "id", None)
    guild_id = getattr(guild, "id", None)
    if guild_id is None or channel_id is None:
        raise ValueError("journalled channel sends require a resolved guild and channel")
    return Destination.channel(guild_id, channel_id)


def _ids(value: object) -> tuple[str, ...]:
    if value is False:
        return ()
    if value is True:
        raise ValueError("journalled cards require an explicit mention allowlist")
    return tuple(str(item.id) for item in value)  # type: ignore[union-attr]


def _card_files(card: formatting.Card) -> tuple[SendFile, ...]:
    files: list[SendFile] = []
    for path, prefix in (
        (card.thumbnail_path, ""),
        (card.image_path, IMAGE_PREFIX),
    ):
        if path is None:
            continue
        try:
            files.append(SendFile(f"{prefix}{path.name}", Path(path).read_bytes()))
        except OSError:
            log.warning("could not read the card image at %s", path)
    return tuple(files)


def _send_plan(
    bot: object,
    channel: object,
    card: formatting.Card,
    *,
    effect_kind: str,
    targets: tuple[DeliveryTarget, ...],
    dedupe: DedupePolicy | None,
    mention_users: list[str] | None,
) -> SendPlan:
    card, allowed = bot._prepared(card, mention_users)  # type: ignore[attr-defined]
    embed = bot._embed(card)  # type: ignore[attr-defined]
    embeds = (embed.to_dict(),) if embed is not None else ()
    payload = SendPayload(
        content=card.content,
        embeds=embeds,
        files=_card_files(card),
        allowed_user_ids=_ids(allowed.users),
        allowed_role_ids=_ids(allowed.roles),
        allow_everyone=bool(allowed.everyone),
        replied_user=allowed.replied_user is True,
    )
    return SendPlan(
        effect_kind,
        _destination(channel),
        payload,
        dedupe or (DedupePolicy.native() if targets else DedupePolicy.operation()),
        targets,
    )


def _plain_plan(
    channel: object,
    content: str,
    *,
    mention_users: list[str],
    mention_roles: list[str] | None,
    reference_id: int | str | None,
    source_message_id: int | str,
    semantic_slot: str,
) -> SendPlan:
    destination = _destination(channel)
    payload = SendPayload(
        content=content,
        allowed_user_ids=tuple(str(user_id) for user_id in mention_users),
        allowed_role_ids=tuple(str(role_id) for role_id in mention_roles or ()),
        reference=str(reference_id) if reference_id else None,
    )
    return SendPlan(
        "chat.reply",
        destination,
        payload,
        DedupePolicy.source(
            destination.guild_id or "",
            destination.channel_id or "",
            source_message_id,
            semantic_slot,
        ),
    )


def _allowed_mentions(payload: SendPayload, *, quiet_mode: bool) -> discord.AllowedMentions:
    return discord.AllowedMentions(
        everyone=payload.allow_everyone,
        users=(
            False
            if quiet_mode
            else [discord.Object(id=int(user_id)) for user_id in payload.allowed_user_ids]
        ),
        roles=(
            [discord.Object(id=int(role_id)) for role_id in payload.allowed_role_ids]
            if payload.allowed_role_ids
            else False
        ),
        replied_user=payload.replied_user,
    )


class _DiscordTransport:
    def __init__(self, channel: object, *, quiet_mode: bool = False):
        self.channel = channel
        self.quiet_mode = quiet_mode
        self.message: object | None = None

    async def send(self, plan: SendPlan):
        files = [
            discord.File(io.BytesIO(item.content), filename=item.name)
            for item in plan.payload.files
        ]
        try:
            embeds = [discord.Embed.from_dict(thaw_json(embed)) for embed in plan.payload.embeds]
            kwargs = {
                "allowed_mentions": _allowed_mentions(plan.payload, quiet_mode=self.quiet_mode)
            }
            if len(embeds) == 1:
                kwargs["embed"] = embeds[0]
            elif embeds:
                kwargs["embeds"] = embeds
            if files:
                kwargs["files"] = files
            if plan.payload.reference is not None:
                kwargs["reference"] = discord.MessageReference(
                    message_id=int(plan.payload.reference),
                    channel_id=int(plan.destination.channel_id or 0),
                    fail_if_not_exists=False,
                )
            message = await self.channel.send(plan.payload.content, **kwargs)  # type: ignore[attr-defined]
            self.message = message
            return _receipt(message)
        except discord.HTTPException as exc:
            raise DeliveryUncertainError("Discord did not confirm the message") from exc
        except Exception as exc:
            raise DeliveryUncertainError("Discord did not confirm the message") from exc
        finally:
            for file in files:
                file.close()


def _receipt(message: object):
    channel = message.channel  # type: ignore[attr-defined]
    guild = channel.guild
    reference = getattr(message, "reference", None)
    attachments = tuple(
        ObservedAttachment(item.filename, item.size, url=getattr(item, "url", None))
        for item in message.attachments  # type: ignore[attr-defined]
    )
    observed = ObservedMessage(
        content=message.content,  # type: ignore[attr-defined]
        embeds=tuple(embed.to_dict() for embed in message.embeds),  # type: ignore[attr-defined]
        user_mentions=tuple(str(user.id) for user in message.mentions),  # type: ignore[attr-defined]
        role_mentions=tuple(str(role.id) for role in message.role_mentions),  # type: ignore[attr-defined]
        everyone_mentioned=bool(message.mention_everyone),  # type: ignore[attr-defined]
        replied_user=False,
        reference=getattr(reference, "message_id", None),
        attachments=attachments,
    )
    return DeliveryReceipt(Destination.channel(guild.id, channel.id), message.id, observed)  # type: ignore[attr-defined]


class DeliveryUncertainError(RuntimeError):
    """A send failed without proof that Discord rejected it."""


def _contextual_kind(effect_kind: str, context: tuple[str | int, ...]) -> str:
    if not context:
        return effect_kind
    encoded = json.dumps(
        ["kanade.delivery.effect-context.v1", [str(item) for item in context]],
        separators=(",", ":"),
    ).encode()
    return f"{effect_kind}.ctx1.{hashlib.sha256(encoded).hexdigest()}"


async def send_operation_payload(
    bot: object,
    channel: object,
    payload: SendPayload,
    *,
    effect_kind: str,
    context: tuple[str | int, ...] = (),
    targets: tuple[DeliveryTarget, ...] = (),
):
    """Journal one operation-scoped post, or bind its typed native target."""
    quiet_mode = bool(getattr(bot, "quiet_mode", False))
    if quiet_mode:
        payload = SendPayload(
            content=formatting.quiet_line(payload.content or ""),
            embeds=payload.embeds,
            files=payload.files,
            reference=payload.reference,
        )
    kind = _contextual_kind(effect_kind, context)
    plan = SendPlan(
        kind,
        _destination(channel),
        payload,
        DedupePolicy.native() if targets else DedupePolicy.operation(),
        targets,
    )
    transport = _DiscordTransport(channel, quiet_mode=quiet_mode)
    async with bot.repo.maintenance.operation(f"discord_send:{kind}"):  # type: ignore[attr-defined]
        outcome = await bot.repo.delivery.execute(plan, transport)  # type: ignore[attr-defined]
        if outcome.kind is DeliveryOutcomeKind.BOUND:
            return transport.message
    return None


async def send_operation_plain(
    bot: object,
    channel: object,
    content: str,
    *,
    mention_users: list[str],
    mention_roles: list[str] | None = None,
    reference_id: int | str | None = None,
    effect_kind: str,
    context: tuple[str | int, ...] = (),
    targets: tuple[DeliveryTarget, ...] = (),
):
    """Journal one plain operation post with its exact mention allow-list."""
    payload = SendPayload(
        content=content,
        allowed_user_ids=tuple(str(user_id) for user_id in mention_users),
        allowed_role_ids=tuple(str(role_id) for role_id in mention_roles or ()),
        reference=str(reference_id) if reference_id else None,
    )
    return await send_operation_payload(
        bot,
        channel,
        payload,
        effect_kind=effect_kind,
        context=context,
        targets=targets,
    )


async def send_card(
    bot: object,
    channel: object,
    card: formatting.Card,
    *,
    effect_kind: str,
    targets: tuple[DeliveryTarget, ...],
    dedupe: DedupePolicy | None = None,
    mention_users: list[str] | None = None,
    react: bool = True,
):
    """Journal one card, bind its native targets, then optionally add RSVP reactions."""
    plan = _send_plan(
        bot,
        channel,
        card,
        effect_kind=effect_kind,
        targets=targets,
        dedupe=dedupe,
        mention_users=mention_users,
    )
    transport = _DiscordTransport(channel, quiet_mode=bool(getattr(bot, "quiet_mode", False)))
    try:
        async with bot.repo.maintenance.operation(f"discord_send:{effect_kind}"):  # type: ignore[attr-defined]
            outcome = await bot.repo.delivery.execute(plan, transport)  # type: ignore[attr-defined]
            if outcome.kind is not DeliveryOutcomeKind.BOUND or transport.message is None:
                return None
            if react:
                from .rsvp import EMOJI_NO, EMOJI_YES

                await add_reactions(bot, transport.message, (EMOJI_YES, EMOJI_NO))
            return transport.message
    except MaintenanceClosedError:
        log.info("maintenance denied Discord send %s", effect_kind)
        return None
    except Exception:
        log.exception("journalled Discord send %s did not bind", effect_kind)
        return None


async def send_plain(
    bot: object,
    channel: object,
    content: str,
    *,
    mention_users: list[str],
    mention_roles: list[str] | None = None,
    reference_id: int | str | None = None,
    source_message_id: int | str,
    semantic_slot: str,
):
    """Journal one source-keyed chat reply without a direct-send fallback."""
    plan = _plain_plan(
        channel,
        content,
        mention_users=mention_users,
        mention_roles=mention_roles,
        reference_id=reference_id,
        source_message_id=source_message_id,
        semantic_slot=semantic_slot,
    )
    transport = _DiscordTransport(channel, quiet_mode=bool(getattr(bot, "quiet_mode", False)))
    try:
        async with bot.repo.maintenance.operation("discord_send:chat_reply"):  # type: ignore[attr-defined]
            outcome = await bot.repo.delivery.execute(plan, transport)  # type: ignore[attr-defined]
            if outcome.kind is DeliveryOutcomeKind.BOUND:
                return transport.message
    except MaintenanceClosedError:
        log.info("maintenance denied chat reply")
    except Exception:
        log.exception("journalled chat reply did not bind")
    return None


async def add_reactions(bot: object, message: object, emojis: tuple[str, ...]) -> bool:
    """Add reactions only while the current task owns a live maintenance lease."""
    try:
        async with bot.repo.maintenance.operation("discord_add_reactions"):  # type: ignore[attr-defined]
            for emoji in emojis:
                await message.add_reaction(emoji)  # type: ignore[attr-defined]
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied Discord reaction add")
    except Exception:
        log.warning("could not add Discord reactions", exc_info=True)
    return False


async def add_chat_reaction(bot: object, message: object, emoji: str) -> bool:
    """Add one chat reaction only while the current task owns a live lease."""
    try:
        async with bot.repo.maintenance.operation("chat_add_reaction"):  # type: ignore[attr-defined]
            await message.add_reaction(emoji)  # type: ignore[attr-defined]
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied chat reaction add")
    except Exception:
        log.warning("could not add chat reaction", exc_info=True)
    return False


async def remove_chat_reaction(bot: object, message: object, emoji: str) -> bool:
    """Remove the bot's chat reaction only while the current task owns a live lease."""
    user = getattr(bot, "user", None)
    if user is None:
        return False
    try:
        async with bot.repo.maintenance.operation("chat_remove_reaction"):  # type: ignore[attr-defined]
            await message.remove_reaction(emoji, user)  # type: ignore[attr-defined]
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied chat reaction removal")
    except Exception:
        log.warning("could not remove chat reaction", exc_info=True)
    return False


async def edit_plain(bot: object, placeholder: object, content: str) -> bool:
    """Lease-gate the silent edit of a chat placeholder."""
    if getattr(bot, "quiet_mode", False):
        content = formatting.quiet_line(content)
    try:
        async with bot.repo.maintenance.operation("chat_edit_placeholder"):  # type: ignore[attr-defined]
            await placeholder.edit(  # type: ignore[attr-defined]
                content=content, allowed_mentions=discord.AllowedMentions.none()
            )
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied chat placeholder edit")
    except Exception:
        log.warning("could not edit chat placeholder", exc_info=True)
    return False


async def delete_placeholder(bot: object, placeholder: object) -> bool:
    """Lease-gate deletion of a chat placeholder; never call Discord directly elsewhere."""
    try:
        async with bot.repo.maintenance.operation("chat_delete_placeholder"):  # type: ignore[attr-defined]
            await placeholder.delete()  # type: ignore[attr-defined]
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied chat placeholder deletion")
    except Exception:
        log.debug("could not delete chat placeholder", exc_info=True)
    return False


async def remove_reaction(
    bot: object, channel_id: int | str, message_id: int | str, user_id: int | str, emoji: str
) -> bool:
    try:
        async with bot.repo.maintenance.operation("discord_remove_reaction"):  # type: ignore[attr-defined]
            channel = await _resolve_channel(bot, channel_id)
            message = await channel.fetch_message(int(message_id))
            await message.remove_reaction(emoji, discord.Object(id=int(user_id)))
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied Discord reaction removal")
        return False


async def edit_card(
    bot: object, channel_id: int | str | None, message_id: int | str, card: formatting.Card
) -> bool:
    if channel_id is None:
        return False
    card, allowed = bot._prepared(card)  # type: ignore[attr-defined]
    embed = bot._embed(card)  # type: ignore[attr-defined]
    try:
        async with bot.repo.maintenance.operation("discord_edit_card"):  # type: ignore[attr-defined]
            channel = await _resolve_channel(bot, channel_id)
            message = await channel.fetch_message(int(message_id))
            await message.edit(content=card.content, embed=embed, allowed_mentions=allowed)
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied Discord card edit")
    except (
        discord.NotFound,
        discord.Forbidden,
        discord.HTTPException,
        AttributeError,
        TypeError,
        ValueError,
        OSError,
        TimeoutError,
    ):
        log.debug("could not refresh card %s", message_id, exc_info=True)
    return False


async def annotate_message(
    bot: object, channel_id: int | str | None, message_id: int | str | None, notice: str
) -> bool:
    if channel_id is None or message_id is None:
        return False
    try:
        async with bot.repo.maintenance.operation("discord_annotate_message"):  # type: ignore[attr-defined]
            channel = await _resolve_channel(bot, channel_id)
            message = await channel.fetch_message(int(message_id))
            if notice in (message.content or ""):
                return True
            await message.edit(
                content=f"{message.content}\n{notice}",
                allowed_mentions=discord.AllowedMentions.none(),
            )
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied Discord message annotation")
    except (
        discord.NotFound,
        discord.Forbidden,
        discord.HTTPException,
        AttributeError,
        TypeError,
        ValueError,
        OSError,
        TimeoutError,
    ):
        log.debug("could not annotate message %s", message_id, exc_info=True)
    return False


async def delete_debug_message(
    bot: object, channel_id: int | str | None, message_id: int | str
) -> bool:
    if channel_id is None:
        return False
    try:
        async with bot.repo.maintenance.operation("discord_debug_cleanup"):  # type: ignore[attr-defined]
            row = bot.repo._conn.execute(  # type: ignore[attr-defined]
                "SELECT channel_id FROM debug_messages WHERE message_id = ?", (str(message_id),)
            ).fetchone()
            if row is None or str(row["channel_id"]) != str(channel_id):
                return False
            channel = await _resolve_channel(bot, channel_id)
            try:
                message = await channel.fetch_message(int(message_id))
            except discord.NotFound:
                message = None
            if message is not None:
                try:
                    await message.delete()
                except discord.NotFound:
                    pass
            bot.repo.delivery._retire_debug_cleanup(  # type: ignore[attr-defined]
                message_id, channel_id, datetime.now(UTC)
            )
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied debug-message cleanup")
    except (
        discord.NotFound,
        discord.Forbidden,
        discord.HTTPException,
        AttributeError,
        TypeError,
        ValueError,
        OSError,
        TimeoutError,
        RuntimeError,
    ):
        log.debug("could not delete debug message %s", message_id, exc_info=True)
    return False


async def delete_decline_notice(
    bot: object, run_id: str, user_id: int | str, channel_id: int | str, message_id: int | str
) -> bool:
    try:
        async with bot.repo.maintenance.operation("discord_decline_cleanup"):  # type: ignore[attr-defined]
            journal = bot.repo.delivery  # type: ignore[attr-defined]
            row = bot.repo.get_decline_notice(run_id, user_id)  # type: ignore[attr-defined]
            if (
                row is None
                or str(row["channel_id"]) != str(channel_id)
                or str(row["message_id"]) != str(message_id)
            ):
                return False
            claim = journal._decline_retraction_claim(run_id, user_id, channel_id, message_id)
            if claim is None:
                return False
            channel = await _resolve_channel(bot, channel_id)
            if str(getattr(channel, "id", "")) != str(channel_id) or str(
                getattr(getattr(channel, "guild", None), "id", "")
            ) != str(claim["guild_id"]):
                return False
            try:
                message = await channel.fetch_message(int(message_id))
            except discord.NotFound:
                message = None
            if message is not None:
                actual_channel = getattr(message, "channel", None)
                actual_guild = getattr(actual_channel, "guild", None)
                if (
                    str(getattr(message, "id", "")) != str(message_id)
                    or str(getattr(actual_channel, "id", "")) != str(channel_id)
                    or str(getattr(actual_guild, "id", "")) != str(claim["guild_id"])
                ):
                    return False
                try:
                    await message.delete()
                except discord.NotFound:
                    pass
            journal._retire_decline_retraction(
                claim["attempt_id"], run_id, user_id, channel_id, message_id, datetime.now(UTC)
            )
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied decline-notice cleanup")
    except (
        discord.NotFound,
        discord.Forbidden,
        discord.HTTPException,
        AttributeError,
        TypeError,
        ValueError,
        OSError,
        TimeoutError,
        RuntimeError,
    ):
        log.debug("could not delete decline notice %s", message_id, exc_info=True)
    return False


async def retire_deleted_digest(
    bot: object,
    week_start: datetime,
    channel_id: int | str,
    message_id: int | str,
    *,
    at: datetime,
) -> bool:
    """Retire one exact bound digest only after its confirmed remote deletion."""
    try:
        async with bot.repo.maintenance.operation("digest_replacement"):  # type: ignore[attr-defined]
            journal = bot.repo.delivery  # type: ignore[attr-defined]
            claim = journal._digest_replacement_claim(week_start, channel_id, message_id)
            if claim is None:
                return False
            channel = await _resolve_channel(bot, channel_id)
            if str(getattr(channel, "id", "")) != str(channel_id) or str(
                getattr(getattr(channel, "guild", None), "id", "")
            ) != str(claim["guild_id"]):
                return False
            try:
                message = await channel.fetch_message(int(message_id))
            except discord.NotFound:
                message = None
            if message is not None:
                actual_channel = getattr(message, "channel", None)
                actual_guild = getattr(actual_channel, "guild", None)
                if (
                    str(getattr(message, "id", "")) != str(message_id)
                    or str(getattr(actual_channel, "id", "")) != str(channel_id)
                    or str(getattr(actual_guild, "id", "")) != str(claim["guild_id"])
                ):
                    return False
                try:
                    await message.delete()
                except discord.NotFound:
                    pass
            journal._retire_digest_replacement(
                claim["attempt_id"], week_start, channel_id, message_id, at
            )
        return True
    except MaintenanceClosedError:
        log.info("maintenance denied digest replacement")
    except (
        discord.NotFound,
        discord.Forbidden,
        discord.HTTPException,
        AttributeError,
        TypeError,
        ValueError,
        OSError,
        TimeoutError,
        RuntimeError,
    ):
        log.warning("could not safely replace digest %s", message_id, exc_info=True)
    return False


async def _resolve_channel(bot: object, channel_id: int | str):
    channel = bot.get_channel(int(channel_id))  # type: ignore[attr-defined]
    if channel is None:
        channel = await bot.fetch_channel(int(channel_id))  # type: ignore[attr-defined]
    return channel


__all__ = [
    "IMAGE_PREFIX",
    "DeliveryUncertainError",
    "add_chat_reaction",
    "add_reactions",
    "annotate_message",
    "delete_debug_message",
    "delete_decline_notice",
    "delete_placeholder",
    "edit_plain",
    "edit_card",
    "remove_reaction",
    "retire_deleted_digest",
    "remove_chat_reaction",
    "send_card",
    "send_operation_payload",
    "send_operation_plain",
    "send_plain",
]
