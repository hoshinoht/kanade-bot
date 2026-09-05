"""Gate, assemble, generate, and post chatbot replies."""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import math
import re
import time
from collections import deque
from collections.abc import Sequence
from dataclasses import dataclass, field
from typing import Any

from bot.agent.util import is_bot_admin
from bot.domain.bosses import BossReference
from bot.domain.timeutil import utcnow
from bot.domain.weeks import current_week_start
from bot.infrastructure import events
from bot.infrastructure.modellock import (
    FOLLOWUP,
    MODEL_LOCK,
    acquire_within,
    chat_label,
    held,
    release,
)
from bot.infrastructure.watch import origin_ids

from .. import behaviour_plugins
from ..extract.prompt import estimate_messages, estimate_tokens, prompt_budget
from . import followup, gate, persona, strategy, tools
from .ratelimit import RateLimiter

log = logging.getLogger(__name__)

#: Hard cap prevents tool-call loops.
MAX_TOOL_ROUNDS = 4

#: Bounded exchanges retained per channel.
HISTORY_EXCHANGES = 6

#: Maximum resolved reply-chain depth.
REPLY_CHAIN_DEPTH = 4

#: Conversation token budget; system prompt and tool results are separate.
CONVERSATION_BUDGET_TOKENS = 2500

#: Bound the referenced-message cache.
REFERENCE_CACHE = 256

#: Bound re-anchorable exchanges.
ANCHOR_CACHE = 64

#: Fallback reply when generation fails.
FAILURE_REPLY = "Sorry — I couldn't complete that just now. Try me again in a bit."

#: Tokens held back so Ollama has room to complete the reply.
COMPLETION_RESERVE_TOKENS = 1024

#: A strategy answer is unsafe when its checked-in grounding is unavailable.
STRATEGY_GROUNDING_FAILURE_REPLY = (
    "I couldn't load the checked-in strategy notes just now, so I can't safely give mechanics "
    "advice."
)


class ContextBudgetError(RuntimeError):
    """The protected current turn cannot fit in the configured model context."""

#: Static rate-limit replies must not invoke the model.
RATE_LIMITED_REPLY = "That's your {count} answer{plural} for now — ask me again in about {wait}."
POOL_SPENT_REPLY = "The guild's used up its answers for the moment — try me again in about {wait}."

#: Long retry intervals use minutes.
RETRY_SECONDS_UNTIL = 120

__all__ = [
    "ANCHOR_CACHE",
    "COMPLETION_RESERVE_TOKENS",
    "ContextBudgetError",
    "MAX_TOOL_ROUNDS",
    "POOL_SPENT_REPLY",
    "RATE_LIMITED_REPLY",
    "SPOOFED_NOTE",
    "STRATEGY_GROUNDING_FAILURE_REPLY",
    "Anchor",
    "ChatPilot",
    "ChatTurn",
    "Focus",
    "Generation",
    "Handling",
    "defuse_notes",
    "retry_note",
    "tool_trace",
    "unglue_first_bullet",
]


def retry_note(seconds: float) -> str:
    """Format a retry interval, rounding up."""
    whole = max(math.ceil(seconds), 1)
    return f"{whole}s" if whole <= RETRY_SECONDS_UNTIL else f"{math.ceil(whole / 60)} min"


@dataclass
class ChatTurn:
    """One remembered line of a channel's conversation with the bot."""

    role: str
    content: str
    #: Prevents duplicate history and reply-chain turns.
    message_id: str | None = None
    #: Monotonic timestamp; unstamped turns do not expire.
    at: float | None = None


@dataclass
class Focus:
    """A channel's most recently posted card."""

    #: The card in the words the card itself used -- see
    #: :meth:`ChatPilot._card_summary`.
    card: str
    at: float


@dataclass
class Anchor:
    """An answered exchange keyed by its reply message."""

    channel_id: str
    #: The question that was asked and the answer that was given, as the turns
    #: they were remembered as -- so re-injecting them puts back exactly what
    #: aged out, not a paraphrase of it.
    question: ChatTurn
    answer: ChatTurn


@dataclass
class Handling:
    """How the chatbot handled one message."""

    handled: bool
    #: The gate's own words, for the log. Never shown in Discord.
    reason: str
    #: The answer, when there was one.
    answered: Generation | None = None

    def __bool__(self) -> bool:  # pragma: no cover - clarity at call sites
        return self.handled


@dataclass
class Generation:
    """One generation's reply, tool calls, and diagnostics."""

    reply: str = ""
    rounds: int = 0
    tool_calls: list[str] = field(default_factory=list)
    #: One per tool call, with its arguments, duration and outcome.
    outcomes: list[tools.ToolOutcome] = field(default_factory=list)
    created: list[str] = field(default_factory=list)
    #: Only amendments whose cards were successfully posted. ``created`` also
    #: includes rows left behind when Discord rejected the card post.
    posted: list[str] = field(default_factory=list)
    #: Provider-returned material from each model round, in order. This is
    #: diagnostic data only; it never changes the messages replayed to the model.
    model_rounds: list[dict[str, Any]] = field(default_factory=list)
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
        """Return ``answered`` or ``failed``."""
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


def _message_parts(response: Any) -> tuple[str | None, str | None, list]:
    """Raw content, thinking and calls from a response or dict stand-in."""
    message = getattr(response, "message", None)
    if message is None and isinstance(response, dict):
        message = response.get("message")
    if message is None:
        return None, None, []
    if isinstance(message, dict):
        content = message.get("content")
        thinking = message.get("thinking")
        calls = message.get("tool_calls")
    else:
        content = getattr(message, "content", None)
        thinking = getattr(message, "thinking", None)
        calls = getattr(message, "tool_calls", None)
    return (
        content if isinstance(content, str) else None,
        thinking if isinstance(thinking, str) else None,
        list(calls or []),
    )


