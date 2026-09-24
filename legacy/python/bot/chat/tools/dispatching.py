"""Canonical tool handler registries and the guarded dispatch boundary."""

from __future__ import annotations

import json
import logging
import time
from typing import Any

from ...api.errors import BadRequest, NotFound
from .contracts import (
    FAILED,
    READ_ONLY_TURN,
    REFUSED,
    UNKNOWN,
    UNKNOWN_TOOL,
    ToolContext,
    ToolError,
    ToolOutcome,
)
from .get_boss_strategy import handle as get_boss_strategy
from .get_pending import handle as get_pending
from .get_run import handle as get_run
from .get_schedule import handle as get_schedule
from .list_bosses import handle as list_bosses
from .list_fixed import handle as list_fixed
from .propose_add import handle as propose_add
from .propose_cancel import handle as propose_cancel
from .propose_change_fixed import handle as propose_change_fixed
from .propose_move import handle as propose_move
from .propose_remove_fixed import handle as propose_remove_fixed
from .propose_rsvp import handle as propose_rsvp
from .schemas import TOOLS, tool_names

log = logging.getLogger(__name__)

_READ = {
    "get_schedule": get_schedule,
    "get_run": get_run,
    "list_bosses": list_bosses,
    "get_boss_strategy": get_boss_strategy,
    "get_pending": get_pending,
    "list_fixed": list_fixed,
}

_WRITE = {
    "propose_add": propose_add,
    "propose_remove_fixed": propose_remove_fixed,
    "propose_change_fixed": propose_change_fixed,
    "propose_move": propose_move,
    "propose_cancel": propose_cancel,
    "propose_rsvp": propose_rsvp,
}


def is_write_tool(name: str) -> bool:
    """Whether ``name`` is one of the card-posting tools."""
    return name in _WRITE


def read_tools() -> list[dict]:
    """:data:`TOOLS` with the six ``propose_*`` schemas taken out."""
    return [tool for tool in TOOLS if tool["function"]["name"] in _READ]


def _arguments(raw: Any) -> dict:
    """The model's arguments as a dict, whatever shape the client handed back."""
    if isinstance(raw, dict):
        return raw
    if isinstance(raw, str):
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            return {}
        return parsed if isinstance(parsed, dict) else {}
    return {}


async def run(ctx: ToolContext, name: str, arguments: Any) -> ToolOutcome:
    """Run one tool call and describe what happened without ever raising."""
    started = time.monotonic()
    already = len(ctx.created)
    already_posted = len(ctx.posted)
    args = _arguments(arguments)

    def done(output: str, ok: bool = True, error: str | None = None) -> ToolOutcome:
        return ToolOutcome(
            name=name,
            output=output,
            arguments=args,
            ok=ok,
            error=error,
            duration_ms=int((time.monotonic() - started) * 1000),
            created=list(ctx.created[already:]),
            posted=list(ctx.posted[already_posted:]),
        )

    if ctx.read_only and name in _WRITE:
        # Structural, not advisory: the schemas were withheld from this turn
        # too, but a model that names one from memory must be refused by the
        # dispatcher rather than trusted not to try.
        log.info("chat: %s is not available in a read-only turn", name)
        return done(READ_ONLY_TURN, False, REFUSED)

    handler = _READ.get(name) or _WRITE.get(name)
    if handler is None:
        log.warning("chat: the model asked for an unknown tool %r", name)
        return done(UNKNOWN_TOOL.format(name=name, known=", ".join(tool_names())), False, UNKNOWN)
    try:
        output = await handler(ctx, args) if name in _WRITE else handler(ctx, args)
        log.debug("chat: %s response %r", name, output)
        return done(output)
    except ToolError as exc:
        log.info("chat: %s refused: %s", name, exc)
        return done(str(exc), False, REFUSED)
    except (BadRequest, NotFound) as exc:
        log.info("chat: %s rejected by the service layer: %s", name, exc.message)
        return done(str(exc.message), False, REFUSED)
    except Exception:  # noqa: BLE001 - a tool must never take the answer down
        log.exception("chat: %s failed", name)
        return done(
            "That lookup failed. Say you could not complete that request just now.", False, FAILED
        )


async def dispatch(ctx: ToolContext, name: str, arguments: Any) -> str:
    """Return just the text the model reads, for callers that do not log."""
    return (await run(ctx, name, arguments)).output
