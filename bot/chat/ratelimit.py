"""A per-person sliding window, so one member cannot monopolise the model.

Deliberately in memory: a restart forgetting who has asked what is the right
trade for a pilot, and the alternative is a table whose rows are worthless
within five minutes.  The limit is on *answers*, not on messages -- anything the
gate turned away never reaches here, so being ignored costs nobody their quota.

Most keys run on the shared ``count``/``window``; a key with an entry in
:meth:`RateLimiter.overrides` runs on its own pair instead
(:meth:`RateLimiter.limit_for`), which is how one member is given a bigger
allowance than the guild's. The map is held here and filled from outside --
:meth:`bot.chat.agent.ChatPilot.apply_limits` loads it from the database -- so
this class stays what it has always been: a clock, some deques, and no idea
where any of its numbers came from.
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
        #: user id -> its own ``(count, window)``. Sparse: everybody not in here
        #: is on the shared pair above.
        self._overrides: dict[str, tuple[int, float]] = {}

    # -- per-key allowances -------------------------------------------------
    def limit_for(self, user_id: int | str) -> tuple[int, float]:
        """The ``(count, window)`` this key actually runs on.

        The one place the override is applied, so every method below asks it
        rather than reading ``self.count`` and quietly limiting an overridden
        member by the default.
        """
        return self._overrides.get(str(user_id), (self.count, self.window))

    def set_override(self, user_id: int | str, count: int, window: float) -> None:
        """Give one key its own allowance, replacing any it had.

        Takes effect on the *next* call, against the hits already recorded:
        somebody mid-window whose count is raised is not given a fresh window,
        they are given more room in the one they are in. That is the reading
        that matches what an operator is doing when they raise it -- "let them
        keep going" -- and it means lowering a count can refuse the very next
        message, which is also what they asked for.
        """
        self._overrides[str(user_id)] = (int(count), float(window))

    def clear_override(self, user_id: int | str) -> bool:
        """Put one key back on the shared allowance. False if it already was."""
        return self._overrides.pop(str(user_id), None) is not None

    def overrides(self) -> dict[str, tuple[int, float]]:
        """Every key with its own allowance. A copy, for the portal to render."""
        return dict(self._overrides)

    def replace_overrides(self, overrides: dict[str, tuple[int, float]]) -> None:
        """Swap the whole map, as loading it from the database does.

        Whole-map rather than merged, so the limiter after a load says exactly
        what the table says -- an override deleted elsewhere does not survive in
        memory because nothing thought to remove it.
        """
        self._overrides = {str(key): (int(c), float(w)) for key, (c, w) in overrides.items()}

    # -- the window ---------------------------------------------------------
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
        count, window = self.limit_for(key)
        hits = self._hits.setdefault(key, deque())
        cutoff = now - window
        while hits and hits[0] <= cutoff:
            hits.popleft()
        if len(hits) >= count:
            return False
        hits.append(now)
        return True

    def remaining(self, user_id: int | str) -> int:
        """How many more answers this user may have in the current window."""
        now = self._clock()
        count, window = self.limit_for(user_id)
        hits = self._hits.get(str(user_id))
        if not hits:
            return count
        live = sum(1 for stamp in hits if stamp > now - window)
        return max(count - live, 0)

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
        count, window = self.limit_for(user_id)
        cutoff = now - window
        live = [stamp for stamp in self._hits.get(str(user_id), ()) if stamp > cutoff]
        if len(live) < count:
            return 0.0
        return max(live[0] + window - now, 0.0)

    def resets_in(self, user_id: int | str) -> float:
        """Seconds until this key's oldest live hit expires, or ``0.0`` if none is.

        The same arithmetic as :meth:`retry_after` on the same oldest hit, and
        the two differ only in whether a window with room left counts as a wait.
        It does not, to somebody being refused -- "come back in 200 s" when they
        may ask right now is simply wrong -- and it does, to somebody being
        *shown* their window by ``/limits``, who wants to know when the answer
        they have already spent comes back to them.

        Non-mutating, like every other reader here: reading your own allowance
        must not cost you any of it.
        """
        now = self._clock()
        window = self.limit_for(user_id)[1]
        live = [stamp for stamp in self._hits.get(str(user_id), ()) if stamp > now - window]
        return max(live[0] + window - now, 0.0) if live else 0.0

    def snapshot(self) -> dict[str, int]:
        """``{user_id: answers used}`` for every window still open. Never mutates.

        For the portal's Limits page, which wants to show who is currently inside
        a window. A public reader because the two alternatives are both wrong:
        reading ``_hits`` from outside is one rename away from breaking, and
        asking :meth:`allow` would charge somebody for the question.

        Counted against each key's *own* window, so a member with a ten-minute
        allowance is not reported through the guild's five-minute one. Keys with
        nothing live are left out, so the size of the result is "how many people
        are mid-window" rather than "how many have ever asked".
        """
        now = self._clock()
        live = {}
        for key, hits in self._hits.items():
            cutoff = now - self.limit_for(key)[1]
            used = sum(1 for stamp in hits if stamp > cutoff)
            if used:
                live[key] = used
        return live

    def reset(self, user_id: int | str | None = None) -> None:
        """Forget the hits. An override is an allowance, not a window, and stays."""
        if user_id is None:
            self._hits.clear()
        else:
            self._hits.pop(str(user_id), None)
