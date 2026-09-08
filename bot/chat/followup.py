"""What the bot says when somebody ❌s a card it asked for.

A member says "move HMaleficStar to Friday", the pilot posts a card, and the member
reacts ❌ -- which used to be the end of it. The card is dead, the bot never
mentions it again, and the member has to start the whole request over. So the
pilot asks: the card said *this*, what should it be instead? Their answer
arrives as a reply to the bot, which the ordinary gate already treats as a
mention, so the correction flows back through :meth:`ChatPilot.offer` with no
new path at all.

Two things make that safe to do from a *reaction*, which is not a message and
carries no words:

**Nothing here reads message text.** The question is generated from the
amendment row -- its kind, bosses, time and party, all written server-side when
the card was created -- and from the roster. A member cannot steer the follow-up
by writing anything, because the follow-up never looks at anything they wrote.

**The turn cannot post a card.** It runs with
:attr:`bot.chat.tools.ToolContext.read_only` set, so the write tools are both
withheld from the model and refused by the dispatcher. A follow-up that could
raise its own card would turn one ❌ into a second card to ❌.

The rest of this module is the gate: :func:`scope`, which decides whether a
rejection is the pilot's business at all, and which is a pure function of a row,
a reactor and the settings for exactly the reason :mod:`bot.chat.gate` is.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from bot.agent import formatting
from bot.domain.weeks import WEEKDAY_NAMES

from ..extract.commit import FIX_REMOVE
from . import gate

#: How long a channel is left alone after a follow-up is *started*, across
#: different cards. Somebody clearing out four stale cards in a row is one
#: action, not four questions, and thirty seconds is long enough to cover the
#: reactions of a person working down a channel while being short enough that a
#: genuine second rejection a minute later still gets asked about.
COOLDOWN_S = 30.0

__all__ = ["COOLDOWN_S", "Scope", "card_facts", "memory_note", "prompt", "scope"]


@dataclass(frozen=True)
class Scope:
    """Whether a rejection deserves a follow-up, and who it is addressed to."""

    act: bool
    #: For the log, never for Discord -- like :class:`bot.chat.gate.ChatDecision`.
    reason: str
    #: The member who asked for the card, from the interaction that created it.
    #: They are the only person it can be addressed to.
    author_id: str = ""

    def __bool__(self) -> bool:  # pragma: no cover - clarity at call sites
        return self.act


def scope(
    bot: Any,
    amendment: dict,
    *,
    reactor_id: int | str,
    channel: Any,
    enabled: bool,
) -> Scope:
    """May the pilot ask about this rejected card?

    Every check is silent when it fails, and the order is cheapest first:

    1. The feature is on -- ``chat_mode``, a configured pilot, and not quiet
       mode. Quiet mode means the bot is deliberately not talking in the guild,
       and a question is talking.
    2. The card is in one of the pilot's own channels
       (:func:`bot.chat.gate.is_chat_channel`). A card the pilot posted into a
       channel that has since left the allow-list gets no follow-up.
    3. The **chat pilot created it**. This is the check that keeps the extractor
       out: a card read out of ambient party chat has no interaction behind it
    (:meth:`bot.infrastructure.db.Repo.chat_interaction_for_amendment`), nobody asked the
       bot for it, and asking "what should it be instead?" about a guess the bot
       volunteered is noise.
    4. The reactor is the member who asked. A card names a party, and any of
       them may ❌ it -- but a question about what somebody *wanted* can only be
       put to the person who wanted something.
    """
    settings = bot.settings
    if not enabled or not settings.chat_pilot_configured:
        return Scope(False, "the chat pilot is off")
    if getattr(bot, "quiet_mode", False):
        return Scope(False, "quiet mode")
    if not gate.is_chat_channel(channel, settings):
        return Scope(False, "not a chat channel")

    interaction = bot.repo.chat_interaction_for_amendment(amendment["id"])
    if interaction is None:
        return Scope(False, "not a card the chat pilot created")
    author_id = str(interaction["author_id"] or "")
    if not author_id:
        return Scope(False, "the interaction names no author")
    if author_id != str(reactor_id):
        return Scope(False, "rejected by somebody other than the member who asked")
    return Scope(True, "ok", author_id)


def card_facts(bot: Any, amendment: dict) -> str:
    """What the card said, in one line, from the row rather than from Discord.

    The ``summary`` column first, because that is the sentence the write tool
    wrote when it raised the card ("move Hard Will to Sun 07 Sep 22:00") and it
    is already the card's own account of itself. Everything else is a fallback
    for a row that has none.

    The party is spelled out by roster name, never as a mention: the model is
    being told who is on the run so it can talk about them, and handing it
    ``<@1002>`` teaches it to write pings -- which is the one thing
    :data:`bot.chat.persona.HARD_RULES` rule 8 forbids.
    """
    from ..api import service

    run = bot.repo.get_run(amendment["run_id"]) if amendment["run_id"] else None
    said = (amendment.get("summary") or "").strip() or _fallback(bot, amendment, run)
    people = amendment["participants"] or (run["participants"] if run else [])
    party = ", ".join(service.member_name(bot, uid) for uid in people)
    return f"{said}{f' — for {party}' if party else ''}"


def _fallback(bot: Any, amendment: dict, run: dict | None) -> str:
    """A card's facts for a row whose ``summary`` is empty."""
    payload = amendment.get("payload") or {}
    kind = amendment["kind"]
    bosses = formatting.boss_labels(run["bosses"] if run is not None else amendment["bosses"])
    if kind == "fix" and payload.get("op") == FIX_REMOVE:
        weekly = payload.get("weekly_when")
        return f"stop scheduling {bosses}{f' every {weekly}' if weekly else ' every week'}"
    verb = formatting.KIND_VERB.get(kind, kind)
    weekday, hhmm = payload.get("weekday"), payload.get("time")
    if kind == "fix" and weekday is not None and hhmm:
        return f"{verb} {bosses} every {WEEKDAY_NAMES[int(weekday)]} {hhmm}"
    when = formatting.when_text(amendment, bot.tz).replace("**", "")
    return f"{verb} {bosses} {when}".strip()


