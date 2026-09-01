"""A per-person sliding window, so one member cannot monopolise the model.

Deliberately in memory: a restart forgetting who has asked what is the right
trade for a pilot, and the alternative is a table whose rows are worthless
within five minutes.  The limit is on *answers*, not on messages -- anything the
gate turned away never reaches here, so being ignored costs nobody their quota.
"""

from __future__ import annotations

import time
from collections import deque

__all__ = ["RateLimiter"]


class RateLimiter:
    """``count`` allowances per ``window`` seconds, per user id."""

    def __init__(self, count: int, window: float, clock=time.monotonic):
        self.count = int(count)
        self.window = float(window)
        self._clock = clock
        self._hits: dict[str, deque[float]] = {}

    def allow(self, user_id: int | str, exempt: bool = False) -> bool:
        """Record an answer for ``user_id`` and say whether it may go out.

        ``exempt`` (the admin role) short-circuits before anything is recorded,
        so a developer testing the bot never fills up somebody's window and
        never has one of their own.
        """
        if exempt:
            return True
        now = self._clock()
        key = str(user_id)
        hits = self._hits.setdefault(key, deque())
        cutoff = now - self.window
        while hits and hits[0] <= cutoff:
            hits.popleft()
        if len(hits) >= self.count:
            return False
        hits.append(now)
        return True

    def remaining(self, user_id: int | str) -> int:
        """How many more answers this user may have in the current window."""
        now = self._clock()
        hits = self._hits.get(str(user_id))
        if not hits:
            return self.count
        live = sum(1 for stamp in hits if stamp > now - self.window)
        return max(self.count - live, 0)

    def reset(self, user_id: int | str | None = None) -> None:
        if user_id is None:
            self._hits.clear()
        else:
            self._hits.pop(str(user_id), None)
