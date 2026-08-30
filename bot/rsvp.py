"""RSVP + run-status logic.

Kept free of Discord objects so the reaction handling can be unit tested against
an in-memory database: :mod:`bot.client` only translates raw reaction events
into ``(run, user_id, emoji, added)`` and calls in here.
"""

from __future__ import annotations

from dataclasses import dataclass

from .db import Repo

EMOJI_YES = "✅"  # white heavy check mark
EMOJI_NO = "❌"  # cross mark

#: Statuses the RSVP tally must not overwrite -- they were set deliberately.
STICKY_STATUSES = ("cancelled", "otot", "done")


def state_for_emoji(emoji: str) -> str | None:
    """``✅`` -> ``"yes"``, ``❌`` -> ``"no"``, anything else -> ``None``."""
    if emoji == EMOJI_YES:
        return "yes"
    if emoji == EMOJI_NO:
        return "no"
    return None


def compute_status(current_status: str, participants: list[str], rsvps: dict[str, str]) -> str:
    """Derive a run's status from its RSVP tally.

    * anyone declined -> ``at_risk``
    * every participant said yes -> ``confirmed``
    * otherwise -> ``planned``

    Statuses in :data:`STICKY_STATUSES` are returned unchanged.
    """
    if current_status in STICKY_STATUSES:
        return current_status
    relevant = {uid: state for uid, state in rsvps.items() if uid in participants}
    if any(state == "no" for state in relevant.values()):
        return "at_risk"
    if participants and all(relevant.get(uid) == "yes" for uid in participants):
        return "confirmed"
    return "planned"


@dataclass
class ReactionResult:
    """What a single reaction did, so the caller knows whether to post anything."""

    run_id: int
    applied: bool = False
    state: str | None = None
    old_status: str = "planned"
    new_status: str = "planned"

    @property
    def declined(self) -> bool:
        return self.applied and self.state == "no"

    @property
    def status_changed(self) -> bool:
        return self.applied and self.old_status != self.new_status


def apply_reaction(
    repo: Repo, run: dict, user_id: int | str, emoji: str, added: bool
) -> ReactionResult:
    """Record (or undo) one ✅/❌ reaction and recompute the run's status.

    Reactions from people who are not participants of the run are ignored.
    """
    result = ReactionResult(run_id=run["id"], old_status=run["status"], new_status=run["status"])
    state = state_for_emoji(emoji)
    if state is None:
        return result
    user_id = str(user_id)
    if user_id not in run["participants"]:
        return result

    if added:
        repo.set_rsvp(run["id"], user_id, state, source="reaction")
    else:
        # Only clear if the removed reaction is the one currently recorded, so
        # un-reacting ❌ after switching to ✅ does not wipe the ✅.
        if repo.get_rsvps(run["id"]).get(user_id) != state:
            return result
        repo.clear_rsvp(run["id"], user_id)

    result.applied = True
    result.state = state if added else None
    new_status = compute_status(run["status"], run["participants"], repo.get_rsvps(run["id"]))
    if new_status != run["status"]:
        repo.set_run_status(run["id"], new_status)
    result.new_status = new_status
    return result
