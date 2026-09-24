"""Kanata gateway boundary: HTTP client, OpenAI wire translation, and errors."""

from .capabilities import FULL, MINIMAL, TRUST_ZONES, Capabilities, profile_of
from .catalog import model_catalog
from .client import KanataClient, key_file_error
from .errors import (
    ModelAuthError,
    ModelConfigError,
    ModelError,
    ModelFieldRejected,
    ModelResponseError,
    ModelTimeout,
    ModelUnavailable,
)
from .probe import gateway_status
from .wire import Reply, ToolCall, chat_body, json_schema_format, parse_reply, tool_call

__all__ = [
    "FULL",
    "MINIMAL",
    "TRUST_ZONES",
    "Capabilities",
    "KanataClient",
    "ModelAuthError",
    "ModelConfigError",
    "ModelError",
    "ModelFieldRejected",
    "ModelResponseError",
    "ModelTimeout",
    "ModelUnavailable",
    "Reply",
    "ToolCall",
    "chat_body",
    "gateway_status",
    "json_schema_format",
    "key_file_error",
    "model_catalog",
    "parse_reply",
    "profile_of",
    "tool_call",
]
