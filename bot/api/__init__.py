"""The in-process HTTP API, the web portal, and what ``bossctl`` talks to.

DESIGN.md §5: one API, two front-ends.  It runs on the *same* asyncio loop as
discord.py (see :class:`bot.api.server.ApiServer`) so a portal action can read
live state, write to the same SQLite connection the bot uses, and post to
Discord -- without a second process fighting over the database file.

Nothing here is reachable from outside the machine: compose publishes the port
as ``127.0.0.1:8080:8080`` and the tailnet route is `tailscale serve` on the
host.  See :mod:`bot.api.auth` for what is required of a request.
"""

from .app import create_app
from .errors import ApiError
from .server import ApiServer

__all__ = ["ApiError", "ApiServer", "create_app"]
