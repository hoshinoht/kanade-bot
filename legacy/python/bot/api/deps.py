"""Shared FastAPI dependencies: the live bot, and who is calling.

Both are read off ``app.state`` rather than captured in a closure, so the same
router objects can be mounted onto a test app with a stand-in bot.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Annotated

from fastapi import Depends, Request

from .auth import Identity, authenticate

if TYPE_CHECKING:  # pragma: no cover
    from bot.agent.client import BossBot


def get_bot(request: Request) -> BossBot:
    return request.app.state.bot


def require_identity(request: Request) -> Identity:
    """Authenticate, or raise :class:`~bot.api.errors.ApiError`.

    Declared as a dependency rather than middleware so ``/healthz`` and
    ``/login`` can simply not depend on it, and so the resolved identity is
    available to handlers that want to show who is signed in.
    """
    return authenticate(request, get_bot(request).settings)


Bot = Annotated["BossBot", Depends(get_bot)]
Caller = Annotated[Identity, Depends(require_identity)]

__all__ = ["Bot", "Caller", "get_bot", "require_identity"]