def _message_text(response: Any) -> tuple[str, list]:
    """``(content, tool_calls)`` from a ChatResponse, tolerating a plain dict."""
    content, _thinking, calls = _message_parts(response)
    return (content or "").strip(), calls


def _usage(response: Any) -> tuple[int | None, int | None]:
    """Return optional Ollama prompt and completion counts."""

    def count(key: str) -> int | None:
        value = response.get(key) if isinstance(response, dict) else getattr(response, key, None)
        try:
            return int(value) if value is not None else None
        except (TypeError, ValueError):  # pragma: no cover - defensive
            return None

    return count("prompt_eval_count"), count("eval_count")


def _brief(arguments: dict, limit: int = 200) -> str:
    """Render bounded tool arguments for one log line."""
    rendered = ", ".join(f"{key}={value!r}" for key, value in (arguments or {}).items())
    return rendered if len(rendered) <= limit else rendered[: limit - 1] + "…"


def tool_trace(outcomes: Sequence[tools.ToolOutcome]) -> list[dict]:
    """Return stored diagnostics for tool calls."""
    return [
        {
            "name": outcome.name or "?",
            "round": outcome.round,
            "arguments": _brief(outcome.arguments),
            "output": outcome.output,
            "ms": outcome.duration_ms,
            "outcome": outcome.outcome,
            "created": list(outcome.created),
            "posted": list(outcome.posted),
        }
        for outcome in outcomes
    ]


#: A list marker accidentally placed after a heading.
GLUED_BULLET = ": - "


_SCHEDULE_RUN_LINE_RE = re.compile(r"^\s*(?:[-*]\s*)?`?\[[0-9a-fA-F]{8}\]`?\s+\S")


def _tidy_blank_lines(text: str) -> str:
    """Collapse excess blank lines without splitting schedule rows."""
    normalised = re.sub(r"\n(?:[ \t]*\n){1,}", "\n\n", text)
    lines = normalised.split("\n")
    compact: list[str] = []
    for index, line in enumerate(lines):
        between_runs = (
            not line
            and compact
            and index + 1 < len(lines)
            and _SCHEDULE_RUN_LINE_RE.match(compact[-1]) is not None
            and _SCHEDULE_RUN_LINE_RE.match(lines[index + 1]) is not None
        )
        if not between_runs:
            compact.append(line)
    return "\n".join(compact)


_EMPTY_PLACEHOLDER_RE = re.compile(r"`?<\s*none\s*>`?", re.IGNORECASE)
_SCHEDULE_CALL_RE = re.compile(r"`?\bget_schedule\s*\([^)]*\)`?", re.IGNORECASE | re.DOTALL)
_SCHEDULE_ARGUMENT_RE = re.compile(
    r"`?(?:\b(?P<assigned>participant|scope|week|day)\s*=|"
    r"['\"](?P<json>participant|scope|week|day)['\"]\s*:)\s*"
    r"(?:(?P<quote>['\"])(?P<quoted>[^'\"]+)(?P=quote)|"
    r"(?P<bare><@(?:[!&])?\d+>|[\w-]+))`?",
    re.IGNORECASE,
)
_BARE_SCHEDULE_RE = re.compile(
    r"what(?:'s|s| is)\s+(?:on|for)\s+"
    r"(?:today|tonight|tomorrow|tmr|tmrw|mon(?:day)?|tue(?:sday)?|wed(?:nesday)?|"
    r"thu(?:rsday)?|fri(?:day)?|sat(?:urday)?|sun(?:day)?)(?:\s+(?:in this channel|here))?",
    re.IGNORECASE,
)
_SCHEDULE_QUESTION_RE = re.compile(r"what(?:'s|s| is)\s+(?:on|for)\s+.+", re.IGNORECASE)
_CHANNEL_QUALIFIER_RE = re.compile(r"\b(?:this channel|in here|here|our runs)\b", re.IGNORECASE)
_PERSON_QUALIFIER_RE = re.compile(
    r"\b(?:for me|my runs|my schedule|am i|do i|i am|i'm|myself)\b|<@!?\d+>",
    re.IGNORECASE,
)


def _schedule_defaults(
    text: str, bot_user_id: str | None, self_role_id: str | None
) -> tuple[bool, bool]:
    """Return trusted schedule-scope defaults for a complete question."""
    cleaned = text or ""
    if bot_user_id:
        cleaned = re.sub(rf"<@!?{re.escape(bot_user_id)}>", " ", cleaned)
    if self_role_id:
        cleaned = re.sub(rf"<@&{re.escape(self_role_id)}>", " ", cleaned)
    cleaned = re.sub(r"[?!.,]+\s*$", "", cleaned).strip()
    all_channels = re.search(r"\b(?:whole server|all channels)\b", cleaned, re.I)
    whole_group = re.search(r"\b(?:whole group|everyone)\b", cleaned, re.I)
    complete_question = _SCHEDULE_QUESTION_RE.fullmatch(cleaned) is not None
    explicit_channel = _CHANNEL_QUALIFIER_RE.search(cleaned) is not None
    explicit_person = _PERSON_QUALIFIER_RE.search(cleaned) is not None
    force_all = bool(all_channels or whole_group) or (complete_question and not explicit_channel)
    force_group = _BARE_SCHEDULE_RE.fullmatch(cleaned) is not None or (
        bool(whole_group) and not explicit_person
    )
    return force_all, force_group


#: A write claim that must never survive a refusal: the model said a card went
#: up even though the tool refused to post one.
_WRITE_CLAIM_RE = re.compile(r"card'?s up|\bposted\b|✅", re.IGNORECASE)


