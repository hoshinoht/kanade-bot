"""Which channels the bot listens to.

``is_watched`` is the single gate used by the message listener (and, later, by
backfill and export).  It is resolved per message rather than from a cached list,
so a channel created under a watched category is picked up without a restart.

Kept free of Discord types -- it only duck-types ``.id``, ``.category_id`` and
``.parent`` -- so it can be unit tested with stand-in objects.
"""

from __future__ import annotations

from collections.abc import Collection
from typing import Any


def _as_int(value: Any) -> int | None:
    if value is None or isinstance(value, bool):
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _matches(channel: Any, channel_ids: Collection[int], category_ids: Collection[int]) -> bool:
    if _as_int(getattr(channel, "id", None)) in channel_ids:
        return True
    return _as_int(getattr(channel, "category_id", None)) in category_ids


def is_watched(channel: Any, channel_ids: Collection[int], category_ids: Collection[int]) -> bool:
    """True if ``channel`` is watched explicitly, via its category, or via its parent.

    A thread counts as its parent channel, so a thread under a watched category
    (or in an explicitly watched channel) is watched too.
    """
    if channel is None:
        return False
    channel_ids = {i for i in map(_as_int, channel_ids) if i is not None}
    category_ids = {i for i in map(_as_int, category_ids) if i is not None}
    if not channel_ids and not category_ids:
        return False
    if _matches(channel, channel_ids, category_ids):
        return True
    # discord.Thread carries the text channel it lives in as `.parent`.
    parent = getattr(channel, "parent", None)
    return parent is not None and _matches(parent, channel_ids, category_ids)


def origin_ids(channel: Any) -> tuple[int, int | None]:
    """``(parent_channel_id, thread_id)`` for a channel or thread.

    A thread is reported under the channel it lives in, so its messages group
    with that channel's rather than looking like a separate party. The live
    listener and `python -m bot.export` both use this, so `messages.channel_id`
    means the same thing whichever wrote the row.
    """
    parent = getattr(channel, "parent", None)
    if parent is not None:
        return int(parent.id), int(channel.id)
    return int(channel.id), None
