"""The chatbot's closed tool surface and its stable compatibility facade.

The individual model-callable tools live in sibling modules; this facade owns
the public API and, deliberately, the mutable :data:`utcnow` seam.  Child
modules read that seam through :mod:`bot.chat.tools.clock` at call time so
existing ``tools.utcnow`` monkeypatches continue to control every tool.
"""

from __future__ import annotations

from bot.domain.timeutil import utcnow as utcnow

from .contracts import (
    FAILED,
    MAX_RUNS,
    READ_ONLY_TURN,
    REFUSED,
    UNKNOWN,
    ToolContext,
    ToolError,
    ToolOutcome,
)
from .contracts import (
    UNKNOWN_TOOL as UNKNOWN_TOOL,
)
from .dispatching import dispatch, is_write_tool, read_tools, run
from .resolution import resolve_fixed, resolve_run
from .schemas import TOOLS, tool_names

__all__ = [
    "FAILED",
    "MAX_RUNS",
    "READ_ONLY_TURN",
    "REFUSED",
    "TOOLS",
    "UNKNOWN",
    "ToolContext",
    "ToolError",
    "ToolOutcome",
    "dispatch",
    "is_write_tool",
    "read_tools",
    "resolve_fixed",
    "resolve_run",
    "run",
    "tool_names",
]
