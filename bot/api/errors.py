"""The one error type the API layer raises.

Handlers and the service layer raise :class:`ApiError`; :func:`bot.api.app.create_app`
installs the handler that turns it into ``{"error": "..."}`` for ``/api`` routes
and into a rendered page (or a redirect to ``/login``) for the portal.
"""

from __future__ import annotations


class ApiError(Exception):
    """An HTTP status plus a message meant for a human to read."""

    def __init__(self, status_code: int, message: str):
        self.status_code = status_code
        self.message = message
        super().__init__(message)


class NotConfigured(ApiError):
    """``ADMIN_TOKEN`` is empty, so nothing can be authenticated."""

    def __init__(self) -> None:
        super().__init__(
            503,
            "set ADMIN_TOKEN in .env and restart - the API refuses every request until it is "
            "set. Generate one with `openssl rand -hex 32`.",
        )


class Unauthorized(ApiError):
    def __init__(self, message: str = "not authenticated") -> None:
        super().__init__(401, message)


class NotFound(ApiError):
    def __init__(self, message: str = "not found") -> None:
        super().__init__(404, message)


class BadRequest(ApiError):
    def __init__(self, message: str) -> None:
        super().__init__(400, message)


__all__ = ["ApiError", "BadRequest", "NotConfigured", "NotFound", "Unauthorized"]
