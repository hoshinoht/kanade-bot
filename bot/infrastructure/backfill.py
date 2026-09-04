"""Pulling channel history into the ``messages`` table.

Both ``python -m bot.export`` and the live bot need the same three things: the
channel plus its threads, one pass over ``history()`` oldest-first, and a row in
``messages`` for every human message.  Keeping that in one place means an export
and a `/rescan` backfill can never disagree about which channel a thread's
messages belong to, or about which messages count.

Nothing here posts, edits or reacts -- it only reads history and writes rows.
"""

from __future__ import annotations

import logging
from collections.abc import AsyncIterator, Callable
from datetime import datetime
from typing import Any

import discord

from .db import Repo
from .watch import origin_ids

log = logging.getLogger(__name__)


class AccessDenied(Exception):
    """The bot cannot read a channel's history."""


async def thread_list(channel: Any) -> list[Any]:
    """A channel's active and archived public threads, de-duplicated.

    Archived threads are a separate paginated endpoint; a channel the bot can
    read but whose archive it cannot list still yields its active threads
    rather than failing the whole backfill.
    """
    threads = list(getattr(channel, "threads", []) or [])
    seen = {t.id for t in threads}
    archived = getattr(channel, "archived_threads", None)
    if archived is None:
        return threads
    try:
        async for thread in archived(private=False, limit=None):
            if thread.id not in seen:
                threads.append(thread)
                seen.add(thread.id)
    except (discord.Forbidden, discord.HTTPException):
        log.warning("could not list archived threads for #%s", getattr(channel, "name", "?"))
    return threads


async def iter_history(
    source: Any, since: datetime, until: datetime | None = None
) -> AsyncIterator[Any]:
    """Every message in ``[since, until)``, oldest first.

    One sequential pass per source: discord.py handles the rate limiting, and
    fanning several channels out concurrently only makes it throttle harder.
    """
    async for message in source.history(after=since, before=until, oldest_first=True, limit=None):
        yield message


async def record_source(
    repo: Repo,
    source: Any,
    since: datetime,
    until: datetime | None = None,
    on_message: Callable[[Any], None] | None = None,
) -> int:
    """Upsert one channel-or-thread's history; returns how many rows it covered.

    Bot messages are skipped -- the bot's own reminders and cards must never
    become chat the extractor reads back. Messages are filed under the *parent*
    channel (:func:`bot.infrastructure.watch.origin_ids`), so a thread's planning groups with
    its channel's.
    """
    count = 0
    async for message in iter_history(source, since, until):
        if getattr(message.author, "bot", False):
            continue
        channel_id, _thread_id = origin_ids(message.channel)
        repo.record_message(
            message.id,
            channel_id,
            message.author.id,
            message.created_at,
            message.content or "",
        )
        if on_message is not None:
            on_message(message)
        count += 1
    return count


async def record_channel(
    repo: Repo,
    channel: Any,
    since: datetime,
    until: datetime | None = None,
    on_message: Callable[[Any], None] | None = None,
) -> int:
    """Backfill a channel and its threads. Idempotent -- ``record_message`` ignores duplicates.

    Raises :class:`AccessDenied` when the channel's own history is unreadable,
    so a caller sweeping a whole category can skip it and keep going. A thread
    the bot cannot read is skipped quietly.
    """
    try:
        total = await record_source(repo, channel, since, until, on_message)
    except discord.Forbidden as exc:
        raise AccessDenied(str(exc)) from exc
    for thread in await thread_list(channel):
        try:
            total += await record_source(repo, thread, since, until, on_message)
        except discord.Forbidden:
            log.warning("no access to a thread in #%s; skipping it", getattr(channel, "name", "?"))
    return total


__all__ = ["AccessDenied", "iter_history", "record_channel", "record_source", "thread_list"]
