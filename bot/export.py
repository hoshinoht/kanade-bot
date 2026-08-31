"""``python -m bot.export`` -- dump watched channel history to JSONL.

Phase 2 turns real chat into ``tests/fixtures/*.json`` for prompt tuning, and
that needs the conversation on disk first.  This is the minimal version that
ships before ``bossctl`` exists (DESIGN.md §5, "Message export").

It logs in with a throwaway :class:`discord.Client` using the **bot token** --
never a user token, which would be self-botting and against Discord's ToS -- and
does nothing else: no command sync, no reminder tick, no posting.  Only channels
that pass :func:`bot.watch.is_watched` can be exported, and attachments are
recorded as ``[image]``/``[file]`` markers, never downloaded.

Exports land in git-ignored ``data/exports/``.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import re
import sys
from collections.abc import Iterable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

import discord

from .backfill import AccessDenied, iter_history, thread_list
from .config import Settings, get_settings
from .db import Repo
from .timeutil import to_iso
from .watch import is_watched, origin_ids
from .weeks import current_week_start

log = logging.getLogger("bot.export")

DEFAULT_EXPORT_DIR = Path("data/exports")
#: Reaction users are only needed to see who agreed; a huge reaction is noise.
REACTION_USER_CAP = 50

_SLUG_RE = re.compile(r"[^a-z0-9]+")


# ---------------------------------------------------------------------------
# pure helpers (unit tested)
# ---------------------------------------------------------------------------


def slugify(name: str) -> str:
    return _SLUG_RE.sub("-", (name or "channel").strip().lower()).strip("-") or "channel"


def parse_when(text: str, tz: ZoneInfo) -> datetime:
    """Parse ``YYYY-MM-DD`` or an ISO timestamp into an aware UTC datetime.

    A bare date means midnight *in the guild timezone*, which is what someone
    typing ``--since 2026-06-01`` means.
    """
    text = text.strip()
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        raise ValueError(
            f"could not read {text!r} as a date - use YYYY-MM-DD or an ISO timestamp"
        ) from None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=tz)
    return parsed.astimezone(UTC)


def default_out_path(
    channel_name: str, since: datetime, tz: ZoneInfo, out_dir: Path = DEFAULT_EXPORT_DIR
) -> Path:
    """``data/exports/<channel-name>-<since>.jsonl``, one file per channel."""
    stamp = since.astimezone(tz).strftime("%Y-%m-%d")
    return Path(out_dir) / f"{slugify(channel_name)}-{stamp}.jsonl"


def attachment_label(content_type: str | None, filename: str) -> str:
    """``[image] shot.png`` / ``[file] log.txt`` -- the name, never the bytes."""
    kind = "[image]" if (content_type or "").startswith("image/") else "[file]"
    return f"{kind} {filename}".strip()


def message_record(
    message: Any,
    channel_name: str,
    reactions: dict[str, list[int]] | None = None,
) -> dict:
    """Serialise one message. ``reactions`` is pre-fetched by the async caller."""
    channel_id, thread_id = origin_ids(message.channel)
    author = message.author
    reference = getattr(message, "reference", None)
    edited_at = getattr(message, "edited_at", None)
    record = {
        "id": str(message.id),
        "channel_id": str(channel_id),
        "channel_name": channel_name,
        "thread_id": str(thread_id) if thread_id else None,
        "author_id": str(author.id),
        "author_name": getattr(author, "display_name", None) or str(author),
        "author_bot": bool(getattr(author, "bot", False)),
        "created_at": to_iso(message.created_at),
        "content": message.content or "",
        "mentions": [str(u.id) for u in getattr(message, "mentions", []) or []],
        "reply_to": (
            str(reference.message_id)
            if reference is not None and getattr(reference, "message_id", None)
            else None
        ),
        "reactions": reactions or {},
        "attachments": [
            attachment_label(getattr(a, "content_type", None), getattr(a, "filename", ""))
            for a in getattr(message, "attachments", []) or []
        ],
    }
    if edited_at is not None:
        record["edited_at"] = to_iso(edited_at)
    return record


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m bot.export",
        description="Export watched Discord channel history to JSONL for fixture building.",
    )
    parser.add_argument(
        "--channel",
        action="append",
        type=int,
        default=[],
        metavar="ID",
        help="channel id to export (repeatable)",
    )
    parser.add_argument(
        "--category",
        action="append",
        type=int,
        default=[],
        metavar="ID",
        help="category id; every text channel under it is exported (repeatable)",
    )
    parser.add_argument(
        "--since",
        metavar="WHEN",
        help="YYYY-MM-DD or ISO timestamp (default: the current boss week's start)",
    )
    parser.add_argument("--until", metavar="WHEN", help="YYYY-MM-DD or ISO timestamp")
    parser.add_argument(
        "--out",
        metavar="PATH",
        help="output file; only valid when exactly one channel is selected",
    )
    return parser


# ---------------------------------------------------------------------------
# Discord side
# ---------------------------------------------------------------------------


def _resolve_channels(
    guild: discord.Guild, channel_ids: Iterable[int], category_ids: Iterable[int]
) -> list[discord.TextChannel]:
    """Expand explicit ids + categories into text channels, de-duplicated."""
    picked: dict[int, discord.TextChannel] = {}
    for cid in channel_ids:
        channel = guild.get_channel(cid)
        if channel is None:
            raise SystemExit(f"channel {cid} not found in the guild")
        if not isinstance(channel, discord.TextChannel):
            raise SystemExit(f"channel {cid} is not a text channel")
        picked[channel.id] = channel
    for cat_id in category_ids:
        category = guild.get_channel(cat_id)
        if not isinstance(category, discord.CategoryChannel):
            raise SystemExit(f"category {cat_id} not found in the guild")
        for channel in category.text_channels:
            picked[channel.id] = channel
    return list(picked.values())


async def _reactions(message: discord.Message) -> dict[str, list[int]]:
    out: dict[str, list[int]] = {}
    for reaction in message.reactions:
        try:
            out[str(reaction.emoji)] = [u.id async for u in reaction.users(limit=REACTION_USER_CAP)]
        except discord.HTTPException:  # pragma: no cover - network
            out[str(reaction.emoji)] = []
    return out


async def _export_channel(
    channel: discord.TextChannel,
    since: datetime,
    until: datetime | None,
    repo: Repo,
    path: Path,
) -> int:
    """Export one channel and its threads into ``path``.

    Raises :class:`AccessDenied` if the channel's history is unreadable, so the
    caller can skip it -- one private channel in a watched category must not
    abandon every channel after it.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    total = 0
    with path.open("w", encoding="utf-8") as handle:
        try:
            total += await _export_source(channel, channel.name, since, until, repo, handle)
        except discord.Forbidden as exc:
            raise AccessDenied(str(exc)) from exc
        for thread in await thread_list(channel):
            try:
                total += await _export_source(thread, channel.name, since, until, repo, handle)
            except discord.Forbidden:
                log.warning("no access to a thread in #%s; skipping it", channel.name)
    return total


