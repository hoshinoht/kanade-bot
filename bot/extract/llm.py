"""One guarded call to the local model.

``gpt-oss:20b`` is 13 GB and lives on the host's GPU, so exactly one call runs at
a time (:data:`MODEL_LOCK`) and ``keep_alive=-1`` keeps it resident between
bursts -- otherwise every call pays to reload the weights.

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
import json
import logging
import time
from dataclasses import dataclass, field
from typing import Any

from pydantic import ValidationError

from bot.infrastructure.config import Settings
from bot.infrastructure.modellock import EXTRACTOR, MODEL_LOCK, held

from .schema import Extraction, json_schema

log = logging.getLogger(__name__)

#: Sent back to the model when its first answer would not validate.
RETRY_INSTRUCTION = (
    "Your previous answer did not fit the schema:\n{error}\n"
    "Answer again with the same information in the required shape. "
    "Do not add anything the messages do not say."
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

    @property
    def ok(self) -> bool:
        return self.extraction is not None

    def __post_init__(self) -> None:
        if self.extraction is not None and not self.amendments:
            self.amendments = list(self.extraction.amendments)


def _client(settings: Settings, host: str | None = None):
    """Build an ``ollama.AsyncClient``.  Imported lazily so tests need no model."""
    from ollama import AsyncClient

    return AsyncClient(host=host or settings.ollama_host, timeout=settings.ollama_timeout)


def _text(response: Any) -> tuple[str, str]:
    """``(content, thinking)`` from a ChatResponse, tolerating a plain dict."""
    message = getattr(response, "message", None)
    if message is None and isinstance(response, dict):
        message = response.get("message")
    if message is None:
        return "", ""
    content = getattr(message, "content", None)
    thinking = getattr(message, "thinking", None)
    if isinstance(message, dict):
        content = message.get("content")
        thinking = message.get("thinking")
    return (content or "").strip(), (thinking or "").strip()


def parse_response(raw: str) -> Extraction:
    """Validate one raw model response.  Raises :class:`ValidationError`/``ValueError``."""
    if not raw:
        raise ValueError("the model returned an empty response")
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        # `format=` makes this unlikely, but a truncated response is still JSON-ish.
        raise ValueError(f"not JSON: {exc}") from None
    if not isinstance(data, dict):
        raise ValueError(f"expected a JSON object, got {type(data).__name__}")
    return Extraction.model_validate(data)


class Extractor:
    """Calls the model, validates the answer, and never raises at the caller."""

    def __init__(self, settings: Settings, client: Any | None = None, host: str | None = None):
        self.settings = settings
        self.host = host or settings.ollama_host
        self._client = client
        self._own_client = client is None

    def client(self) -> Any:
        if self._client is None:
            self._client = _client(self.settings, self.host)
        return self._client

    async def close(self) -> None:
        if self._client is not None and self._own_client:
            close = getattr(self._client, "close", None)
            if close is not None:
                await close()
            self._client = None

    async def _chat(self, messages: list[dict[str, str]]) -> Any:
        return await asyncio.wait_for(
            self.client().chat(
                model=self.settings.ollama_model,
                messages=messages,
                format=json_schema(),
                options={"temperature": 0, "seed": 0, "num_ctx": self.settings.ollama_num_ctx},
                keep_alive=-1,
                think=self.settings.think,
            ),
            timeout=self.settings.ollama_timeout + 5,
        )

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

        async with held(EXTRACTOR):
            for attempt in (1, 2):
                attempts = attempt
                try:
                    response = await self._chat(conversation)
                except TimeoutError:
                    error = f"the model did not answer within {self.settings.ollama_timeout:.0f}s"
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
                    conversation = [
                        *conversation,
                        {"role": "assistant", "content": raw},
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

        log.warning("extraction failed after %d attempt(s): %s", attempts, error)
        return ExtractionCall(
            prompt=prompt,
            raw=raw,
            latency_ms=int((time.monotonic() - started) * 1000),
            error=error,
            attempts=attempts,
            thinking=thinking,
        )


__all__ = ["MODEL_LOCK", "ExtractionCall", "Extractor", "parse_response"]
