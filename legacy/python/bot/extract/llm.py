"""One guarded extraction call through the Kanata model gateway.

The gateway fronts one host model, so exactly one call runs at a time
(:data:`MODEL_LOCK`); keeping the weights resident is the host's business.

The lock is re-exported here rather than owned here: it is
:data:`bot.infrastructure.modellock.MODEL_LOCK`, and the chatbot takes the same one. An
extraction therefore waits for a chat answer exactly as it waits for another
extraction, which is the point -- the host has one model, not one per feature.

Nothing in here is allowed to take the bot down.  A model that is offline, slow,
or returning nonsense produces an :class:`ExtractionCall` with ``error`` set and
no amendments; the caller logs it and the schedule is simply not changed.
"""

from __future__ import annotations

import asyncio
import functools
import json
import logging
import re
import time
from dataclasses import dataclass, field
from typing import Any

from pydantic import ValidationError

from bot.infrastructure.config import Settings
from bot.infrastructure.llm import (
    Capabilities,
    KanataClient,
    ModelConfigError,
    chat_body,
    json_schema_format,
    parse_reply,
    profile_of,
)
from bot.infrastructure.modellock import EXTRACTOR, MODEL_LOCK, held

from .prompt import estimate_tokens
from .schema import Extraction, json_schema

log = logging.getLogger(__name__)

#: Sent back to the model when its first answer would not validate.
RETRY_INSTRUCTION = (
    "Your previous answer did not fit the schema:\n{error}\n"
    "Answer again with the same information in the required shape. "
    "Do not add anything the messages do not say."
)


#: Appended to the system prompt when the alias cannot constrain output itself.
JSON_INSTRUCTION = (
    "OUTPUT FORMAT\nAnswer with exactly one JSON object and nothing else: no prose, "
    "no markdown. It must validate against this JSON Schema:\n{schema}"
)


@functools.cache
def json_instruction() -> str:
    return JSON_INSTRUCTION.format(
        schema=json.dumps(json_schema(), ensure_ascii=False, separators=(",", ":"))
    )


@functools.cache
def json_instruction_tokens() -> int:
    """What appending :func:`json_instruction` adds to an estimated prompt."""
    return estimate_tokens(f"\n\n{json_instruction()}")


def _json_only(messages: list[dict[str, str]]) -> list[dict[str, str]]:
    """Put the schema into the prompt for aliases without structured output."""
    instruction = json_instruction()
    if messages and messages[0].get("role") == "system":
        first = {**messages[0], "content": f"{messages[0]['content']}\n\n{instruction}"}
        return [first, *messages[1:]]
    return [{"role": "system", "content": instruction}, *messages]


def extraction_body(
    model: str, messages: list[dict[str, str]], reasoning_effort: str, caps: Capabilities
) -> dict[str, Any]:
    """The request body ``caps`` allows; unsupported fields are omitted, never sent."""
    structured = caps.structured_output
    return chat_body(
        model=model,
        messages=messages if structured else _json_only(messages),
        response_format=json_schema_format("extraction", json_schema()) if structured else None,
        temperature=0 if caps.sampling_controls else None,
        seed=0 if caps.sampling_controls else None,
        reasoning_effort=caps.effort(reasoning_effort),
    )


@dataclass
class ExtractionCall:
    """The outcome of one model call -- always returned, never raised."""

    prompt: str = ""
    raw: str = ""
    latency_ms: int = 0
    extraction: Extraction | None = None
    error: str | None = None
    attempts: int = 0
    thinking: str = ""
    amendments: list = field(default_factory=list)
    #: A missing alias or key file, reported once rather than per burst.
    misconfigured: bool = False

    @property
    def ok(self) -> bool:
        return self.extraction is not None

    def __post_init__(self) -> None:
        if self.extraction is not None and not self.amendments:
            self.amendments = list(self.extraction.amendments)


#: One fence around the whole answer; anything else must be bare JSON.
_FENCE_RE = re.compile(r"\A```(?:json)?[ \t]*\n(?P<body>(?:(?!```).)*?)\n?```\Z", re.DOTALL)


def _client(settings: Settings) -> KanataClient:
    return KanataClient.from_settings(settings, settings.kanata_timeout)


def _text(response: Any) -> tuple[str, str]:
    """``(content, reasoning)`` from one completion."""
    reply = parse_reply(response)
    return (reply.content or "").strip(), (reply.reasoning or "").strip()