def _looks_like_clarification(text: str) -> bool:
    """Whether a refused-turn reply already asks the member a question.

    The old check required a trailing ``?``, so a good clarification ending in
    ``... tonight 23:30.`` was overwritten with the raw tool refusal. Any
    ``?`` counts, unless the reply also claims a card went up (a lie after a
    refusal that the overwrite must still correct).
    """
    if "?" not in (text or ""):
        return False
    return _WRITE_CLAIM_RE.search(text or "") is None


def _strip_tool_directives(text: str) -> str:
    """Drop model-only tool instructions that must never reach a member."""
    cleaned = text or ""
    # Keep the spoken options, drop the tool-only half:
    # "Ask in words ('Easy ...?') and pass the short form (HBellona) back to
    # the tool." -> "Ask in words ('Easy ...?')."
    cleaned = re.sub(r"\s+and pass the short form\b[^.?!]*", "", cleaned, flags=re.IGNORECASE)
    cleaned = re.sub(
        r"[^.?!]*\bshort forms? are for the tool only\b[^.?!]*[.?!]?",
        "",
        cleaned,
        flags=re.IGNORECASE,
    )
    cleaned = re.sub(
        r"[^.?!]*\bfor the tool only\b[^.?!]*[.?!]?",
        "",
        cleaned,
        flags=re.IGNORECASE,
    )
    cleaned = re.sub(
        r"[^.?!]*\bnever show them to a member\b[^.?!]*[.?!]?",
        "",
        cleaned,
        flags=re.IGNORECASE,
    )
    cleaned = re.sub(r"\s*\bback to the tool\b[^.?!]*[.?!]?", "", cleaned, flags=re.IGNORECASE)
    # Model-only guard: "Ask them which one they mean -- do not choose ..." ->
    # "Ask them which one they mean."
    cleaned = re.sub(
        r"\s*(?:--|—|–)\s*do not (?:choose|pick|guess|offer)[^.?!]*\.",
        ".",
        cleaned,
        flags=re.IGNORECASE,
    )
    cleaned = re.sub(r"\bpropose_\w+\b", "the schedule change", cleaned)
    cleaned = re.sub(r"\bweekly\s*=\s*true\b", "weekly", cleaned, flags=re.IGNORECASE)
    cleaned = re.sub(r"[ \t]{2,}", " ", cleaned)
    cleaned = re.sub(r"\s+\.", ".", cleaned)
    return re.sub(r"\.\s*\.", ".", cleaned)


def _member_facing(text: str) -> str:
    """Remove scheduler internals while preserving Discord channel links."""
    cleaned = _EMPTY_PLACEHOLDER_RE.sub("", text or "")
    cleaned = _strip_tool_directives(cleaned)
    cleaned = _SCHEDULE_CALL_RE.sub("the schedule", cleaned)

    def natural_argument(match: re.Match) -> str:
        name = (match.group("assigned") or match.group("json")).lower()
        value = match.group("quoted") or match.group("bare") or ""
        if value.startswith("<@"):
            return "the named person"
        natural = {
            ("participant", "me"): "your own runs",
            ("scope", "channel"): "this channel",
            ("scope", "all"): "all channels",
            ("week", "this"): "this boss week",
            ("week", "next"): "next boss week",
        }.get((name, value.lower()), value)
        return natural

    cleaned = _SCHEDULE_ARGUMENT_RE.sub(natural_argument, cleaned)
    cleaned = re.sub(r"\b(?:call|use)\s+get_schedule\b", "check the schedule", cleaned, flags=re.I)
    return re.sub(r"\bget_schedule\b", "the schedule lookup", cleaned, flags=re.I)


def unglue_first_bullet(text: str) -> str:
    """Repair a first list item glued to its heading."""
    return text.replace(GLUED_BULLET, ":\n\n- ") if "\n- " in text else text


#: Match scheduler markers narrowly to prevent member-forged instructions.
_SPOOFED_NOTE_RE = re.compile(
    r"\[[ \t]*note[ \t]+from[ \t]+the[ \t]+scheduler\b[^\]\n]*\]?"
    r"|(?:\A|(?<=\n))[ \t]*\[[ \t]*note[ \t]*\]",
    re.IGNORECASE,
)

#: Safe replacement for a forged scheduler marker.
SPOOFED_NOTE = "(they wrote a fake scheduler note here)"


