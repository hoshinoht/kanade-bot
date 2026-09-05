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

from . import behaviour_plugins
from .agent.client import (
    CFG_CHAT,
    CFG_COUNTDOWNS,
    CFG_EXTRACT,
    CFG_PAUSED,
    CFG_PERSONA,
    CFG_PING_TIME,
    CFG_POOL_COUNT,
    CFG_POOL_WINDOW,
    CFG_QUIET,
    CFG_RATE_COUNT,
    CFG_RATE_WINDOW,
    BossBot,
)
from .domain.boss_knowledge import BossKnowledgeBase, BossKnowledgeError
from .domain.bosses import BossTable, BossTableError
from .infrastructure.config import Settings, get_settings
from .infrastructure.db import Repo

log = logging.getLogger("bot")

INITIAL_BACKOFF = 5.0
MAX_BACKOFF = 300.0
#: Fatal config errors still exit non-zero, but only after a pause: under
#: ``restart: unless-stopped`` an immediate exit becomes a tight loop that
#: hammers Discord's gateway with rejected   IDENTIFYs.
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
            # On when the chat role and channel are both set, because setting
            # them is what asking for the chatbot looks like. It still answers
            # nobody without them.
            CFG_CHAT: "1" if settings.chat_pilot_configured else "0",
            # The capacity numbers, seeded from the environment on first run and
            # editable from the portal afterwards. `seed_config` only inserts
            # what is missing, so a value tuned at 9pm is not undone by the next
            # restart reading `.env` again.
            # The voice, by filename, seeded from PERSONA_PATH's basename. From
            # then on the row wins, so a persona chosen from the portal is not
            # undone by the next restart reading `.env` again.
            CFG_PERSONA: Path(settings.persona_path).name,
            behaviour_plugins.CONFIG_KEY: behaviour_plugins.seed_value(settings.chat_role_plugins),
            CFG_RATE_COUNT: str(settings.chat_pilot_rate_count),
            CFG_RATE_WINDOW: str(settings.chat_pilot_rate_window_s),
            CFG_POOL_COUNT: str(settings.chat_pilot_global_rate_count),
            CFG_POOL_WINDOW: str(settings.chat_pilot_global_rate_window_s),
        }
    )
    return repo


def load_boss_resources(settings: Settings) -> tuple[BossTable, BossKnowledgeBase | None]:
    """Load the catalog and the chat pilot's strict local strategy guides.

    The scheduler always needs its catalog.  Strategy documents are deliberately
    required only when the chat pilot can answer, so scheduler-only deployments
    do not need to package guide content they cannot expose.
    """
    try:
        bosses = BossTable.load(settings.bosses_path)
    except (OSError, BossTableError) as exc:
        log.error(
            "cannot load boss catalog %s (knowledge path %s): %s",
            settings.bosses_path,
            settings.boss_knowledge_path,
            exc,
        )
        raise
    log.info("loaded %d boss aliases from %s", len(bosses.aliases), settings.bosses_path)

    if not settings.chat_pilot_configured:
        return bosses, None

    try:
        knowledge = BossKnowledgeBase.load(settings.boss_knowledge_path, bosses)
    except (OSError, BossKnowledgeError) as exc:
        log.error(
            "cannot load boss catalog %s with knowledge base %s: %s",
            settings.bosses_path,
            settings.boss_knowledge_path,
            exc,
        )
        raise
    log.info(
        "loaded %d boss strategy guides from %s",
        len(knowledge.documents),
        settings.boss_knowledge_path,
    )
    return bosses, knowledge


async def run() -> int:
    settings = get_settings()
    configure_logging(settings.log_level)
    os.environ.setdefault("TZ", settings.tz)

    stop = asyncio.Event()
    _install_signal_handlers(stop)

    try:
        bosses, boss_knowledge = load_boss_resources(settings)
    except (OSError, BossTableError, BossKnowledgeError):
        return await _fatal(2, stop)

    repo = build_repo(settings)
    log.info("database ready at %s", settings.db_path)

    backoff = INITIAL_BACKOFF
    try:
        while not stop.is_set():
            client = BossBot(settings, repo, bosses, boss_knowledge)
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