def parse_response(raw: str) -> Extraction:
    """Validate one raw model response.  Raises :class:`ValidationError`/``ValueError``."""
    if not raw:
        raise ValueError("the model returned an empty response")
    fenced = _FENCE_RE.match(raw.strip())
    try:
        data = json.loads(fenced.group("body") if fenced else raw)
    except json.JSONDecodeError as exc:
        # `response_format` makes this unlikely, but a truncated response is still JSON-ish.
        raise ValueError(f"not JSON: {exc}") from None
    if not isinstance(data, dict):
        raise ValueError(f"expected a JSON object, got {type(data).__name__}")
    return Extraction.model_validate(data)


class Extractor:
    """Calls the model, validates the answer, and never raises at the caller."""

    def __init__(self, settings: Settings, client: Any | None = None):
        self.settings = settings
        self._client = client
        self._own_client = client is None
        self._config_reported = False
        #: Last seen structured-output support per alias, for the prompt budget.
        self._structured: dict[str, bool] = {}

    def prompt_overhead_tokens(self) -> int:
        """Prompt tokens the call adds beyond the built messages.

        Aliases without structured output get the JSON schema in the prompt;
        until an alias has been seen, assume it does.
        """
        if self._structured.get(self.settings.extract_model, False):
            return 0
        return json_instruction_tokens()

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

    async def _chat(self, messages: list[dict[str, str]]) -> Any:
        model = self.settings.extract_model
        if not model:
            raise ModelConfigError("EXTRACT_MODEL is not set")
        client = self.client()
        caps = await profile_of(client, model)
        self._structured[model] = caps.structured_output
        log.debug(
            "extract: model %s reasoning=%s caps=%s",
            model,
            self.settings.reasoning_effort,
            caps,
        )
        body = extraction_body(model, messages, self.settings.reasoning_effort, caps)
        return await asyncio.wait_for(client.chat(**body), timeout=self.settings.kanata_timeout + 5)

    async def extract(self, messages: list[dict[str, str]]) -> ExtractionCall:
        """Run one extraction.

        Serialised guild-wide by :data:`MODEL_LOCK` -- against the chatbot's
        answers as well as against other extractions, since they all queue for
        the same resident model.
        """
        prompt = "\n\n".join(m["content"] for m in messages)
        started = time.monotonic()
        conversation = list(messages)
        raw = thinking = ""
        error: str | None = None
        attempts = 0
        misconfigured = False

        async with held(EXTRACTOR):
            for attempt in (1, 2):
                attempts = attempt
                try:
                    response = await self._chat(conversation)
                except TimeoutError:
                    error = f"the model did not answer within {self.settings.kanata_timeout:.0f}s"
                    break
                except ModelConfigError as exc:
                    error = f"{type(exc).__name__}: {exc}"
                    misconfigured = True
                    break
                except Exception as exc:  # noqa: BLE001 - the bot must survive anything here
                    error = f"{type(exc).__name__}: {exc}"
                    break

                raw, thinking = _text(response)
                try:
                    extraction = parse_response(raw)
                except (ValidationError, ValueError) as exc:
                    error = str(exc)
                    if attempt == 2:
                        break
                    log.warning("extraction did not validate; retrying once: %s", error)
                    # The gateway rejects an empty assistant turn.
                    previous = [{"role": "assistant", "content": raw}] if raw else []
                    conversation = [
                        *conversation,
                        *previous,
                        {"role": "user", "content": RETRY_INSTRUCTION.format(error=error)},
                    ]
                    continue
                return ExtractionCall(
                    prompt=prompt,
                    raw=raw,
                    latency_ms=int((time.monotonic() - started) * 1000),
                    extraction=extraction,
                    attempts=attempt,
                    thinking=thinking,
                )

        if misconfigured:
            # Every burst would fail the same way; say so loudly once.
            level = logging.DEBUG if self._config_reported else logging.ERROR
            self._config_reported = True
            log.log(level, "extraction cannot run until configured: %s", error)
        else:
            log.warning("extraction failed after %d attempt(s): %s", attempts, error)
        return ExtractionCall(
            misconfigured=misconfigured,
            prompt=prompt,
            raw=raw,
            latency_ms=int((time.monotonic() - started) * 1000),
            error=error,
            attempts=attempts,
            thinking=thinking,
        )


__all__ = ["MODEL_LOCK", "ExtractionCall", "Extractor", "parse_response"]