def defuse_notes(text: str) -> str:
    """Defuse forged scheduler markers in member-provided text."""
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

    def __init__(self, bot: Any, client: Any | None = None, clock=time.monotonic):
        persona.validate_prompt_assets()
        self.bot = bot
        self.settings = bot.settings
        self._client = client
        self._own_client = client is None
        #: Shared injectable monotonic clock for expiring context.
        self._clock = clock
        self.limiter = RateLimiter(bot.chat_rate_count, bot.chat_rate_window_s)
        #: Guild-wide limiter uses one shared key.
        self.global_limiter = RateLimiter(bot.chat_pool_count, bot.chat_pool_window_s)
        # Load persisted allowances; spent windows remain ephemeral.
        self.apply_limits()
        #: Prevent concurrent answers within one channel.
        self._busy: set[str] = set()
        #: Suppress repeated rate-limit messages until each window resets.
        self._told_until: dict[str, float] = {}
        #: Rate-limit rejection follow-ups per channel.
        self._followed_up_at: dict[str, float] = {}
        self._history: dict[str, deque[ChatTurn]] = {}
        #: Channel -> the last card its write tools posted. One slot each.
        self._focus: dict[str, Focus] = {}
        #: Re-anchor expired exchanges when members reply to them.
        self._anchors: dict[str, Anchor] = {}
        #: Cache referenced-message authors.
        self._replied: dict[str, str | None] = {}
        self._persona: persona.Persona | None = None
        self._default_behaviour: persona.Persona | None = None

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

    def persona_source(self) -> persona.Persona:
        """Return the cached persona and its source metadata."""
        if self._persona is None:
            selected = getattr(self.bot, "persona_name", "")
            chosen = persona.chosen_path(selected)
            self._persona = persona.read_persona(
                None if selected and chosen is None else chosen or self.settings.persona_path
            )
        return self._persona

    def persona_text(self) -> str:
        """Just the words, which is all the prompt builders want."""
        return self.persona_source().text

    def default_behaviour_source(self) -> persona.Persona:
        """Deployment default behaviour, cached and reloaded with the identity."""
        if self._default_behaviour is None:
            self._default_behaviour = persona.read_default_behaviour()
        return self._default_behaviour

    def default_behaviour_text(self) -> str:
        return self.default_behaviour_source().text

    def reload_persona(self) -> str:
        self._persona = None
        self._default_behaviour = None
        return self.persona_text()

    def answering(self) -> list[str]:
        """Return sorted channels with an answer in flight."""
        return sorted(self._busy)

    # -- intake ------------------------------------------------------------
    async def offer(self, message: Any) -> Handling:
        """Handle one guild message after evaluating the gate exactly once."""
        bot_user_id = getattr(getattr(self.bot, "user", None), "id", None)
        # Avoid fetching reply parents before cheap gates pass.
        replied_author_id = (
            await self.replied_author_id(message)
            if gate.would_check_mention(
                message, self.settings, bot_user_id=bot_user_id, enabled=self.enabled
            )
            else None
        )
        # Share one staff decision between gate and tool authority checks.
        is_admin = self._is_admin(getattr(message, "author", None))
        self_role_id = self._self_role_id(message)
        decision = gate.decide(
            message,
            self.settings,
            bot_user_id=bot_user_id,
            enabled=self.enabled,
            is_admin=is_admin,
            limiter=self.limiter,
            global_limiter=self.global_limiter,
            self_role_id=self_role_id,
            replied_author_id=replied_author_id,
        )
        if decision.act or decision.busy:
            # Keep `gate.decide` pure while refreshing the Limits page.
            events.notify()
        if not decision.act:
            if decision.busy:
                log.info("chat: %s from %s", decision.reason, getattr(message.author, "id", "?"))
                await self._react(message, gate.RATE_LIMITED_REACTION)
                await self._say_limited(message, decision)
                # Addressed chat remains ours even when rate-limited.
                return Handling(True, decision.reason)
            log.debug("chat: ignoring a message (%s)", decision.reason)
            return Handling(False, decision.reason)

        channel_id = str(origin_ids(message.channel)[0])
        if channel_id in self._busy:
            # Drop, rather than queue, a second question in the same channel.
            log.info("chat: channel %s is already answering; dropping", channel_id)
            await self._react(message, gate.CHANNEL_BUSY_REACTION)
            return Handling(True, "already answering")

        self._busy.add(channel_id)
        # Mark the message while generation is in progress.
        await self._react(message, gate.SEEN_REACTION)
        # Staff wait longer for the shared model; normal requests shed quickly.
        wait_s = (
            self.settings.chat_pilot_timeout if is_admin else self.settings.chat_pilot_lock_wait_s
        )
        if not await acquire_within(wait_s, chat_label(channel_id)):
            # The gate already spent this request's rate-limit slot.
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
            # Hold the shared model across every round of this answer.
            return Handling(
                True,
                "ok",
                await self._answer(
                    message,
                    channel_id,
                    is_admin,
                    bot_user_id=str(bot_user_id) if bot_user_id is not None else None,
                    self_role_id=str(self_role_id) if self_role_id is not None else None,
                ),
            )
        finally:
            release()
            self._busy.discard(channel_id)
            await self._unreact(message, gate.SEEN_REACTION)

    async def _say_limited(self, message: Any, decision: gate.ChatDecision) -> None:
        """Post one static rate-limit reply per refusal episode."""
        author_id = str(getattr(getattr(message, "author", None), "id", ""))
        if not self._first_refusal(author_id, decision.retry_after_s):
            return
        template = POOL_SPENT_REPLY if decision.reason == gate.POOL_SPENT else RATE_LIMITED_REPLY
        # Their own allowance, not the guild's: telling somebody with a raised
        # limit that they have had the default number of answers is worse than
        # saying nothing, because it is confidently wrong about their own case.
        count = self.limiter.limit_for(author_id)[0]
        await self._post(
            message,
            template.format(
                count=count,
                plural="" if count == 1 else "s",
                wait=retry_note(decision.retry_after_s),
            ),
        )

    def _first_refusal(self, user_id: str, retry_after_s: float) -> bool:
        """Return whether this is a member's first active refusal."""
        now = time.monotonic()
        for key in [key for key, until in self._told_until.items() if until <= now]:
            del self._told_until[key]
        if self._told_until.get(user_id, 0.0) > now:
            return False
        self._told_until[user_id] = now + max(retry_after_s, 0.0)
        return True

    def apply_limits(self) -> None:
        """Apply runtime and per-member rate-limit settings."""
        self.limiter.count = self.bot.chat_rate_count
        self.limiter.window = self.bot.chat_rate_window_s
        self.global_limiter.count = self.bot.chat_pool_count
        self.global_limiter.window = self.bot.chat_pool_window_s
        self.limiter.replace_overrides(
            {
                row["user_id"]: (row["count"], row["window_s"])
                for row in self.bot.repo.list_rate_limits()
            }
        )

    def forget_limit(self, user_id: int | str) -> None:
        """Reset one member's allowance and refusal notice."""
        self.limiter.reset(user_id)
        self._told_until.pop(str(user_id), None)

    def _self_role_id(self, message: Any) -> int | None:
        """Return the bot's managed integration role id, if present."""
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
        """Return a replied-to author id, fetching unresolved references once."""
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
        """Apply the shared live-member administrator rule."""
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

    def reply_overlay(self, author: Any) -> str:
        """Effective non-default profile instructions for an authorized asker."""
        role_ids = [getattr(role, "id", role) for role in (getattr(author, "roles", None) or ())]
        configured = behaviour_plugins.decode(
            self.bot.repo.get_config(behaviour_plugins.CONFIG_KEY, "[]")
        )
        selectable = behaviour_plugins.decode_catalog(
            self.bot.repo.get_config(behaviour_plugins.SELECTABLE_CONFIG_KEY, "[]")
        )
        resolution = behaviour_plugins.resolve(
            selected=self.bot.repo.get_reply_style(getattr(author, "id", "")),
            selectable=selectable,
            assignments=configured,
            role_ids=role_ids,
            default_instructions=self.default_behaviour_text(),
        )
        return "" if resolution.source == "default" else resolution.prompt_instructions()

    async def _answer(
        self,
        message: Any,
        channel_id: str,
        is_admin: bool = False,
        *,
        bot_user_id: str | None = None,
        self_role_id: str | None = None,
    ) -> Generation:
        author_id = str(message.author.id)
        text = (message.content or "").strip()
        force_all_channels, force_group_schedule = _schedule_defaults(
            text, bot_user_id, self_role_id
        )
        context = tools.ToolContext(
            bot=self.bot,
            author_id=author_id,
            channel_id=channel_id,
            message_id=str(message.id),
            bot_user_id=bot_user_id,
            self_role_id=self_role_id,
            force_all_channels=force_all_channels,
            force_group_schedule=force_group_schedule,
            is_admin=is_admin,
        )
        overlay = self.reply_overlay(message.author)
        conversation = self.build_conversation(message, channel_id, overlay)
        intent = strategy.route_strategy_intent(text, self.bot.bosses)
        if intent.kind == "unresolved":
            log.info(
                "chat: strategy intent unresolved for %r (refs=%d) in channel %s",
                text[:120],
                len(intent.references),
                channel_id,
            )
            result = await self._rewrite_fixed(
                conversation,
                context,
                overlay,
                intent.reply or strategy.STRATEGY_CLARIFICATION_REPLY,
            )
        else:
            result = await self.generate(
                conversation,
                context,
                overlay,
                strategy_references=intent.references,
            )

        reply = result.reply or FAILURE_REPLY
        posted = await self._post(message, reply)
        # Remember only successfully posted conversation.
        asked = ChatTurn("user", self._speaker(author_id, text), str(message.id))
        # The posted id supports re-anchoring and de-duplication.
        answered = ChatTurn("assistant", reply, str(getattr(posted, "id", "") or "") or None)
        self.remember(channel_id, asked)
        self.remember(channel_id, answered)
        self.anchor(answered.message_id, channel_id, asked, answered)
        # Summary only; DEBUG logs tool arguments.
        log.info(
            "chat: answered %s in channel %s in %d ms (%d round(s), %d tool call(s)%s)%s%s",
            author_id,
            channel_id,
            result.latency_ms,
            result.rounds,
            len(result.tool_calls),
            f": {result.trace}" if result.outcomes else "",
            f" -> proposal {', '.join(result.posted)}" if result.posted else "",
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
        """Record a handled generation without risking its posted reply."""
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
                model_rounds=result.model_rounds,
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
        """Ask for clarification after an eligible chatbot card is rejected."""
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
            # Avoid one question per card during bulk cleanup.
            log.info("chat: channel %s had a follow-up %.0fs ago; dropping", channel_id, now - last)
            return Handling(False, "a follow-up was asked here too recently")
        if channel_id in self._busy:
            # Do not queue stale-card questions behind live answers.
            log.info("chat: channel %s is already answering; dropping the follow-up", channel_id)
            return Handling(False, "already answering")
        if MODEL_LOCK.locked():
            # Unsolicited follow-ups yield immediately to other model work.
            log.debug("chat: the model is busy; no follow-up on the rejected card")
            return Handling(False, "the model is busy")

        # Claim every amendment together so each card prompts at most once.
        mine = [a for a in rows if self.bot.repo.chat_interaction_for_amendment(a["id"])]
        claimed = [a for a in mine if self.bot.repo.claim_chat_followup(a["id"])]
        if len(claimed) != len(mine):
            log.info("chat: card %s has already been followed up", card_message_id)
            return Handling(False, "already followed up")

        self._busy.add(channel_id)
        self._followed_up_at[channel_id] = now
        try:
            # No await occurred since `locked()`; hold through generation.
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
        """Generate and post a read-only clarification question."""
        channel_id = str(origin_ids(channel)[0])
        context = tools.ToolContext(
            bot=self.bot,
            author_id=author_id,
            channel_id=channel_id,
            message_id=str(card_message_id or ""),
            read_only=True,
        )
        question = followup.prompt(self.bot, amendments, author_id)
        guild = getattr(channel, "guild", None)
        get_member = getattr(guild, "get_member", None)
        member = get_member(int(author_id)) if get_member is not None else None
        overlay = self.reply_overlay(member)
        # gpt-oss hoists system turns, so provenance travels in a synthetic user turn.
        conversation = self.assemble(
            [*self.history(channel_id), ChatTurn("user", question)], channel_id, overlay
        )
        result = await self.generate(conversation, context, overlay)
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
            # Keep the note beside its reply so later references retain context.
            note = followup.memory_note(self.bot, amendments)
            posted_id = str(getattr(posted, "id", "") or "") or None
            asked = ChatTurn("user", note)
            answered = ChatTurn("assistant", result.reply, posted_id)
            self.remember(channel_id, asked)
            self.remember(channel_id, answered)
            # Re-anchor late replies to this clarification.
            self.anchor(posted_id, channel_id, asked, answered)
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
        """Render roster-derived speaker identity and defused member text."""
        from ..api import service

        return f"{service.member_name(self.bot, user_id)}: {defuse_notes(text)}"

    def history(self, channel_id: str) -> deque[ChatTurn]:
        """Return this channel's live, TTL-pruned conversation."""
        turns = self._history.setdefault(str(channel_id), deque(maxlen=HISTORY_EXCHANGES * 2))
        cutoff = self._clock() - self.settings.chat_pilot_history_ttl_s
        while turns and turns[0].at is not None and turns[0].at <= cutoff:
            turns.popleft()
        return turns

    def remember(self, channel_id: str, turn: ChatTurn) -> None:
        """Append a turn, stamping unstamped turns."""
        if turn.at is None:
            turn.at = self._clock()
        self.history(channel_id).append(turn)

    def forget(self, channel_id: str | None = None) -> None:
        """Clear all memory, or memory for one channel."""
        if channel_id is None:
            self._history.clear()
            self._focus.clear()
            self._anchors.clear()
            return
        key = str(channel_id)
        self._history.pop(key, None)
        self._focus.pop(key, None)
        for message_id in [mid for mid, a in self._anchors.items() if a.channel_id == key]:
            del self._anchors[message_id]

    # -- the current focus -------------------------------------------------
    def note_card(self, channel_id: str, amendment_id: str) -> None:
        """Record the most recently posted card for a channel."""
        summary = self._card_summary(amendment_id)
        if summary:
            self._focus[str(channel_id)] = Focus(summary, self._clock())

    def _card_summary(self, amendment_id: str) -> str:
        """Return a stored card summary and participant names."""
        try:
            from ..api import service

            row = self.bot.repo.get_amendment(amendment_id) or {}
            summary = (row.get("summary") or "").strip()
            people = row.get("participants") or ()
            party = ", ".join(service.member_name(self.bot, uid) for uid in people)
            return f"{summary} — {party}" if summary and party else summary
        except Exception:  # noqa: BLE001 - context must never cost an answer
            log.exception("chat: could not describe card %s", amendment_id)
            return ""

    def focus(self, channel_id: str) -> str:
        """Return unexpired current-card context for a channel."""
        key = str(channel_id)
        entry = self._focus.get(key)
        if entry is None:
            return ""
        # `>=`, matching the cutoff `history` sweeps with, so a card and the
        # exchange that raised it never disagree about whether they are still
        # current.
        if self._clock() - entry.at >= self.settings.chat_pilot_history_ttl_s:
            del self._focus[key]
            return ""
        return entry.card

    # -- re-anchoring a reply ----------------------------------------------
    def anchor(
        self, message_id: str | None, channel_id: str, question: ChatTurn, answer: ChatTurn
    ) -> None:
        """Keep a bounded answered exchange keyed by its reply id."""
        if not message_id:
            return
        if len(self._anchors) >= ANCHOR_CACHE:
            self._anchors.pop(next(iter(self._anchors)), None)
        self._anchors[str(message_id)] = Anchor(str(channel_id), question, answer)

    @staticmethod
    def replied_message_id(message: Any) -> str | None:
        """The id of the message this one replies to, resolved or not.

        Both halves are needed: discord.py fills ``reference.resolved`` from its
        in-memory cache, which a restart empties, and leaves ``message_id`` set
        either way.
        """
        reference = getattr(message, "reference", None)
        if reference is None:
            return None
        resolved = getattr(reference, "resolved", None)
        parent_id = getattr(resolved, "id", None) if resolved is not None else None
        return str(parent_id or getattr(reference, "message_id", None) or "") or None

    def reanchored(self, message: Any, seen: set[str]) -> list[ChatTurn]:
        """Return an unexpired anchor's unseen turns for a reply."""
        found = self._anchors.get(self.replied_message_id(message) or "")
        # The answer is what the reply points at, so its presence is what decides
        # whether the exchange is already in the prompt -- the question beside it
        # may have no id of its own (a rejection follow-up's note has none).
        if found is None or found.answer.message_id in seen:
            return []
        return [turn for turn in (found.question, found.answer) if turn.message_id not in seen]

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

    def build_conversation(
        self, message: Any, channel_id: str, role_overlay: str = ""
    ) -> list[dict[str, str]]:
        """Build a prompt from anchored, live, and reply-chain context."""
        live = self.history(channel_id)
        seen = {turn.message_id for turn in live if turn.message_id}
        # Reply chains and anchors may reach the same message; de-duplicate them.
        chain = [turn for turn in self.reply_chain(message) if turn.message_id not in seen]
        seen |= {turn.message_id for turn in chain if turn.message_id}
        earlier = [*self.reanchored(message, seen), *live, *chain]
        question = ChatTurn(
            "user", self._speaker(str(message.author.id), (message.content or "").strip())
        )
        return self.assemble([*earlier, question], channel_id, role_overlay)

    def assemble(
        self,
        turns: Sequence[ChatTurn],
        channel_id: str | None = None,
        role_overlay: str = "",
    ) -> list[dict[str, str]]:
        """Assemble a system prompt and budgeted conversation turns."""
        now = utcnow()
        week = current_week_start(
            self.bot.tz, self.settings.reset_weekday, self.settings.reset_time, now
        )
        system = persona.component_system_prompt(
            persona.PromptComponents(
                identity=self.persona_text(),
                default_behaviour=self.default_behaviour_text(),
                active_profile=role_overlay,
            ),
            persona.clock_header(now, self.bot.tz, week),
            persona.runtime_line(self.settings.chat_pilot_model),
            persona.focus_line(self.focus(channel_id) if channel_id is not None else ""),
        )
        rendered = [{"role": t.role, "content": t.content} for t in turns]
        available = max(
            256,
            min(
                CONVERSATION_BUDGET_TOKENS,
                prompt_budget(self.settings.ollama_num_ctx) - estimate_tokens(system),
            ),
        )
        while len(rendered) > 1 and estimate_messages(rendered) > available:
            rendered.pop(0)
        return [{"role": "system", "content": system}, *rendered]

    # -- the model ---------------------------------------------------------
    async def generate(
        self,
        conversation: list[dict[str, str]],
        context: tools.ToolContext,
        role_overlay: str = "",
        strategy_references: Sequence[BossReference] = (),
    ) -> Generation:
        """Run the tool loop until the model answers in words. Never raises."""
        started = time.monotonic()
        result = Generation()
        try:
            await asyncio.wait_for(
                self._loop(conversation, context, result, role_overlay, strategy_references),
                timeout=self.settings.chat_pilot_timeout,
            )
        except TimeoutError:
            result.error = f"no answer within {self.settings.chat_pilot_timeout:.0f}s"
            log.warning("chat: the model did not answer in time")
        except Exception as exc:  # noqa: BLE001 - chat must never break the bot
            result.error = f"{type(exc).__name__}: {exc}"
            log.exception("chat: the model call failed")
        self._finalize_write_reply(result)
        self._finalize_strategy_reply(result, strategy_references)
        if result.reply:
            result.reply = self._tidy(_member_facing(result.reply))
        result.created = list(context.created)
        result.posted = list(context.posted)
        result.latency_ms = int((time.monotonic() - started) * 1000)
        return result

    async def _rewrite_fixed(
        self,
        conversation: list[dict[str, str]],
        context: tools.ToolContext,
        role_overlay: str,
        fixed: str,
    ) -> Generation:
        """Say a fixed strategy meaning in voice. Never raises; falls back to fixed."""
        started = time.monotonic()
        result = Generation(rounds=1)
        rewrite = (
            "Say this in your own voice (identity + default behaviour + active "
            "reply style), preserving its meaning exactly. You may add one small "
            f"in-character touch and nothing else: {fixed!r}"
        )
        try:
            response = await asyncio.wait_for(
                self._chat(
                    [*conversation, {"role": "user", "content": rewrite}],
                    False,
                    context,
                    role_overlay,
                ),
                timeout=self.settings.chat_pilot_timeout,
            )
            content, _calls = _message_text(response)
            result.reply = self._tidy(_member_facing(content)) or fixed
            prompt, completion = _usage(response)
            result.add_usage(prompt, completion)
        except TimeoutError:
            result.error = f"no answer within {self.settings.chat_pilot_timeout:.0f}s"
            result.reply = fixed
            log.warning("chat: fixed rewrite timed out, using static reply")
        except Exception as exc:  # noqa: BLE001 - chat must never break the bot
            result.error = f"{type(exc).__name__}: {exc}"
            result.reply = fixed
            log.exception("chat: fixed rewrite failed, using static reply")
        result.latency_ms = int((time.monotonic() - started) * 1000)
        return result

    def _finalize_write_reply(self, result: Generation) -> None:
        """Replace unresolved write claims with the tool's outcome."""
        for outcome in reversed(result.outcomes):
            if not tools.is_write_tool(outcome.name):
                continue
            if outcome.ok and outcome.posted:
                return
            if outcome.error == tools.REFUSED and _looks_like_clarification(result.reply or ""):
                return
            if result.reply:
                detail = self._tidy(_member_facing(outcome.output))
                status = (
                    "The requested card was posted, but the request did not finish cleanly."
                    if outcome.posted
                    else "The requested card was not posted."
                )
                result.reply = self._tidy(status + (f" {detail}" if detail else ""))
            return

    @staticmethod
    def _strategy_grounded(result: Generation, references: Sequence[BossReference]) -> bool:
        """Whether every deterministically required guide was retrieved successfully."""
        successful = {
            outcome.arguments.get("boss")
            for outcome in result.outcomes
            if outcome.name == "get_boss_strategy" and outcome.ok
        }
        return all(reference.short in successful for reference in references)

    def _finalize_strategy_reply(
        self, result: Generation, references: Sequence[BossReference]
    ) -> None:
        """Never let a mechanics answer survive missing checked-in grounding."""
        if not references:
            return
        if not self._strategy_grounded(result, references) or (result.error or "").startswith(
            "ContextBudgetError:"
        ):
            result.reply = STRATEGY_GROUNDING_FAILURE_REPLY
            result.error = result.error or "strategy grounding unavailable"

    async def _loop(
        self,
        conversation: list[dict[str, str]],
        context: tools.ToolContext,
        result: Generation,
        role_overlay: str = "",
        strategy_references: Sequence[BossReference] = (),
    ) -> None:
        messages: list[dict[str, Any]] = list(conversation)
        if strategy_references and not await self._prefetch_strategy(
            messages, context, result, strategy_references
        ):
            result.reply = STRATEGY_GROUNDING_FAILURE_REPLY
            result.error = "strategy grounding unavailable"
            return
        for round_number in range(1, MAX_TOOL_ROUNDS + 1):
            result.rounds = round_number
            # Reserve the round after a posted write for its confirmation.
            last = round_number == MAX_TOOL_ROUNDS
            posted_write = any(
                outcome.ok and outcome.posted and tools.is_write_tool(outcome.name)
                for outcome in result.outcomes
            )
            asked_at = time.monotonic()
            response = await self._chat(
                messages,
                with_tools=not last and not posted_write,
                context=context,
                role_overlay=role_overlay,
            )
            raw_content, thinking, calls = _message_parts(response)
            content = (raw_content or "").strip()
            model_ms = int((time.monotonic() - asked_at) * 1000)
            result.model_ms += model_ms
            result.add_usage(*_usage(response))
            result.model_rounds.append(
                {
                    "round": round_number,
                    "content": raw_content,
                    "thinking": thinking,
                    "requested_tools": [_call_parts(call)[0] for call in calls],
                }
            )
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
                outcome.round = round_number
                result.outcomes.append(outcome)
                result.tools_ms += outcome.duration_ms
                if outcome.ok and outcome.posted:
                    # The one place the pilot learns a write tool actually
                    # posted something. The last id wins: a later card in the
                    # same turn is the one on screen.
                    self.note_card(context.channel_id, outcome.posted[-1])
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
                    f" card {', '.join(outcome.posted)}" if outcome.posted else "",
                )
                messages.append({"role": "tool", "name": name, "content": outcome.output})
        # Four rounds of tools and still nothing said.
        result.error = "the model kept calling tools"
        log.warning("chat: gave up after %d tool rounds", MAX_TOOL_ROUNDS)

    async def _prefetch_strategy(
        self,
        messages: list[dict[str, Any]],
        context: tools.ToolContext,
        result: Generation,
        references: Sequence[BossReference],
    ) -> bool:
        """Retrieve canonical guide documents before the first model round."""
        calls = []
        for reference in references:
            arguments = {"boss": reference.short}
            if reference.difficulty is not None:
                arguments["difficulty"] = reference.difficulty
            calls.append({"function": {"name": "get_boss_strategy", "arguments": arguments}})
        messages.append({"role": "assistant", "content": "", "tool_calls": calls})
        for call in calls:
            name, arguments = _call_parts(call)
            result.tool_calls.append(name)
            outcome = await tools.run(context, name, arguments)
            outcome.round = 0
            result.outcomes.append(outcome)
            result.tools_ms += outcome.duration_ms
            log.debug(
                "chat: round 0 prefetch %s(%s) -> %s in %d ms",
                name,
                _brief(outcome.arguments),
                outcome.outcome,
                outcome.duration_ms,
            )
            messages.append({"role": "tool", "name": name, "content": outcome.output})
            if not outcome.ok:
                return False
        return True

    def voice_reminder(self, role_overlay: str = "") -> dict[str, str]:
        """Return the final, scheduler-identified voice cue."""
        return {
            "role": "user",
            "content": persona.component_voice_reminder(
                self.default_behaviour_text(), role_overlay, self.persona_text()
            ),
        }

    async def _chat(
        self,
        messages: list[dict[str, Any]],
        with_tools: bool,
        context: tools.ToolContext,
        role_overlay: str = "",
    ) -> Any:
        # Tool execution independently rejects writes on read-only turns.
        offered = (tools.read_tools() if context.read_only else tools.TOOLS) if with_tools else []
        outgoing = self._budgeted_messages(messages, offered, role_overlay)
        log.debug(
            "chat: model %s think=%r tools=%d",
            self.settings.chat_pilot_model,
            self.settings.chat_think,
            len(offered),
        )
        return await self.client().chat(
            model=self.settings.chat_pilot_model,
            messages=outgoing,
            # Chat sampling differs from deterministic extraction.
            options={
                "num_ctx": self.settings.ollama_num_ctx,
                "temperature": self.settings.chat_pilot_temperature,
            },
            keep_alive=-1,
            think=self.settings.chat_think,
            **({"tools": offered} if with_tools else {}),
        )

    def _budgeted_messages(
        self, messages: list[dict[str, Any]], offered: list[dict], role_overlay: str
    ) -> list[dict[str, Any]]:
        """Trim only prior history until the full request and reply reserve fit."""
        current_user = max(
            (index for index, item in enumerate(messages) if item.get("role") == "user"), default=1
        )
        schemas = json.dumps(offered, ensure_ascii=False, default=str, separators=(",", ":"))
        schema_tokens = estimate_tokens(schemas)
        tool_suffix = json.dumps(
            [item["tool_calls"] for item in messages if item.get("tool_calls")],
            ensure_ascii=False,
            default=str,
            separators=(",", ":"),
        )
        reminder = self.voice_reminder(role_overlay)
        while True:
            outgoing = [*messages, reminder]
            # Estimate the rendered turns, tool-call arguments, and schemas as
            # one request so rounding cannot reject a request by a fraction of a
            # token at the configured boundary.
            material = "\n\n".join(item["content"] for item in outgoing)
            request_tokens = estimate_tokens("\n\n".join((material, tool_suffix))) + schema_tokens
            total = request_tokens + COMPLETION_RESERVE_TOKENS
            if total <= self.settings.ollama_num_ctx:
                return outgoing
            if current_user <= 1:
                raise ContextBudgetError(
                    f"chat request estimate {total} exceeds context budget "
                    f"{self.settings.ollama_num_ctx} with completion reserve"
                )
            messages.pop(1)
            current_user -= 1

    @staticmethod
    def _tidy(content: str) -> str:
        """Normalize and bound a member-facing reply."""
        text = _tidy_blank_lines(content or "").strip()
        return unglue_first_bullet(text)[:1200].strip()

    # -- discord -----------------------------------------------------------
    async def _post(self, message: Any, content: str) -> Any:
        """Post a reply through the allow-listed plain-message path."""
        try:
            return await self.bot.post_plain(
                message.channel,
                content,
                [str(message.author.id)],
                reference_id=getattr(message, "id", None),
            )
        except Exception:  # noqa: BLE001 - a failed reply is not worth a crash
            log.exception("chat: could not post the reply")
            return None

    @staticmethod
    async def _react(message: Any, emoji: str) -> None:
        """Best-effort add a reaction."""
        add = getattr(message, "add_reaction", None)
        if add is None:
            return
        with contextlib.suppress(Exception):
            await add(emoji)

    async def _unreact(self, message: Any, emoji: str) -> None:
        """Best-effort remove the bot's reaction."""
        remove = getattr(message, "remove_reaction", None)
        if remove is None:
            return
        with contextlib.suppress(Exception):
            await remove(emoji, getattr(self.bot, "user", None))