async def _export_source(
    source: discord.abc.Messageable,
    channel_name: str,
    since: datetime,
    until: datetime | None,
    repo: Repo,
    handle,
) -> int:
    """One channel or thread -> JSONL lines, and rows in ``messages``.

    The JSONL keeps the bot's own messages (a fixture wants to see the card the
    conversation was answering), but they are deliberately *not* stored: the
    extractor must never read its own proposals back as chat. That is the same
    rule :func:`bot.backfill.record_source` and the live listener apply.
    """
    count = 0
    async for message in iter_history(source, since, until):
        record = message_record(message, channel_name, await _reactions(message))
        handle.write(json.dumps(record, ensure_ascii=False) + "\n")
        if not record["author_bot"]:
            # Keep the DB in step so a later rescan already has the history.
            repo.record_message(
                message.id,
                record["channel_id"],
                message.author.id,
                message.created_at,
                message.content or "",
            )
        count += 1
    return count


async def _run(settings: Settings, repo: Repo, args: argparse.Namespace) -> int:
    tz = settings.zoneinfo
    since = (
        parse_when(args.since, tz)
        if args.since
        else current_week_start(tz, settings.reset_weekday, settings.reset_time)
    )
    until = parse_when(args.until, tz) if args.until else None
    if until is not None and until <= since:
        raise SystemExit("--until must be after --since")

    intents = discord.Intents.default()
    intents.message_content = True
    intents.members = True
    client = discord.Client(intents=intents)
    status = {"code": 0}

    @client.event
    async def on_ready() -> None:
        try:
            guild = client.get_guild(settings.guild_id)
            if guild is None:
                raise SystemExit(f"guild {settings.guild_id} not visible to the bot")

            channels = _resolve_channels(guild, args.channel, args.category)
            if not channels:
                channels = _resolve_channels(
                    guild, settings.chat_channel_id_list, settings.chat_category_id_list
                )
            if not channels:
                raise SystemExit("nothing to export: no channels selected and none watched")

            unwatched = [
                c.name
                for c in channels
                if not is_watched(c, settings.chat_channel_id_list, settings.chat_category_id_list)
            ]
            if unwatched:
                raise SystemExit(
                    "refusing to export unwatched channel(s): "
                    + ", ".join(f"#{n}" for n in unwatched)
                    + " - add them to CHAT_CHANNEL_IDS or CHAT_CATEGORY_IDS first"
                )

            if args.out and len(channels) > 1:
                raise SystemExit(
                    f"--out takes a single file but {len(channels)} channels were "
                    "selected; narrow it with one --channel"
                )

            exported = skipped = 0
            for channel in channels:
                path = Path(args.out) if args.out else default_out_path(channel.name, since, tz)
                try:
                    total = await _export_channel(channel, since, until, repo, path)
                except AccessDenied:
                    # A private channel the bot was never added to; keep going
                    # rather than abandoning every channel after this one.
                    print(f"#{channel.name}: SKIPPED (no access)", file=sys.stderr)
                    path.unlink(missing_ok=True)
                    skipped += 1
                    continue
                print(f"#{channel.name}: {total} message(s) -> {path}")
                exported += 1
            print(f"done: {exported} channel(s) exported, {skipped} skipped")
            if skipped:
                status["code"] = 1
        except SystemExit as exc:
            print(str(exc), file=sys.stderr)
            status["code"] = 2
        except Exception:
            log.exception("export failed")
            status["code"] = 1
        finally:
            await client.close()

    try:
        await client.start(settings.discord_token)
    except discord.LoginFailure:
        print("Discord rejected DISCORD_TOKEN", file=sys.stderr)
        return 3
    except discord.PrivilegedIntentsRequired:
        print(
            "Message Content / Server Members intent is not enabled for this bot "
            "in the Discord developer portal - see README.md",
            file=sys.stderr,
        )
        return 4
    return status["code"]


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(
        level=logging.INFO, format="%(levelname)-8s %(name)s: %(message)s", stream=sys.stdout
    )
    logging.getLogger("discord").setLevel(logging.WARNING)
    args = build_parser().parse_args(argv)
    settings = get_settings()
    repo = Repo(settings.db_path)
    try:
        return asyncio.run(_run(settings, repo, args))
    finally:
        repo.close()


if __name__ == "__main__":
    raise SystemExit(main())
