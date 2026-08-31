"""Who actually gets an @mention, in one place (DESIGN.md §3, "Mention policy").

A notification is the bot's most expensive act: it lights up a phone. So it is
spent only where the person has to *do* something -- answer a run, decide
whether to re-plan -- and every other post names people in plain text instead.

Every posting path resolves its mentions here rather than passing a participant
list straight to ``allowed_mentions``, so the policy is one table rather than a
judgement call repeated at a dozen call sites.
"""

from __future__ import annotations

import logging
from collections.abc import Iterable, Sequence

from .db import DEFAULT_PING_LEVEL, PING_LEVELS, Repo
from .formatting import Audience

log = logging.getLogger(__name__)

#: The posts that ask somebody to act. These mention everyone in their
#: candidate list except the people who chose ``off``.
#:
#: * ``day_of``    -- the morning card: RSVP for tonight.
#: * ``countdown`` -- T-1h/T-15m, and only the people who have not answered yet.
#: * ``proposal``  -- an extractor card someone has to ✅ or ❌.
#: * ``decline``   -- "X can't make it": the rest of the run decide whether to re-plan.
ESSENTIAL_KINDS = frozenset({"day_of", "countdown", "proposal", "decline"})

#: Posts that report something already decided. Nobody is mentioned unless they
#: asked to be (``all``); everyone else is named in plain text.
INFORMATIONAL_KINDS = frozenset(
    {
        "amend",  # a move that has already been applied
        "status",  # cancelled / own-time / done / back on
        "swap",  # a one-week stand-in
        "fixed",  # a weekly timing added, changed or removed
        "digest",  # the whole guild's week
        "retract",  # "they're back in"
        "rescan",  # "I'll post the cards here"
        "test",  # /debug ping
    }
)

PING_KINDS = ESSENTIAL_KINDS | INFORMATIONAL_KINDS

__all__ = [
    "DEFAULT_PING_LEVEL",
    "ESSENTIAL_KINDS",
    "INFORMATIONAL_KINDS",
    "PING_KINDS",
    "PING_LEVELS",
    "audience",
    "display_names",
    "normalise_level",
    "resolve_mentions",
]


def normalise_level(value: str | None) -> str:
    """Validate a level typed by a person; raises :class:`ValueError` if it is not one."""
    level = (value or "").strip().lower()
    if level not in PING_LEVELS:
        raise ValueError(f"ping level must be one of {', '.join(PING_LEVELS)}, not `{value}`")
    return level


def wants_mention(level: str, kind: str) -> bool:
    """The whole policy, as one predicate.

    * ``off``       -- never, anywhere. Their choice; they still appear by name.
    * ``all``       -- every post that lists them.
    * ``essential`` -- only the posts in :data:`ESSENTIAL_KINDS` (the default).
    """
    if level == "off":
        return False
    if level == "all":
        return True
    return kind in ESSENTIAL_KINDS


def resolve_mentions(repo: Repo, candidates: Iterable[int | str], kind: str) -> list[str]:
    """The subset of ``candidates`` this ``kind`` of post may actually @mention.

    Order is preserved and duplicates dropped, so the caller can hand this
    straight to ``discord.AllowedMentions(users=...)``.
    """
    if kind not in PING_KINDS:
        # Never guess loudly: an unrecognised kind is treated as informational,
        # so a typo costs a missing ping rather than a burst of notifications.
        log.warning("unknown ping kind %r; treating it as informational", kind)
    out: list[str] = []
    for candidate in candidates:
        uid = str(candidate)
        if uid in out:
            continue
        if wants_mention(repo.get_ping_level(uid), kind):
            out.append(uid)
    return out


def display_names(repo: Repo, user_ids: Iterable[int | str]) -> dict[str, str]:
    """``user_id -> what to call them`` for everyone who is not being mentioned.

    Their ``/nick`` alias is deliberately not used: an alias is what the
    extractor matches in chat ("MY"), not necessarily what the party would
    recognise in a post.
    """
    names: dict[str, str] = {}
    for user_id in user_ids:
        uid = str(user_id)
        if uid in names:
            continue
        member = repo.get_member(uid)
        if member:
            name = member["nickname"] or member["display_name"]
            if name:
                names[uid] = name
    return names


def audience(
    repo: Repo,
    people: Sequence[int | str],
    kind: str,
    candidates: Iterable[int | str] | None = None,
) -> Audience:
    """Everything a card needs to render its people: names, and who to ping.

    ``people`` is everyone the post lists; ``candidates`` (default: all of them)
    is the subset the post *would* ping if they all wanted it -- a countdown
    lists the whole party but only ever pings the ones who have not answered.
    """
    listed = [str(p) for p in people]
    wanted = listed if candidates is None else [str(c) for c in candidates]
    return Audience(
        names=display_names(repo, listed),
        mentioned=tuple(resolve_mentions(repo, wanted, kind)),
    )