def memory_note(bot: Any, amendments: list[dict]) -> str:
    """What the channel's memory keeps about the card that was rejected.

    The follow-up itself is remembered as the assistant turn it was, but on its
    own it is a question with no subject: "what should it be instead?" tells the
    model nothing about what "it" was, and the member's answer -- "friday, and
    put priya on it" -- arrives as a correction to nothing.

    Kept short and marked ``[Note]`` rather than written as a line of dialogue,
    because nobody said it -- the bracket does that job, since the turn itself is
    remembered as a ``user`` one for the same reason :func:`prompt` is. It is
    remembered, so it is trimmed by the ordinary budget and forgotten with the
    rest of the conversation.
    """
    return (
        "[Note] The card you posted was rejected and is off; nothing changed. It said: "
        + "; ".join(card_facts(bot, a) for a in amendments)
        + ". You have asked what it should be instead."
    )


def prompt(bot: Any, amendments: list[dict], author_id: str) -> str:
    """The synthetic turn the model answers, built entirely from the rows.

    Written as a note *to* the bot rather than as a line from the member: nobody
    said any of this. A member's own words never reach it, so there is nothing in
    it for a message to steer -- the facts come from the amendment rows and the
    names from the roster.

    Fed in as a ``user`` turn all the same, which is a fact about the runtime
    rather than about the note. Ollama's gpt-oss template skips system messages
    in the message loop and concatenates them into the instructions header at the
    top of the prompt, so a ``system`` turn does not stay where it was put -- and
    a note about a reaction that just happened is worthless before the
    conversation it reacted to. The bracketed opener keeps the provenance
    explicit where the role no longer can.

    It ends by saying what the answer is for, because a small model handed a
    rejection will otherwise apologise and stop, and an apology asks nothing.
    """
    from ..api import service

    who = service.member_name(bot, author_id)
    facts = "; ".join(card_facts(bot, a) for a in amendments)
    return (
        f"[Note from the scheduler, not from anybody in the channel.] {who} just reacted ❌ "
        f"on the card you posted for them, so it is off and nothing has changed. The card "
        f"said: {facts}. Write one short message to {who}: say the card is off, and ask what "
        f"they would like instead -- the day, the time, the boss, whoever is on it, whichever "
        f"of those you cannot tell from the card. Ask, do not guess, and do not apologise at "
        f"length. You cannot post a card in this message; their reply comes back to you as a "
        f"normal message and you can post the corrected card then."
    )
