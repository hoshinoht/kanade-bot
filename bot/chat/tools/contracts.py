"""Canonical tool contracts, outcomes, and shared result constants."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

#: How many runs a schedule answer lists before it is truncated. A boss week is
#: ten-ish runs; a model handed fifty lines starts summarising them wrongly.
MAX_RUNS = 20

#: What the model is told when it asks for something that is not a tool. Phrased
#: as an instruction because a bare "error" makes a small model retry the same
#: call; naming the real tools makes it pick one.
UNKNOWN_TOOL = (
    "There is no tool called {name}. The tools you have are: {known}. "
    "Use one of those when it can answer the request."
)

#: What a read-only turn tells the model when it reaches for a card anyway.
#: Phrased as the thing to do instead, for the reason :data:`UNKNOWN_TOOL` is:
#: a bare refusal makes a small model try the same call again.
READ_ONLY_TURN = (
    "You cannot post a card in this message. Ask them in words what it should be "
    "instead, and stop there -- their answer comes back to you as a normal "
    "message and you can post the card then."
)

#: What went wrong with a tool call, in the words the log uses.
REFUSED = "refused"
UNKNOWN = "unknown tool"
FAILED = "failed"


class ToolError(Exception):
    """A refusal the model is allowed to read, and should say something about."""


@dataclass
class ToolContext:
    """Who is asking, from where -- everything a write needs for provenance.

    ``author_id`` is the Discord id of the person who wrote the triggering
    message, taken from the message rather than from anything the model said. It
    is what makes "RSVP for somebody else" impossible rather than merely
    discouraged.
    """

    bot: Any
    author_id: str
    channel_id: str
    message_id: str
    #: Does the author run this bot (:func:`bot.agent.util.is_bot_admin`)? Decided from
    #: the live member object by :meth:`bot.chat.agent.ChatPilot._is_admin` and
    #: carried here as a fact, never re-derived from anything the model said. It
    #: is the one exemption from :func:`bot.chat.tools.authority.require_authority`;
    #: it defaults to ``False`` so any context built without one is the
    #: least-privileged one.
    is_admin: bool = False
    #: No card may be posted in this turn, whatever the model asks for. Set for
    #: the bot's *own* turns -- the rejection follow-up
    #: (:mod:`bot.chat.followup`), which is generated from a card rather than
    #: from anything a member said and must only ever produce a question. It is
    #: enforced in :func:`bot.chat.tools.run`, not by withholding the schemas
    #: alone, so a model that names a write tool from memory is refused rather
    #: than obeyed.
    read_only: bool = False
    #: Amendment ids this turn created, so the agent can report accurately.
    created: list[str] = None  # type: ignore[assignment]
    #: Amendment ids whose proposal cards were successfully posted. A row can be
    #: created before Discord rejects its card, so ``created`` is not proof that
    #: anybody can see or confirm a proposal.
    posted: list[str] = None  # type: ignore[assignment]
    #: The two Discord identities that can legitimately be used to address the
    #: bot. They are resolved from the live message before the model runs, then
    #: carried here so a copied trigger mention can never be mistaken for a
    #: roster member. Kept after the original fields so positional callers retain
    #: their old meaning.
    bot_user_id: str | None = None
    self_role_id: str | None = None
    #: Trusted defaults derived from the Discord message before the model runs.
    #: Kept separate because "my runs across all channels" is global in one
    #: dimension and explicitly personal in the other.
    force_all_channels: bool = False
    force_group_schedule: bool = False

    def __post_init__(self) -> None:
        if self.created is None:
            self.created = []
        if self.posted is None:
            self.posted = []


@dataclass
class ToolOutcome:
    """One tool call, as the log wants to describe it.

    Exists because "why did it propose that?" is answered by the *arguments* the
    model passed and whether the tool did as it was told -- neither of which
    survives being flattened into the answer string the model reads.
    """

    name: str
    output: str
    arguments: dict = field(default_factory=dict)
    #: The tool did what was asked. A refusal is a *successful* refusal only in
    #: the sense that it did not crash; `ok` is False for all three failures.
    ok: bool = True
    error: str | None = None
    duration_ms: int = 0
    #: Amendment ids this one call created, so a card can be traced to the call.
    created: list[str] = field(default_factory=list)
    #: The subset of ``created`` whose cards made it into Discord. A created row
    #: without one is diagnostic state, not a proposal a person can confirm.
    posted: list[str] = field(default_factory=list)
    #: The one-based model round that requested this call, set by the pilot.
    round: int = 0

    @property
    def outcome(self) -> str:
        return "ok" if self.ok else (self.error or FAILED)
