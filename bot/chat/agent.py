"""The conversation loop: gate, assemble, call the model, post one reply.

Structured so that the interesting parts are testable without a gateway or a
model. :meth:`ChatPilot.offer` is the only entry point and does no reasoning of
its own -- it asks :mod:`bot.chat.gate` whether to answer, holds a per-channel
lock so one channel cannot have two answers in flight, and hands the assembled
conversation to :meth:`ChatPilot.generate`, which is the part with the model in
it.

Nothing here is allowed to take the bot down, and nothing here is allowed to be
slow forever: the whole answer, tool rounds included, lives inside one
``asyncio.wait_for``. A model that is offline, wedged, or looping produces a
short apology in the channel and a full traceback in the log.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
import time
from collections import deque
from dataclasses import dataclass, field
from typing import Any

from ..extract.prompt import estimate_messages
from ..timeutil import utcnow
from ..util import is_bot_admin
from ..watch import origin_ids
from ..weeks import current_week_start
from . import gate, persona, tools
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

#: Said in the channel when the model could not answer. Deliberately plain: an
#: in-persona apology written here would be a second, worse persona.
FAILURE_REPLY = "Sorry — I couldn't get to the schedule just now. Try me again in a bit."

__all__ = ["MAX_TOOL_ROUNDS", "ChatPilot", "ChatTurn"]


@dataclass
class ChatTurn:
    """One remembered line of a channel's conversation with the bot."""

    role: str
    content: str
    #: The Discord message id, so a reply chain and the history cannot both
    #: contribute the same line.
    message_id: str | None = None


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

    @property
    def trace(self) -> str:
        """``get_schedule:ok, propose_move:refused`` -- the interaction in one field."""
        return ", ".join(f"{o.name or '?'}:{o.outcome}" for o in self.outcomes)


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


def _brief(arguments: dict, limit: int = 200) -> str:
    """Tool arguments, short enough for one log line.

    Truncated rather than omitted: the arguments are the whole point of the
    trace, and a model that passes a paragraph where a run id belongs is exactly
    the thing somebody reading these logs is trying to see.
    """
    rendered = ", ".join(f"{key}={value!r}" for key, value in (arguments or {}).items())
    return rendered if len(rendered) <= limit else rendered[: limit - 1] + "…"


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
        #: Per channel, so two channels can be answered at once but one channel
        #: cannot queue up a backlog of 60-second generations.
        self._busy: set[str] = set()
        self._history: dict[str, deque[ChatTurn]] = {}
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

    # -- intake ------------------------------------------------------------
    async def offer(self, message: Any) -> Generation | None:
        """Called by ``on_message`` for every guild message. Answers, or does not.

        Returns ``None`` whenever nothing was said, which is the overwhelmingly
        common case: the gate refuses almost everything, and it refuses in
        silence.
        """
        decision = gate.decide(
            message,
            self.settings,
            bot_user_id=getattr(getattr(self.bot, "user", None), "id", None),
            enabled=self.enabled,
            is_admin=self._is_admin(getattr(message, "author", None)),
            limiter=self.limiter,
        )
        if not decision.act:
            if decision.busy:
                log.info("chat: %s from %s", decision.reason, getattr(message.author, "id", "?"))
                await self._react(message, gate.BUSY_REACTION)
            else:
                log.debug("chat: ignoring a message (%s)", decision.reason)
            return None

        channel_id = str(origin_ids(message.channel)[0])
        if channel_id in self._busy:
            # A second question while the first is still being answered. Queuing
            # it would mean a 60-second-old reply arriving after the asker has
            # given up, so it is dropped and they are told to wait.
            log.info("chat: channel %s is already answering; dropping", channel_id)
            await self._react(message, gate.BUSY_REACTION)
            return None

        self._busy.add(channel_id)
        try:
            return await self._answer(message, channel_id)
        finally:
            self._busy.discard(channel_id)

    def _is_admin(self, user: Any) -> bool:
        """The existing "who runs this bot" rule, used only for rate-limit exemption."""
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

    async def _answer(self, message: Any, channel_id: str) -> Generation:
        author_id = str(message.author.id)
        text = (message.content or "").strip()
        context = tools.ToolContext(
            bot=self.bot,
            author_id=author_id,
            channel_id=channel_id,
            message_id=str(message.id),
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
        return result

    # -- context assembly --------------------------------------------------
    def _speaker(self, user_id: str, text: str) -> str:
        """A line as the model sees it: who said it, then what they said.

        The name is looked up from the roster rather than taken from the
        message, so a display name the model is shown is one the bot already
        knows -- and a member cannot rename themselves into a instruction.
        """
        from ..api import service

        return f"{service.member_name(self.bot, user_id)}: {text}"

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
        now = utcnow()
        week = current_week_start(
            self.bot.tz, self.settings.reset_weekday, self.settings.reset_time, now
        )
        system = persona.system_prompt(
            self.persona_text(), persona.clock_header(now, self.bot.tz, week)
        )

        seen = {turn.message_id for turn in self.history(channel_id) if turn.message_id}
        earlier = list(self.history(channel_id))
        earlier += [turn for turn in self.reply_chain(message) if turn.message_id not in seen]
        question = ChatTurn(
            "user", self._speaker(str(message.author.id), (message.content or "").strip())
        )

        turns = [{"role": t.role, "content": t.content} for t in (*earlier, question)]
        # Measured over the conversation alone. The system prompt is a whole
        # persona document -- a few thousand tokens on its own -- so budgeting
        # the two together would leave a long conversation trimmed to nothing
        # and still over the line. What grows without bound is the deque, and
        # the deque is what this trims.
        while len(turns) > 1 and estimate_messages(turns) > CONVERSATION_BUDGET_TOKENS:
            turns.pop(0)
        return [{"role": "system", "content": system}, *turns]

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
            response = await self._chat(messages, with_tools=not last)
            content, calls = _message_text(response)
            model_ms = int((time.monotonic() - asked_at) * 1000)
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

    async def _chat(self, messages: list[dict[str, Any]], with_tools: bool) -> Any:
        return await self.client().chat(
            model=self.settings.chat_pilot_model,
            messages=messages,
            options={"num_ctx": self.settings.ollama_num_ctx},
            keep_alive=-1,
            think=self.settings.think,
            **({"tools": tools.TOOLS} if with_tools else {}),
        )

    @staticmethod
    def _tidy(content: str) -> str:
        """Trim the model's answer to something a channel can read.

        Discord's own limit is 2000 characters; this is far below it because a
        chatbot that writes an essay in a party channel is a worse chatbot, and
        the persona already asks for four sentences.
        """
        text = " ".join((content or "").split("\n\n")).strip()
        return text[:1200].strip()

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
        add = getattr(message, "add_reaction", None)
        if add is None:
            return
        with contextlib.suppress(Exception):
            await add(emoji)
