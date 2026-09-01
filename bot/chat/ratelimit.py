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

    def retry_after(self, user_id: int | str) -> float:
        """Seconds until the next slot frees, or ``0.0`` when one is free now.

        The oldest live hit is the one that expires first, so its age is what the
        wait is measured from. Non-mutating, like :meth:`remaining` and
        :meth:`snapshot`: telling somebody when to come back must not itself cost
        them the answer they came back for.

        An unknown key, or one with room left, is ``0.0`` -- "no wait" rather
        than "a whole window", so a caller can use it without first asking
        whether there was anything to wait for.
        """
        now = self._clock()
        cutoff = now - self.window
        live = [stamp for stamp in self._hits.get(str(user_id), ()) if stamp > cutoff]
        if len(live) < self.count:
            return 0.0
        return max(live[0] + self.window - now, 0.0)

    def snapshot(self) -> dict[str, int]:
        """``{user_id: answers used}`` for every window still open. Never mutates.

        For the portal's Limits page, which wants to show who is currently inside
        a window. A public reader because the two alternatives are both wrong:
        reading ``_hits`` from outside is one rename away from breaking, and
        asking :meth:`allow` would charge somebody for the question.

        Keys with nothing live are left out, so the size of the result is "how
        many people are mid-window" rather than "how many have ever asked".
        """
        cutoff = self._clock() - self.window
        live = {key: sum(1 for stamp in hits if stamp > cutoff) for key, hits in self._hits.items()}
        return {key: used for key, used in live.items() if used}

    def reset(self, user_id: int | str | None = None) -> None:
        if user_id is None:
            self._hits.clear()
        else:
            self._hits.pop(str(user_id), None)
