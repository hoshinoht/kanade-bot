"""OpenAI chat-completion request bodies and normalized replies."""

from __future__ import annotations

import json
from collections.abc import Sequence
from dataclasses import dataclass, field
from typing import Any

#: The only message keys the gateway accepts.
_MESSAGE_KEYS = ("role", "content", "tool_calls", "tool_call_id")


def arguments_text(arguments: Any) -> str:
    """Tool-call arguments as the JSON string the wire requires."""
    if isinstance(arguments, str):
        return arguments
    return json.dumps(arguments if arguments is not None else {}, ensure_ascii=False)


def tool_call(call_id: str, name: str, arguments: Any) -> dict[str, Any]:
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": arguments_text(arguments)},
    }


def _message(item: dict[str, Any]) -> dict[str, Any]:
    out = {key: item[key] for key in _MESSAGE_KEYS if item.get(key) is not None}
    if out.get("role") == "assistant" and "tool_calls" in out:
        calls = [
            tool_call(
                str(call.get("id") or ""),
                str(call.get("function", {}).get("name") or ""),
                call.get("function", {}).get("arguments"),
            )
            for call in out["tool_calls"]
        ]
        if calls:
            out["tool_calls"] = calls
        else:
            del out["tool_calls"]
    return out


def chat_body(
    *,
    model: str,
    messages: Sequence[dict[str, Any]],
    tools: Sequence[dict] | None = None,
    temperature: float | None = None,
    seed: int | None = None,
    reasoning_effort: str | None = None,
    response_format: dict | None = None,
) -> dict[str, Any]:
    """One request body, omitting every unset field rather than sending null."""
    body: dict[str, Any] = {"model": model, "messages": [_message(m) for m in messages]}
    optional = {
        "tools": list(tools) if tools else None,
        "response_format": response_format,
        "temperature": temperature,
        "seed": seed,
        "reasoning_effort": reasoning_effort,
    }
    body.update({key: value for key, value in optional.items() if value is not None})
    return body


def json_schema_format(name: str, schema: dict) -> dict[str, Any]:
    return {"type": "json_schema", "json_schema": {"name": name, "schema": schema, "strict": True}}


@dataclass
class ToolCall:
    id: str
    name: str
    #: The provider's JSON string, parsed later by the tool dispatcher.
    arguments: Any

    def wire(self) -> dict[str, Any]:
        return tool_call(self.id, self.name, self.arguments)


@dataclass
class Reply:
    """``choices[0]`` and ``usage`` of one completion, tolerating absent fields."""

    content: str | None = None
    reasoning: str | None = None
    tool_calls: list[ToolCall] = field(default_factory=list)
    finish_reason: str | None = None
    prompt_tokens: int | None = None
    completion_tokens: int | None = None


def _count(usage: Any, key: str) -> int | None:
    value = usage.get(key) if isinstance(usage, dict) else None
    try:
        return int(value) if value is not None else None
    except (TypeError, ValueError):
        return None


def _text(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def parse_reply(response: Any, id_prefix: str = "call", reserved: set[str] | None = None) -> Reply:
    """Normalize a completion; synthesize stable ids only where the provider omitted them.

    ``reserved`` holds ids already used in the conversation; a reused or missing
    id is replaced, and every id kept is added to it.
    """
    if not isinstance(response, dict):
        return Reply()
    choices = response.get("choices")
    choice = choices[0] if isinstance(choices, list) and choices else {}
    choice = choice if isinstance(choice, dict) else {}
    message = choice.get("message")
    message = message if isinstance(message, dict) else {}
    calls: list[ToolCall] = []
    seen: set[str] = reserved if reserved is not None else set()
    for index, raw in enumerate(message.get("tool_calls") or []):
        if not isinstance(raw, dict):
            continue
        function = raw.get("function")
        function = function if isinstance(function, dict) else {}
        call_id = raw.get("id")
        if not isinstance(call_id, str) or not call_id or call_id in seen:
            call_id = f"{id_prefix}_{index}"
            while call_id in seen:
                call_id += "_"
        seen.add(call_id)
        calls.append(ToolCall(call_id, str(function.get("name") or ""), function.get("arguments")))
    usage = response.get("usage")
    return Reply(
        content=_text(message.get("content")),
        reasoning=_text(message.get("reasoning")) or _text(message.get("reasoning_content")),
        tool_calls=calls,
        finish_reason=_text(choice.get("finish_reason")),
        prompt_tokens=_count(usage, "prompt_tokens"),
        completion_tokens=_count(usage, "completion_tokens"),
    )
