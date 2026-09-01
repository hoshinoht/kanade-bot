"""Who changed the schedule, and from where.

The admin plane is one shared token (DESIGN.md §5, "Access control"), so until
now the most a change could say for itself was "applied via portal" -- true, and
useless a week later when the party wants to know who moved Thursday.  This
module is the seam between a mutation and the row that remembers it:

* :class:`Actor` is a surface plus whatever name that surface could resolve --
  a tailnet login, a Discord user id, the operating-system user behind
  ``bossctl``, or the literal ``token`` when the credential was all there was;
* :data:`CURRENT_ACTOR` carries the actor for the request in flight, set once by
  :class:`bot.api.app.ActorMiddleware`, so a service function does not have to
  be handed one at each of its thirty call sites (and a new route cannot forget
  to pass it);
* :func:`record` writes the row and **never raises**, because an audit trail
  that can fail a mutation is worse than no audit trail at all.

Nothing here validates a surface against :data:`bot.db.AUDIT_SURFACES`: a
mislabelled row is a bad log line, and refusing it would be a lost edit.
"""

from __future__ import annotations

import logging
from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:  # pragma: no cover
    from .db import Repo

log = logging.getLogger(__name__)

#: The header ``bossctl`` puts its operating-system user in. Attribution, not
#: authentication: the request still has to carry ADMIN_TOKEN, and
#: :func:`bot.api.app.resolve_actor` ignores this from anywhere but the machine
#: the bot is running on.
HEADER_BOSSCTL = "X-Bossctl-User"


@dataclass(frozen=True)
class Actor:
    """Where a change came from, and the closest thing to a name there was."""

    surface: str
    who: str

    def __str__(self) -> str:  # pragma: no cover - trivial
        return f"{self.who} ({self.surface})"


#: The bot acting on its own: a tick, a materialisation, a test calling a
#: service function directly. Named rather than left blank, so a row that names
#: nobody says *why* it names nobody.
SYSTEM = Actor("system", "system")

#: Whoever the request in flight belongs to. Task-local, so two overlapping
#: requests cannot borrow each other's identity.
CURRENT_ACTOR: ContextVar[Actor] = ContextVar("current_actor", default=SYSTEM)


def current() -> Actor:
    """The actor for whatever is happening right now, or :data:`SYSTEM`."""
    return CURRENT_ACTOR.get()


@contextmanager
def acting(actor: Actor) -> Iterator[Actor]:
    """Attribute everything inside the block to ``actor``.

    For callers that are not HTTP requests and so have no middleware to do it
    for them -- a slash command knows its ``interaction.user``, and that is a
    better answer than ``system``.
    """
    token = CURRENT_ACTOR.set(actor)
    try:
        yield actor
    finally:
        CURRENT_ACTOR.reset(token)


def record(
    repo: Repo,
    actor: Actor,
    action: str,
    subject: str | None = None,
    detail: str = "",
) -> None:
    """Write one audit row for a change that has already happened.

    Swallows everything, like the chat log's own write does: by the time this
    is called the run has moved, the card has been annotated and the party has
    been told, and unwinding all of that because a log insert failed would turn
    a missing line into a real incident.
    """
    try:
        repo.log_audit(
            surface=actor.surface,
            actor=actor.who,
            action=action,
            subject=subject,
            detail=detail,
        )
    except Exception:  # noqa: BLE001 - the trail must never break the mutation
        log.warning("could not record the audit row for %s", action, exc_info=True)


__all__ = [
    "CURRENT_ACTOR",
    "HEADER_BOSSCTL",
    "SYSTEM",
    "Actor",
    "acting",
    "current",
    "record",
]
