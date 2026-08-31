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

    ==================  ==========================================  ============
    from                tally                                       result
    ==================  ==========================================  ============
    any                 a participant declined                      ``at_risk``
    any                 every participant said yes                  ``confirmed``
    ``confirmed``       anything else (incomplete, or a retraction)  ``confirmed``
    anything else       anything else                               ``planned``
    :data:`STICKY_STATUSES`  (not consulted)                        unchanged
    ==================  ==========================================  ============

    The ``confirmed`` row is the important one. A run reaches ``confirmed``
    either by a full tally or because somebody decided it was on, and a decision
    is not undone by a tally that is merely *incomplete*: the owner confirmed a
    run with two of four answers in, a rescan applied one more chat "yes", and
    the recomputation silently put it back to ``planned``. Only an explicit
    "no" argues against a confirmed run; silence does not.

    A change of *line-up* does undo it, because the person who just joined never
    agreed to anything -- but that is not visible from the tally, so the caller
    handles it (:func:`recompute_after_roster_change`).
    """
    if current_status in STICKY_STATUSES:
        return current_status
    relevant = {uid: state for uid, state in rsvps.items() if uid in participants}
    if any(state == "no" for state in relevant.values()):
        return "at_risk"
    if participants and all(relevant.get(uid) == "yes" for uid in participants):
        return "confirmed"
    if current_status == "confirmed":
        return "confirmed"
    return "planned"


def recompute_after_roster_change(repo: Repo, run_id: str) -> str:
    """Re-derive a run's status after its participants changed; returns the status.

    The one case where a ``confirmed`` run *should* lose it: whoever just joined
    never agreed to the run, so it goes back to being whatever the tally says.
    Done here rather than by weakening :func:`compute_status`, which cannot tell
    an incomplete tally from a changed line-up -- and keeping a manual confirm
    through an incomplete tally is the whole point of it.
    """
    run = repo.get_run(run_id)
    if run is None:  # pragma: no cover - the caller just wrote to it
        return "planned"
    derive_from = "planned" if run["status"] == "confirmed" else run["status"]
    status = compute_status(derive_from, run["participants"], repo.get_rsvps(run_id))
    if status != run["status"]:
        repo.set_run_status(run_id, status)
    return status


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
