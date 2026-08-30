"""Running the API on the bot's own event loop.

DESIGN.md §5 is explicit that this is *one* process: a second one would fight
the bot for the SQLite file and could not post to Discord.  So uvicorn is driven
through :class:`uvicorn.Server` and awaited as a task next to discord.py's
gateway loop rather than through ``uvicorn.run``, which would try to own the
loop.

**Where it listens.** ``API_HOST`` defaults to ``127.0.0.1``, which is what you
want when the bot runs natively.  In the container compose sets it to
``0.0.0.0``: a container's loopback is its own namespace, so binding to it would
make the published port unreachable.  The "never leaves this machine" guarantee
comes from the compose mapping ``ports: ["127.0.0.1:8080:8080"]`` -- Docker only
accepts connections on the host's loopback and forwards them in.  Tailnet access
is `tailscale serve` on the host; see :mod:`bot.api.auth`.
"""

from __future__ import annotations

import asyncio
import logging
from typing import TYPE_CHECKING

import uvicorn

from .app import create_app

if TYPE_CHECKING:  # pragma: no cover
    from ..client import BossBot

log = logging.getLogger(__name__)

#: How long a `docker compose stop` waits for in-flight requests to finish.
GRACEFUL_SHUTDOWN_SECONDS = 5


class ApiServer:
    """Starts and stops uvicorn alongside the Discord client."""

    def __init__(self, bot: BossBot):
        self.bot = bot
        self.server: uvicorn.Server | None = None
        self._task: asyncio.Task | None = None

    @property
    def running(self) -> bool:
        return self._task is not None and not self._task.done()

    def _config(self) -> uvicorn.Config:
        return uvicorn.Config(
            create_app(self.bot),
            host=self.bot.settings.api_host,
            port=self.bot.settings.api_port,
            log_level="warning",
            access_log=False,
            # We decide for ourselves whether a peer is trustworthy (see
            # `bot.api.auth.is_local_peer`); letting uvicorn rewrite the client
            # address from X-Forwarded-For would hand that decision to a header.
            proxy_headers=False,
            timeout_graceful_shutdown=GRACEFUL_SHUTDOWN_SECONDS,
            # discord.py owns the loop; uvicorn must not install its own.
            lifespan="off",
        )

    async def start(self) -> None:
        """Bind the port and serve, as a task on the loop discord.py is already using."""
        if self.running:  # pragma: no cover - setup_hook runs once
            return
        settings = self.bot.settings
        self.server = uvicorn.Server(self._config())
        self._task = asyncio.create_task(self.server.serve(), name="boss-api")
        if not settings.admin_token:
            log.warning(
                "ADMIN_TOKEN is empty: the API is up on %s:%s but refuses every request "
                "except /healthz. Generate one with `openssl rand -hex 32`.",
                settings.api_host,
                settings.api_port,
            )
        else:
            log.info("portal + API listening on %s:%s", settings.api_host, settings.api_port)

    async def stop(self) -> None:
        """Ask uvicorn to finish in-flight requests, then wait for the task."""
        if self.server is not None:
            self.server.should_exit = True
        task, self._task = self._task, None
        if task is None:
            return
        try:
            await asyncio.wait_for(asyncio.shield(task), timeout=GRACEFUL_SHUTDOWN_SECONDS + 2)
        except (TimeoutError, asyncio.CancelledError):  # pragma: no cover - slow client
            task.cancel()
        except Exception:  # pragma: no cover - shutdown must never mask the real error
            log.exception("the API server did not shut down cleanly")
        finally:
            self.server = None


__all__ = ["GRACEFUL_SHUTDOWN_SECONDS", "ApiServer"]
