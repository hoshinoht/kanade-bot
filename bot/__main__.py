"""Entrypoint: ``python -m bot``.

Retries the Discord login with exponential backoff so the container survives
coming up before the network (or Docker/Ollama) is ready.  A bad token is fatal
immediately -- retrying it would only get the token flagged.
"""

from __future__ import annotations

import asyncio
import logging
import os
import signal
import sys
from pathlib import Path

import discord

from .bosses import BossTable, BossTableError
from .client import (
    CFG_COUNTDOWNS,
    CFG_EXTRACT,
    CFG_PAUSED,
    CFG_PING_TIME,
    CFG_QUIET,
    BossBot,
)
from .config import Settings, get_settings
from .db import Repo

log = logging.getLogger("bot")

INITIAL_BACKOFF = 5.0
MAX_BACKOFF = 300.0
#: Fatal config errors still exit non-zero, but only after a pause: under
#: ``restart: unless-stopped`` an immediate exit becomes a tight loop that
#: hammers Discord's gateway with rejected IDENTIFYs.
FATAL_EXIT_DELAY = 60.0


async def _fatal(code: int, stop: asyncio.Event | None = None, delay: float | None = None) -> int:
    """Pause before exiting on an unrecoverable config error.

    ``stop`` is set by SIGINT/SIGTERM, so `docker compose stop` on a
    misconfigured container still shuts down at once instead of sitting out the
    delay and being SIGKILLed.
    """
    delay = FATAL_EXIT_DELAY if delay is None else delay
    log.error("fatal configuration error; fix it and restart (exiting in %.0fs)", delay)
    if stop is None:
        await asyncio.sleep(delay)
        return code
    try:
        await asyncio.wait_for(stop.wait(), timeout=delay)
        log.info("shutdown requested; exiting now")
    except TimeoutError:
        pass
    return code


def _install_signal_handlers(stop: asyncio.Event) -> None:
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, stop.set)
        except NotImplementedError:  # pragma: no cover - non-unix
            pass


def configure_logging(level: str) -> None:
    logging.basicConfig(
        level=getattr(logging, level.upper(), logging.INFO),
        format="%(asctime)s %(levelname)-8s %(name)s: %(message)s",
        stream=sys.stdout,
    )
    logging.getLogger("discord").setLevel(logging.WARNING)


def build_repo(settings: Settings) -> Repo:
    Path(settings.db_path).parent.mkdir(parents=True, exist_ok=True)
    repo = Repo(settings.db_path)
    repo.seed_config(
        {
            CFG_PING_TIME: settings.day_of_ping_time,
            CFG_COUNTDOWNS: settings.countdown_minutes,
            CFG_PAUSED: "0",
            CFG_EXTRACT: "1" if settings.extract_enabled else "0",
            # Off by default: a fresh deployment is meant to ping people.
            CFG_QUIET: "0",
        }
    )
    return repo


async def run() -> int:
    settings = get_settings()
    configure_logging(settings.log_level)
    os.environ.setdefault("TZ", settings.tz)

    stop = asyncio.Event()
    _install_signal_handlers(stop)

    try:
        bosses = BossTable.load(settings.bosses_path)
    except (OSError, BossTableError) as exc:
        log.error("cannot load %s: %s", settings.bosses_path, exc)
        return await _fatal(2, stop)
    log.info("loaded %d boss aliases from %s", len(bosses.aliases), settings.bosses_path)

    repo = build_repo(settings)
    log.info("database ready at %s", settings.db_path)

    backoff = INITIAL_BACKOFF
    try:
        while not stop.is_set():
            client = BossBot(settings, repo, bosses)
            try:
                start = asyncio.create_task(client.start(settings.discord_token))
                waiter = asyncio.create_task(stop.wait())
                done, _ = await asyncio.wait({start, waiter}, return_when=asyncio.FIRST_COMPLETED)
                if waiter in done:
                    log.info("shutdown requested")
                    start.cancel()
                    await client.close()
                    return 0
                waiter.cancel()
                await start  # re-raise whatever ended the connection
                backoff = INITIAL_BACKOFF
            except discord.LoginFailure:
                log.error("Discord rejected DISCORD_TOKEN - fix .env and restart")
                return await _fatal(3, stop)
            except discord.PrivilegedIntentsRequired:
                log.error(
                    "Message Content and/or Server Members intent is not enabled for this "
                    "bot in the Discord developer portal - see README.md"
                )
                return await _fatal(4, stop)
            except asyncio.CancelledError:
                raise
            except Exception:
                log.exception("connection lost; reconnecting in %.0fs", backoff)
            finally:
                if not client.is_closed():
                    await client.close()
            if stop.is_set():
                break
            try:
                await asyncio.wait_for(stop.wait(), timeout=backoff)
                break
            except TimeoutError:
                backoff = min(backoff * 2, MAX_BACKOFF)
    finally:
        repo.close()
    return 0


def main() -> int:
    try:
        return asyncio.run(run())
    except KeyboardInterrupt:  # pragma: no cover
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
