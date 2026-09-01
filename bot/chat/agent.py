"""The conversation loop: gate, assemble, call the model, post one reply.

Structured so that the interesting parts are testable without a gateway or a
model. :meth:`ChatPilot.offer` is the only entry point and does no reasoning of
its own -- it asks :mod:`bot.chat.gate` whether to answer, holds a per-channel
lock so one channel cannot have two answers in flight, takes the host's one
model lock (:mod:`bot.modellock`) so the whole machine cannot, and hands the
assembled conversation to :meth:`ChatPilot.generate`, which is the part with the
model in it.

Nothing here is allowed to take the bot down, and nothing here is allowed to be
slow forever: the whole answer, tool rounds included, lives inside one
``asyncio.wait_for``. A model that is offline, wedged, or looping produces a
short apology in the channel and a full traceback in the log.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
import re
import time
from collections import deque
from collections.abc import Sequence
from dataclasses import dataclass, field
from typing import Any

from ..extract.prompt import estimate_messages
from ..modellock import FOLLOWUP, MODEL_LOCK, acquire_within, chat_label, held, release
from ..timeutil import utcnow
from ..util import is_bot_admin
from ..watch import origin_ids
from ..weeks import current_week_start
from . import followup, gate, persona, tools
from .ratelimit import RateLimiter

log = logging.getLogger(__name__)

#: How many times the model may call tools before it has to answer in words.
#: Four covers "look up the run, then propose the move"; a model still asking on
#: the fifth is looping, and the loop is the failure mode that costs a minute of
#: GPU per message.
MAX_TOOL_ROUNDS = 4

#: Exchanges kept per channel. Six is enough for "and the one after that?" to
#: mean something and short enough that the prompt never grows without bound.
HISTORY_EXCHANGES = 6

#: How far back a reply chain is followed. A member replying to a reply is
#: continuing a thought; four hops in, they are quoting history at the bot.
REPLY_CHAIN_DEPTH = 4

#: The ceiling on the *conversation*, in tokens, measured with the extractor's
#: accounting. The system prompt sits outside it (a persona document is a few
#: thousand tokens by itself and is not trimmable), as do the tool results,
#: which are appended after this is checked. Persona + this + a few tool results
#: is comfortably inside ``OLLAMA_NUM_CTX``.
CONVERSATION_BUDGET_TOKENS = 2500

#: How many replied-to messages are remembered before the oldest is forgotten.
#: A reply chain is short-lived; this only exists to stop a busy channel fetching
#: the same parent message once per reply.
REFERENCE_CACHE = 256

#: Said in the channel when the model could not answer. Deliberately plain: an
#: in-persona apology written here would be a second, worse persona.
FAILURE_REPLY = "Sorry — I couldn't get to the schedule just now. Try me again in a bit."

__all__ = [
    "MAX_TOOL_ROUNDS",
    "SPOOFED_NOTE",
    "ChatPilot",
    "ChatTurn",
    "Generation",
    "Handling",
    "defuse_notes",
    "tool_trace",
    "unglue_first_bullet",
]


@dataclass
class ChatTurn:
    """One remembered line of a channel's conversation with the bot."""

    role: str
    content: str
    #: The Discord message id, so a reply chain and the history cannot both
    #: contribute the same line.
    message_id: str | None = None


@dataclass
class Handling:
    """What the pilot did with one message.

    ``handled`` means "this message was addressed to the bot and the bot dealt
    with it" -- answered, or knowingly declined with a ⏳. It is what stops the
    extractor also reading the message as ambient party chat and proposing a
    schedule change from it, which is what happened live when the pilot channel
    turned out to sit under a watched category.

    It is deliberately *not* "the bot said something": a rate-limited question
    is still the pilot's business, and a message the gate refused for any other
    reason was never the pilot's at all.
    """

    handled: bool
    #: The gate's own words, for the log. Never shown in Discord.
    reason: str
    #: The answer, when there was one.
    answered: Generation | None = None

    def __bool__(self) -> bool:  # pragma: no cover - clarity at call sites
        return self.handled


@dataclass
class Generation:
    """What one answer cost and did -- returned for the logs and the tests."""

    reply: str = ""
    rounds: int = 0
    tool_calls: list[str] = field(default_factory=list)
    #: One per tool call, with its arguments, duration and outcome.
    outcomes: list[tools.ToolOutcome] = field(default_factory=list)
    created: list[str] = field(default_factory=list)
    error: str | None = None
    latency_ms: int = 0
    #: The two named parts of ``latency_ms``: time waiting on the model, summed
    #: over rounds, and time inside the tools. They do not add up to it -- the
    #: assembly either side is neither -- and that is the point of naming them.
    model_ms: int = 0
    tools_ms: int = 0
    #: Summed across rounds, and ``None`` when the model reported no usage at
    #: all. Zero and "did not say" are different facts about a bill.
    prompt_tokens: int | None = None
    completion_tokens: int | None = None

    @property
    def trace(self) -> str:
        """``get_schedule:ok, propose_move:refused`` -- the interaction in one field."""
        return ", ".join(f"{o.name or '?'}:{o.outcome}" for o in self.outcomes)

    @property
    def outcome(self) -> str:
        """``answered`` or ``failed`` -- what the member actually got.

        A generation that errored *and* a generation that came back empty are
        both failures: the member saw :data:`FAILURE_REPLY` either way.
        """
        return "failed" if self.error or not self.reply else "answered"

    def add_usage(self, prompt: int | None, completion: int | None) -> None:
        """Fold one round's token counts in, leaving ``None`` if none ever came."""
        if prompt is not None:
            self.prompt_tokens = (self.prompt_tokens or 0) + prompt
        if completion is not None:
            self.completion_tokens = (self.completion_tokens or 0) + completion


def _client(settings: Any, host: str | None = None):
    """Build an ``ollama.AsyncClient``.  Imported lazily so tests need no model."""
    from ollama import AsyncClient

    return AsyncClient(host=host or settings.ollama_host, timeout=settings.chat_pilot_timeout)


def _message_text(response: Any) -> tuple[str, list]:
    """``(content, tool_calls)`` from a ChatResponse, tolerating a plain dict."""
    message = getattr(response, "message", None)
    if message is None and isinstance(response, dict):
        message = response.get("message")
    if message is None:
        return "", []
    if isinstance(message, dict):
        return (message.get("content") or "").strip(), list(message.get("tool_calls") or [])
    content = getattr(message, "content", None)
    calls = getattr(message, "tool_calls", None)
    return (content or "").strip(), list(calls or [])


def _usage(response: Any) -> tuple[int | None, int | None]:
    """``(prompt_tokens, completion_tokens)`` from a chat response.

    Ollama reports these as ``prompt_eval_count`` and ``eval_count`` on the
    response itself. Every part of this is optional: a scripted stand-in has
    neither, an older server may send one, and a future one may rename them --
    so an absent or unparseable count is ``None`` ("nobody said") rather than
    zero ("it was free").
    """

    def count(key: str) -> int | None:
        value = response.get(key) if isinstance(response, dict) else getattr(response, key, None)
        try:
            return int(value) if value is not None else None
        except (TypeError, ValueError):  # pragma: no cover - defensive
            return None

    return count("prompt_eval_count"), count("eval_count")


def _brief(arguments: dict, limit: int = 200) -> str:
    """Tool arguments, short enough for one log line.

    Truncated rather than omitted: the arguments are the whole point of the
    trace, and a model that passes a paragraph where a run id belongs is exactly
    the thing somebody reading these logs is trying to see.
    """
    rendered = ", ".join(f"{key}={value!r}" for key, value in (arguments or {}).items())
    return rendered if len(rendered) <= limit else rendered[: limit - 1] + "…"


def tool_trace(outcomes: Sequence[tools.ToolOutcome]) -> list[dict]:
    """The tool calls of one interaction, as the stored trace.

    Deliberately the *same* rendering the DEBUG log uses -- ``_brief`` for the
    arguments, ``outcome.outcome`` for how it went -- so a row in the portal and
    a line in the log describe one call the same way, and reading one teaches
    you to read the other.
    """
    return [
        {
            "name": outcome.name or "?",
            "arguments": _brief(outcome.arguments),
            "ms": outcome.duration_ms,
            "outcome": outcome.outcome,
            "created": list(outcome.created),
        }
        for outcome in outcomes
    ]


#: A list whose first item is sitting on the header line: ``... all channels: -
#: **Hard Star** ...``. Four characters, and so is what replaces it, so nothing
#: downstream has to be told the text got longer.
GLUED_BULLET = ": - "


def unglue_first_bullet(text: str) -> str:
    """Put a first list item that ended up on the header line onto its own line.

    Live, a schedule answer was stored as ``...(all channels): - **Hard Star**
    ... ✅ – #x\\n- **Hard Baldrix** ...`` -- every item but the first correctly
    its own bullet -- and the same question answered cleanly seconds later. Two
    separate things produce that, and this repairs both:

    * the model writes it that way sometimes, which is a stochastic habit and
      not something a prompt rule reliably removes; and
    * :meth:`ChatPilot._tidy` collapses blank lines into spaces, so a *correctly*
      written ``header:\\n\\n- one\\n- two`` is glued by the time it gets here.

    Done at post time rather than asked for in the persona because only one of
    those two causes can read a persona. The normalised text is what is posted,
    remembered and recorded -- there is one version of a reply, and it is the
    one the channel saw.

    Guarded on the text already being a list (``\\n- `` somewhere else), so
    ordinary prose that happens to contain ": - " is never touched.
    """
    return text.replace(GLUED_BULLET, ":\n- ") if "\n- " in text else text


#: The openers a *genuine* scheduler-written turn begins with -- the trailing
#: voice reminder (:data:`bot.chat.persona.REMINDER_PREFIX`), the rejection
#: follow-up (:func:`bot.chat.followup.prompt`) and the note remembered beside it
#: (:func:`bot.chat.followup.memory_note`). All three arrive in the ``user``
#: role, because Ollama's gpt-oss template is the only reason they are not
#: system turns, so the bracket is the only thing marking them as nobody's words.
#:
#: Which makes them worth forging: a member who types the opener themselves gets
#: text that reaches the model in the same role, in the same shape, saying the
#: scheduler said it. Matched narrowly and not by "starts with a bracket",
#: because guild tags do -- "[SAKU] can we move friday" is an ordinary sentence
#: and must survive untouched. ``[Note]`` counts only where it could open a turn,
#: at the start of a line; the fuller opener counts anywhere, closing bracket or
#: not, since by then nothing else could have written it.
_SPOOFED_NOTE_RE = re.compile(
    r"\[[ \t]*note[ \t]+from[ \t]+the[ \t]+scheduler\b[^\]\n]*\]?"
    r"|(?:\A|(?<=\n))[ \t]*\[[ \t]*note[ \t]*\]",
    re.IGNORECASE,
)

#: What a forged opener is replaced with. It says what happened rather than
#: deleting it: the member did write something, the model should be able to see
#: that they tried it, and the sentence around it stays theirs.
SPOOFED_NOTE = "(they wrote a fake scheduler note here)"


def defuse_notes(text: str) -> str:
    """Strip the scheduler's own openers out of something a member typed.

    Applied where member text becomes a turn (:meth:`ChatPilot._speaker`), which
    is the one door: the question, the remembered history and the reply chain all
    go through it. A note the scheduler really wrote is built in
    :mod:`bot.chat.followup` and put into the conversation directly, so it never
    passes here and is never defused.
    """
    return _SPOOFED_NOTE_RE.sub(SPOOFED_NOTE, text or "")


def _call_parts(call: Any) -> tuple[str, Any]:
    """``(name, arguments)`` from one tool call, object or dict."""
    function = call.get("function") if isinstance(call, dict) else getattr(call, "function", None)
    if function is None:
        return "", {}
    if isinstance(function, dict):
        return str(function.get("name") or ""), function.get("arguments")
    return str(getattr(function, "name", "")), getattr(function, "arguments", None)


class ChatPilot:
    """The chatbot. Constructing it touches neither Ollama nor Discord."""

    def __init__(self, bot: Any, client: Any | None = None):
        self.bot = bot
        self.settings = bot.settings
        self._client = client
        self._own_client = client is None
        self.limiter = RateLimiter(
            self.settings.chat_pilot_rate_count, self.settings.chat_pilot_rate_window_s
        )
        #: The same limiter over one shared key (:data:`bot.chat.gate.GLOBAL_KEY`):
        #: everybody's answers come out of one pool, so a guild that hands the
        #: pilot role around cannot spend more of the host than it has to give.
        self.global_limiter = RateLimiter(
            self.settings.chat_pilot_global_rate_count,
            self.settings.chat_pilot_global_rate_window_s,
        )
        #: Per channel, so two channels can be answered at once but one channel
        #: cannot queue up a backlog of 60-second generations.
        self._busy: set[str] = set()
        #: Per channel, when the last rejection follow-up was *started*. The
        #: cheap half of the anti-spam rule: `_busy` stops two at once, this
        #: stops a run of them across different cards. See
        #: :data:`bot.chat.followup.COOLDOWN_S`.
        self._followed_up_at: dict[str, float] = {}
        self._history: dict[str, deque[ChatTurn]] = {}
        #: Referenced message id -> its author id (or None). One API call per
        #: replied-to message, however many people reply to it.
        self._replied: dict[str, str | None] = {}
        self._persona: str | None = None

    # -- wiring ------------------------------------------------------------
    def client(self) -> Any:
        if self._client is None:
            self._client = _client(self.settings)
        return self._client

    async def close(self) -> None:
        if self._client is not None and self._own_client:
            close = getattr(self._client, "close", None)
            if close is not None:
                await close()
            self._client = None

    @property
    def enabled(self) -> bool:
        """The runtime kill switch, seeded from whether the feature is configured."""
        return bool(getattr(self.bot, "chat_mode", False))

    def persona_text(self) -> str:
        """The persona document, read once per process.

        Cached because it is read on every message and never changes without a
        restart; :meth:`reload_persona` exists for the deploy that edits it.
        """
        if self._persona is None:
            self._persona = persona.load_persona(self.settings.persona_path)
        return self._persona

    def reload_persona(self) -> str:
        self._persona = None
        return self.persona_text()

    def answering(self) -> list[str]:
        """The channels with an answer in flight, for the portal's Limits page.

        A copy, sorted: the page must not be able to hold a reference to the set
        the answer path is adding to and removing from.
        """
        return sorted(self._busy)

    # -- intake ------------------------------------------------------------
    async def offer(self, message: Any) -> Handling:
        """Called by ``on_message`` for every guild message. Answers, or does not.

        Returns a :class:`Handling` rather than an answer-or-``None`` because
        the caller needs to know something the answer cannot express: whether
        this message was *the pilot's*. A message that got a ⏳ was handled --
        the person was talking to the bot and the bot declined to answer right
        now -- and the extractor must stay out of it either way.

        The gate is evaluated exactly **once**, here, and the verdict is carried
        out in this method. It must not be re-run to answer "was that ours?":
        :meth:`bot.chat.ratelimit.RateLimiter.allow` records an allowance every
        time it is consulted, so a second call would silently halve everybody's
        quota.
        """
        bot_user_id = getattr(getattr(self.bot, "user", None), "id", None)
        # Only worth resolving a reply once the cheap checks have passed: a
        # watched party channel that is not a chat channel produces hundreds of
        # messages a day and none of them deserves an API call.
        replied_author_id = (
            await self.replied_author_id(message)
            if gate.would_check_mention(
                message, self.settings, bot_user_id=bot_user_id, enabled=self.enabled
            )
            else None
        )
        # Worked out once and used twice: the gate's role stand-in and rate-limit
        # exemption, and the tools' authority check (:func:`bot.chat.tools`
        # ``_require_authority``). Evaluating it twice would risk the two
        # disagreeing about who is staff for one message.
        is_admin = self._is_admin(getattr(message, "author", None))
        decision = gate.decide(
            message,
            self.settings,
            bot_user_id=bot_user_id,
            enabled=self.enabled,
            is_admin=is_admin,
            limiter=self.limiter,
            global_limiter=self.global_limiter,
            self_role_id=self._self_role_id(message),
            replied_author_id=replied_author_id,
        )
        if not decision.act:
            if decision.busy:
                log.info("chat: %s from %s", decision.reason, getattr(message.author, "id", "?"))
                await self._react(message, gate.RATE_LIMITED_REACTION)
                # Rate limited, but addressed to the bot: ours, and not the
                # extractor's to read as ambient chat.
                return Handling(True, decision.reason)
            log.debug("chat: ignoring a message (%s)", decision.reason)
            return Handling(False, decision.reason)

        channel_id = str(origin_ids(message.channel)[0])
        if channel_id in self._busy:
            # A second question while the first is still being answered. Queuing
            # it would mean a 60-second-old reply arriving after the asker has
            # given up, so it is dropped and they are told to wait. Its own emoji,
            # not the rate limiter's: this one clears in seconds.
            log.info("chat: channel %s is already answering; dropping", channel_id)
            await self._react(message, gate.CHANNEL_BUSY_REACTION)
            return Handling(True, "already answering")

        self._busy.add(channel_id)
        # An answer is 10-30 s of GPU, which is long enough to read as being
        # ignored. The 👀 goes on before any of that starts and comes off when
        # the reply lands, so the reaction is only ever "still working".
        await self._react(message, gate.SEEN_REACTION)
        # The channel lock above says nobody else in *this* channel is being
        # answered; this one says nothing else on the host is using the model
        # at all -- another channel's answer, an extraction, a rescan. Staff
        # wait a whole answer's worth, because `asyncio.Lock` wakes its waiters
        # in order and being next in that queue is the only priority the feature
        # has. Everybody else waits a couple of seconds and is then turned away:
        # a reply that lands two minutes after the question is worse than a
        # visible "not now".
        wait_s = (
            self.settings.chat_pilot_timeout if is_admin else self.settings.chat_pilot_lock_wait_s
        )
        if not await acquire_within(wait_s, chat_label(channel_id)):
            # This costs the asker a rate-limit slot, because the gate spent one
            # on the way in. The alternative -- gating after the model instead of
            # before it -- would leave a queue of already-admitted questions
            # piled up on the lock, which is the pile-up the wait exists to
            # prevent. Its own emoji again: this one clears in seconds.
            log.info(
                "chat: the model was busy for %.1fs; shedding %s in channel %s",
                wait_s,
                getattr(message.author, "id", "?"),
                channel_id,
            )
            self._busy.discard(channel_id)
            await self._unreact(message, gate.SEEN_REACTION)
            await self._react(message, gate.CHANNEL_BUSY_REACTION)
            return Handling(True, "the model is busy")
        try:
            # Held across the *whole* answer -- every tool round and the Discord
            # send -- rather than round each model call. Releasing between rounds
            # would let an extraction take the model in the middle of a
            # conversation and stretch one answer past its own timeout.
            return Handling(True, "ok", await self._answer(message, channel_id, is_admin))
        finally:
            release()
            self._busy.discard(channel_id)
            await self._unreact(message, gate.SEEN_REACTION)

    def _self_role_id(self, message: Any) -> int | None:
        """The bot's own managed integration role, if the guild has one.

        ``guild.self_role`` is the role Discord creates for this bot and that
        nobody else can hold. Read from the live guild rather than configured,
        because it is not a choice anybody makes -- and hardcoding an id would
        break the moment the bot is added to another guild.
        """
        guild = getattr(message, "guild", None) or getattr(self.bot, "get_guild", lambda _i: None)(
            self.settings.guild_id
        )
        role = getattr(guild, "self_role", None)
        role_id = getattr(role, "id", None)
        try:
            return int(role_id) if role_id is not None else None
        except (TypeError, ValueError):  # pragma: no cover - defensive
            return None

    async def replied_author_id(self, message: Any) -> str | None:
        """Who wrote the message this one is replying to, fetching if need be.

        ``message.reference.resolved`` is filled from discord.py's in-memory
        message cache, which a restart empties. Live, the bot restarted at 09:22
        and a 09:24 reply to a message from before it was dropped as "the bot was
        not mentioned" -- with the reply ping off there was nothing else to go
        on. Every restart makes that likely, so an unresolved reference is
        fetched once from the API.

        Best-effort throughout: a deleted, hidden or unreachable parent is simply
        "not the bot". Nothing here may raise into the answer path.
        """
        reference = getattr(message, "reference", None)
        if reference is None:
            return None
        resolved = getattr(reference, "resolved", None)
        if resolved is not None:
            # A `DeletedReferencedMessage` resolves but has no author: the reply
            # is real and its parent is gone, which is not a mention of anybody.
            author = getattr(resolved, "author", None)
            return str(author.id) if author is not None else None

        parent_id = getattr(reference, "message_id", None)
        if parent_id is None:
            return None
        key = str(parent_id)
        if key in self._replied:
            return self._replied[key]

        author_id: str | None = None
        fetch = getattr(getattr(message, "channel", None), "fetch_message", None)
        if fetch is not None:
            try:
                parent = await fetch(int(parent_id))
            except Exception:  # noqa: BLE001 - NotFound/Forbidden/HTTP/anything
                log.debug("chat: could not fetch replied-to message %s", key, exc_info=True)
            else:
                author = getattr(parent, "author", None)
                author_id = str(author.id) if author is not None else None
        self._remember_reference(key, author_id)
        return author_id

    def _remember_reference(self, key: str, author_id: str | None) -> None:
        """One fetch per referenced message, and a bounded number remembered."""
        if len(self._replied) >= REFERENCE_CACHE:
            self._replied.pop(next(iter(self._replied)), None)
        self._replied[key] = author_id

    def _is_admin(self, user: Any) -> bool:
        """The existing "who runs this bot" rule (:func:`bot.util.is_bot_admin`).

        Two callers, one rule: the gate uses it as the chat role's stand-in and
        as the rate-limit exemption, and it rides along on the
        :class:`bot.chat.tools.ToolContext` as the exemption from the write
        tools' authority check. Read from the live member object, so a member
        who says they are an admin is saying a sentence.
        """
        if user is None:
            return False
        guild = getattr(self.bot, "get_guild", lambda _id: None)(self.settings.guild_id)
        permissions = getattr(user, "guild_permissions", None)
        return is_bot_admin(
            bool(getattr(permissions, "administrator", False)),
            guild is not None and getattr(guild, "owner_id", None) == getattr(user, "id", None),
            [getattr(role, "id", role) for role in getattr(user, "roles", None) or ()],
            self.settings.admin_role_id,
        )

    async def _answer(self, message: Any, channel_id: str, is_admin: bool = False) -> Generation:
        author_id = str(message.author.id)
        text = (message.content or "").strip()
        context = tools.ToolContext(
            bot=self.bot,
            author_id=author_id,
            channel_id=channel_id,
            message_id=str(message.id),
            is_admin=is_admin,
        )
        conversation = self.build_conversation(message, channel_id)
        result = await self.generate(conversation, context)

        reply = result.reply or FAILURE_REPLY
        await self._post(message, reply)
        # Remembered only once it has actually been said, so a failed generation
        # does not leave the bot talking about an answer nobody saw.
        self.remember(channel_id, ChatTurn("user", self._speaker(author_id, text), str(message.id)))
        self.remember(channel_id, ChatTurn("assistant", reply))
        # One line per completed interaction. Everything needed to decide whether
        # a turn is worth reading the DEBUG trace for: how many rounds it took,
        # how long it took, what it called and how each call went. Turn on
        # `LOG_LEVEL=DEBUG` for the arguments behind each of those outcomes.
        log.info(
            "chat: answered %s in channel %s in %d ms (%d round(s), %d tool call(s)%s)%s%s",
            author_id,
            channel_id,
            result.latency_ms,
            result.rounds,
            len(result.tool_calls),
            f": {result.trace}" if result.outcomes else "",
            f" -> proposal {', '.join(result.created)}" if result.created else "",
            f" [{result.error}]" if result.error else "",
        )
        self._record(getattr(message, "id", None), channel_id, author_id, text, reply, result)
        return result

    def _record(
        self,
        message_id: Any,
        channel_id: str,
        author_id: str,
        question: str,
        reply: str,
        result: Generation,
    ) -> None:
        """Write the interaction to the database. Never raises.

        The same rule the whole module runs on, and the same one
        :attr:`bot.db.Repo.on_run_changed` follows: the member's answer has
        already been posted by the time this runs, and a bookkeeping row that
        cannot be written is a line in the log, not a failed conversation.

        Only *handled generations* land here -- ⏳ and 💬 never reach
        :meth:`_answer` -- so every row is a question that actually cost model
        time. A rejection follow-up (:meth:`on_rejection`) is one of them: the
        bot asked rather than answered, but it cost a generation like any other
        and the trace is worth the same.
        """
        try:
            self.bot.repo.log_chat_interaction(
                model=self.settings.chat_pilot_model,
                question=question,
                reply=reply,
                outcome=result.outcome,
                error=result.error,
                rounds=result.rounds,
                channel_id=channel_id,
                message_id=message_id,
                author_id=author_id,
                latency_ms=result.latency_ms,
                model_ms=result.model_ms,
                tools_ms=result.tools_ms,
                prompt_tokens=result.prompt_tokens,
                completion_tokens=result.completion_tokens,
                tool_calls=tool_trace(result.outcomes),
            )
        except Exception:  # noqa: BLE001 - analytics must never cost an answer
            log.exception("chat: could not record the interaction")

    # -- rejections --------------------------------------------------------
    async def on_rejection(
        self,
        amendments: Sequence[dict],
        *,
        reactor_id: int | str,
        card_message_id: int | str | None = None,
    ) -> Handling:
        """A ❌ landed on a card the pilot posted: ask what it should be instead.

        Called from :meth:`bot.client.BossBot._handle_proposal_reaction` after
        the amendments have been rejected, and does nothing at all unless
        :func:`bot.chat.followup.scope` says this rejection is the pilot's --
        see there for the gates, and for why the question is built from the row
        rather than from anything anybody wrote.

        Everything from the guards down to marking the channel busy is
        deliberately **synchronous**. This runs on the event loop, so a stretch
        with no ``await`` in it cannot be interleaved: two ❌ arriving together
        cannot both find the channel free, and cannot both claim the card. The
        claim (:meth:`bot.db.Repo.claim_chat_followup`) is a single atomic
        statement as well, so the promise survives a future edit that puts an
        ``await`` in the middle of this.

        That stretch is also what lets the model be checked with a plain
        ``MODEL_LOCK.locked()`` and taken further down: nothing can have claimed
        it in between. The check sits *before* the claim on purpose -- a claim is
        permanent, and burning a card's one follow-up on a generation that then
        never runs would silence it for good. A busy model simply means no
        question: this one is the bot's own idea, and unlike an answer nobody is
        waiting for it.

        The answer is not built here. It arrives as an ordinary reply to the
        bot's message, which :meth:`offer` already treats as a mention -- so
        what this has to leave behind is the *context* for it, which is why the
        question and the card's facts are remembered before it returns.
        """
        rows = [a for a in amendments if a.get("id")]
        if not rows:
            return Handling(False, "nothing was rejected")
        channel = self.bot.resolve_channel(rows[0].get("channel_id"))
        allowed = followup.scope(
            self.bot,
            rows[0],
            reactor_id=reactor_id,
            channel=channel,
            enabled=self.enabled,
        )
        if not allowed.act:
            log.debug("chat: no follow-up on a rejected card (%s)", allowed.reason)
            return Handling(False, allowed.reason)

        channel_id = str(origin_ids(channel)[0])
        now = time.monotonic()
        last = self._followed_up_at.get(channel_id)
        if last is not None and now - last < followup.COOLDOWN_S:
            # Somebody clearing out several cards at once. The cards are all
            # visibly rejected already; a question per card would be the bot
            # talking over a tidy-up.
            log.info("chat: channel %s had a follow-up %.0fs ago; dropping", channel_id, now - last)
            return Handling(False, "a follow-up was asked here too recently")
        if channel_id in self._busy:
            # The channel is mid-answer. Queuing this would land a question
            # about a dead card after the answer to a live one.
            log.info("chat: channel %s is already answering; dropping the follow-up", channel_id)
            return Handling(False, "already answering")
        if MODEL_LOCK.locked():
            # Somebody else on the host has the model -- another channel, an
            # extraction, a rescan. A question nobody asked for is the first
            # thing to drop: it is worth a generation only while the ❌ is still
            # fresh, and nobody is waiting on it. Read rather than acquired
            # because this stretch may not await; see the docstring.
            log.debug("chat: the model is busy; no follow-up on the rejected card")
            return Handling(False, "the model is busy")

        # Every amendment on the card, so a card carrying two of them cannot be
        # followed up once per amendment. Anything already claimed means this
        # card has had its question, and the rest are burnt on the way past.
        mine = [a for a in rows if self.bot.repo.chat_interaction_for_amendment(a["id"])]
        claimed = [a for a in mine if self.bot.repo.claim_chat_followup(a["id"])]
        if len(claimed) != len(mine):
            log.info("chat: card %s has already been followed up", card_message_id)
            return Handling(False, "already followed up")

        self._busy.add(channel_id)
        self._followed_up_at[channel_id] = now
        try:
            # Free by construction: the ``locked()`` check above said so and
            # nothing since it has awaited, so nobody can have taken it in the
            # meantime. Held for the whole generation all the same, so an
            # extraction cannot start underneath the question.
            async with held(FOLLOWUP):
                result = await self._ask_instead(
                    claimed, allowed.author_id, channel, card_message_id
                )
        finally:
            self._busy.discard(channel_id)
        return Handling(True, "ok", result)

    async def _ask_instead(
        self,
        amendments: list[dict],
        author_id: str,
        channel: Any,
        card_message_id: int | str | None,
    ) -> Generation:
        """Generate the question, post it, and leave the context for the answer.

        ``read_only`` is the load-bearing argument: the read tools still run, so
        the bot can look the run up before asking, and no write tool exists for
        this turn however the model asks for one.

        A generation that failed says **nothing**. :data:`FAILURE_REPLY` is the
        right answer to somebody who asked a question and is waiting; posted
        unprompted into a channel because a reaction happened, it is the bot
        apologising for a conversation nobody started.
        """
        channel_id = str(origin_ids(channel)[0])
        context = tools.ToolContext(
            bot=self.bot,
            author_id=author_id,
            channel_id=channel_id,
            message_id=str(card_message_id or ""),
            read_only=True,
        )
        question = followup.prompt(self.bot, amendments, author_id)
        # A ``user`` turn, though nobody said it: gpt-oss's template lifts every
        # system message out of the conversation and into the instructions header
        # at the top, which would put this synthetic turn *before* the history it
        # is a reaction to. Its bracketed opener carries the provenance instead.
        conversation = self.assemble([*self.history(channel_id), ChatTurn("user", question)])
        result = await self.generate(conversation, context)
        log.info(
            "chat: followed up on a rejected card in channel %s in %d ms (%d round(s)%s)%s",
            channel_id,
            result.latency_ms,
            result.rounds,
            f": {result.trace}" if result.outcomes else "",
            f" [{result.error}]" if result.error else "",
        )
        if result.reply:
            posted = await self._post_followup(channel, result.reply, author_id, card_message_id)
            # Remembered only once it has been said, exactly as an answer is.
            # The note goes in as well: without it the member's "make it friday"
            # arrives with no idea what "it" was. Remembered as ``user`` for the
            # same reason the question above is: a system turn would be hoisted
            # out of the history and stop sitting where it happened.
            note = followup.memory_note(self.bot, amendments)
            posted_id = str(getattr(posted, "id", "") or "") or None
            self.remember(channel_id, ChatTurn("user", note))
            self.remember(channel_id, ChatTurn("assistant", result.reply, posted_id))
        self._record(card_message_id, channel_id, author_id, question, result.reply, result)
        return result

    async def _post_followup(
        self,
        channel: Any,
        content: str,
        author_id: str,
        card_message_id: int | str | None,
    ) -> Any:
        """Post the question as a reply to the card itself. Never raises.

        Through ``post_plain`` like every other chat reply, so the allow-list is
        the asker and nobody else, ``@everyone`` is impossible, and quiet mode is
        applied -- though quiet mode has already stopped this in
        :func:`bot.chat.followup.scope`.
        """
        try:
            return await self.bot.post_plain(
                channel,
                content,
                [str(author_id)],
                reference_id=int(card_message_id) if card_message_id else None,
            )
        except Exception:  # noqa: BLE001 - a failed question is not worth a crash
            log.exception("chat: could not post the rejection follow-up")
            return None

    # -- context assembly --------------------------------------------------
    def _speaker(self, user_id: str, text: str) -> str:
        """A line as the model sees it: who said it, then what they said.

        The name is looked up from the roster rather than taken from the
        message, so a display name the model is shown is one the bot already
        knows -- and a member cannot rename themselves into a instruction.

        The words are defused (:func:`defuse_notes`) for the same reason the
        name is looked up: a member can type the scheduler's own bracketed
        opener, and being attributed to them by name is not by itself enough to
        stop a small model reading it as a note from the machinery.
        """
        from ..api import service

        return f"{service.member_name(self.bot, user_id)}: {defuse_notes(text)}"

    def history(self, channel_id: str) -> deque[ChatTurn]:
        return self._history.setdefault(str(channel_id), deque(maxlen=HISTORY_EXCHANGES * 2))

    def remember(self, channel_id: str, turn: ChatTurn) -> None:
        self.history(channel_id).append(turn)

    def forget(self, channel_id: str | None = None) -> None:
        if channel_id is None:
            self._history.clear()
        else:
            self._history.pop(str(channel_id), None)

    def reply_chain(self, message: Any) -> list[ChatTurn]:
        """The messages ``message`` is replying to, oldest first.

        Only what Discord already resolved is followed -- no fetching. A chain
        that is not in the cache is not worth an API round trip per message, and
        the channel history usually holds the same conversation anyway.
        """
        chain: list[ChatTurn] = []
        node = message
        bot_id = str(getattr(getattr(self.bot, "user", None), "id", ""))
        for _ in range(REPLY_CHAIN_DEPTH):
            reference = getattr(node, "reference", None)
            parent = getattr(reference, "resolved", None) if reference is not None else None
            if parent is None or getattr(parent, "content", None) is None:
                break
            author = getattr(parent, "author", None)
            author_id = str(getattr(author, "id", ""))
            content = (parent.content or "").strip()
            if content:
                chain.append(
                    ChatTurn(
                        "assistant" if author_id == bot_id else "user",
                        content if author_id == bot_id else self._speaker(author_id, content),
                        str(getattr(parent, "id", "")) or None,
                    )
                )
            node = parent
        chain.reverse()
        return chain

    def build_conversation(self, message: Any, channel_id: str) -> list[dict[str, str]]:
        """System prompt, remembered exchanges, the reply chain, then the question.

        The schedule is deliberately *not* pre-injected: it is ten runs and a
        roster, it is stale the moment somebody reacts, and the model has tools
        for it. Trimming, when the budget needs it, drops the oldest remembered
        exchanges -- never the system prompt and never the question.
        """
        seen = {turn.message_id for turn in self.history(channel_id) if turn.message_id}
        earlier = list(self.history(channel_id))
        earlier += [turn for turn in self.reply_chain(message) if turn.message_id not in seen]
        question = ChatTurn(
            "user", self._speaker(str(message.author.id), (message.content or "").strip())
        )
        return self.assemble([*earlier, question])

    def assemble(self, turns: Sequence[ChatTurn]) -> list[dict[str, str]]:
        """The system prompt, then as much of ``turns`` as the budget allows.

        Shared by :meth:`build_conversation` and by the rejection follow-up
        (:meth:`on_rejection`), which assembles the same prompt around a
        synthetic last turn rather than a member's question -- so a follow-up is
        composed under the persona, the clock and the budget an answer is, and
        there is one place where "what the model sees" is decided.
        """
        now = utcnow()
        week = current_week_start(
            self.bot.tz, self.settings.reset_weekday, self.settings.reset_time, now
        )
        system = persona.system_prompt(
            self.persona_text(),
            persona.clock_header(now, self.bot.tz, week),
            persona.runtime_line(self.settings.chat_pilot_model),
        )
        rendered = [{"role": t.role, "content": t.content} for t in turns]
        # Measured over the conversation alone. The system prompt is a whole
        # persona document -- a few thousand tokens on its own -- so budgeting
        # the two together would leave a long conversation trimmed to nothing
        # and still over the line. What grows without bound is the deque, and
        # the deque is what this trims.
        while len(rendered) > 1 and estimate_messages(rendered) > CONVERSATION_BUDGET_TOKENS:
            rendered.pop(0)
        return [{"role": "system", "content": system}, *rendered]

    # -- the model ---------------------------------------------------------
    async def generate(
        self, conversation: list[dict[str, str]], context: tools.ToolContext
    ) -> Generation:
        """Run the tool loop until the model answers in words. Never raises."""
        started = time.monotonic()
        result = Generation()
        try:
            await asyncio.wait_for(
                self._loop(conversation, context, result),
                timeout=self.settings.chat_pilot_timeout,
            )
        except TimeoutError:
            result.error = f"no answer within {self.settings.chat_pilot_timeout:.0f}s"
            log.warning("chat: the model did not answer in time")
        except Exception as exc:  # noqa: BLE001 - chat must never break the bot
            result.error = f"{type(exc).__name__}: {exc}"
            log.exception("chat: the model call failed")
        result.created = list(context.created)
        result.latency_ms = int((time.monotonic() - started) * 1000)
        return result

    async def _loop(
        self,
        conversation: list[dict[str, str]],
        context: tools.ToolContext,
        result: Generation,
    ) -> None:
        messages: list[dict[str, Any]] = list(conversation)
        for round_number in range(1, MAX_TOOL_ROUNDS + 1):
            result.rounds = round_number
            # The last round is answered without tools at all, so a model that
            # would call one again physically cannot: the alternative is a reply
            # that never arrives because it kept looking things up.
            last = round_number == MAX_TOOL_ROUNDS
            asked_at = time.monotonic()
            response = await self._chat(messages, with_tools=not last, context=context)
            content, calls = _message_text(response)
            model_ms = int((time.monotonic() - asked_at) * 1000)
            result.model_ms += model_ms
            result.add_usage(*_usage(response))
            log.debug(
                "chat: round %d/%d model answered in %d ms (%s%s)",
                round_number,
                MAX_TOOL_ROUNDS,
                model_ms,
                f"{len(calls)} tool call(s)" if calls else "in words",
                "" if not last else ", tools withheld on the last round",
            )
            if not calls:
                result.reply = self._tidy(content)
                return
            messages.append({"role": "assistant", "content": content, "tool_calls": calls})
            for call in calls:
                name, arguments = _call_parts(call)
                result.tool_calls.append(name)
                outcome = await tools.run(context, name, arguments)
                result.outcomes.append(outcome)
                result.tools_ms += outcome.duration_ms
                # The line that answers "why did it propose that": the arguments
                # the model actually passed, and whether the tool obeyed.
                log.debug(
                    "chat: round %d/%d tool %s(%s) -> %s in %d ms%s",
                    round_number,
                    MAX_TOOL_ROUNDS,
                    name or "?",
                    _brief(outcome.arguments),
                    outcome.outcome,
                    outcome.duration_ms,
                    f" card {', '.join(outcome.created)}" if outcome.created else "",
                )
                messages.append({"role": "tool", "name": name, "content": outcome.output})
        # Four rounds of tools and still nothing said.
        result.error = "the model kept calling tools"
        log.warning("chat: gave up after %d tool rounds", MAX_TOOL_ROUNDS)

    def voice_reminder(self) -> dict[str, str]:
        """The one line the model reads immediately before it writes.

        Sent as the **last message of every call**, after the conversation and
        after any tool results. The system prompt's own copy is thousands of
        tokens back and, by composition time, behind a stack of run ids and card
        confirmations -- which is exactly where the recency it was written for
        goes. Card confirmations and error relays were the flattest replies live,
        and those are precisely the turns with the most tool output in front of
        them.

        Sent as **user**, not system, and that is not a stylistic choice.
        Ollama's gpt-oss template does not render system messages in the message
        flow at all: it skips every one of them and concatenates them into the
        developer "# Instructions" header at the very top. A trailing system
        message is therefore not trailing -- it is hoisted back up behind the
        whole persona document, which is precisely the buried copy this exists to
        escape. ``user`` is the only role the template renders in place at the
        end. That role is also why this is worded differently from the system
        prompt's own footer: arriving in the conversation it looks like a member
        typed it, so :data:`bot.chat.persona.REMINDER_PREFIX` opens by saying who
        it is from and that it is not to be answered.

        Appended rather than woven in, so it never accumulates in the remembered
        conversation, and re-derived from the cached persona each turn so a
        `reload_persona` takes effect immediately.
        """
        return {"role": "user", "content": persona.voice_reminder(self.persona_text())}

    async def _chat(
        self, messages: list[dict[str, Any]], with_tools: bool, context: tools.ToolContext
    ) -> Any:
        # A read-only turn is offered the read schemas only. That is the polite
        # half of the rule; `tools.run` refuses a write by name whatever the
        # model asks for, which is the half that actually holds.
        offered = tools.read_tools() if context.read_only else tools.TOOLS
        return await self.client().chat(
            model=self.settings.chat_pilot_model,
            messages=[*messages, self.voice_reminder()],
            # Temperature is set explicitly rather than left to the model's
            # Modelfile: the extractor pins 0 next door, and a reader who sees
            # that one is entitled to know this one is different on purpose.
            options={
                "num_ctx": self.settings.ollama_num_ctx,
                "temperature": self.settings.chat_pilot_temperature,
            },
            keep_alive=-1,
            think=self.settings.think,
            **({"tools": offered} if with_tools else {}),
        )

    @staticmethod
    def _tidy(content: str) -> str:
        """Trim the model's answer to something a channel can read.

        Discord's own limit is 2000 characters; this is far below it because a
        chatbot that writes an essay in a party channel is a worse chatbot, and
        the persona already asks for four sentences.

        The blank-line collapse is also what makes :func:`unglue_first_bullet`
        necessary, so the repair happens here, immediately after the damage.
        """
        text = " ".join((content or "").split("\n\n")).strip()
        return unglue_first_bullet(text)[:1200].strip()

    # -- discord -----------------------------------------------------------
    async def _post(self, message: Any, content: str) -> None:
        """Reply in the channel, notifying the asker and nobody else.

        Through :meth:`bot.client.BossBot.post_plain`, which is the bot's one
        plain-message path: it builds the explicit allow-list, applies quiet
        mode, and refuses ``@everyone`` from any caller. Nothing here constructs
        an ``AllowedMentions`` of its own, so the chatbot cannot become a second
        way to ping a guild.
        """
        try:
            await self.bot.post_plain(
                message.channel,
                content,
                [str(message.author.id)],
                reference_id=getattr(message, "id", None),
            )
        except Exception:  # noqa: BLE001 - a failed reply is not worth a crash
            log.exception("chat: could not post the reply")

    @staticmethod
    async def _react(message: Any, emoji: str) -> None:
        """Put one of the bot's reactions on a message. Never raises.

        A missing Add Reactions permission, or a message deleted mid-answer, is
        a slightly worse experience -- not a reason to lose the answer.
        """
        add = getattr(message, "add_reaction", None)
        if add is None:
            return
        with contextlib.suppress(Exception):
            await add(emoji)

    async def _unreact(self, message: Any, emoji: str) -> None:
        """Take the bot's own reaction back off. Never raises.

        Removing your *own* reaction needs no Manage Messages, so this works in
        channels where `_drop_opposite_reaction` cannot. Best-effort all the
        same: if it fails the 👀 simply stays, which is untidy rather than wrong.
        """
        remove = getattr(message, "remove_reaction", None)
        if remove is None:
            return
        with contextlib.suppress(Exception):
            await remove(emoji, getattr(self.bot, "user", None))
