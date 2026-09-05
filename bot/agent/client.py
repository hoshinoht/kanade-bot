"""The Discord client: roster sync, the reminder tick loop, and reaction RSVPs.

Scheduling deliberately avoids APScheduler (see DESIGN.md §3).  The ``reminders``
table is the source of truth and a :class:`discord.ext.tasks.Loop` ticks every
``TICK_SECONDS``, sending anything due and stamping ``sent_at``.  Nothing lives
in memory, so a container restart neither loses nor replays a reminder.
"""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass
from datetime import datetime, time, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

import discord
from discord.ext import tasks

from bot.api.server import ApiServer
from bot.chat import ChatPilot, persona
from bot.domain.boss_knowledge import BossKnowledgeBase
from bot.domain.bosses import BossTable
from bot.domain.timeutil import to_iso, utcnow
from bot.domain.weeks import current_week_start, next_week_start, parse_hhmm
from bot.extract.commit import CommitResult, commit, expire_stale, may_commit, reject
from bot.extract.pipeline import Pipeline
from bot.infrastructure import audit, backup, identity
from bot.infrastructure.backfill import AccessDenied, record_channel
from bot.infrastructure.config import Settings
from bot.infrastructure.db import Repo
from bot.infrastructure.watch import is_watched, origin_ids

from . import formatting
from .debug import TEST_PREFIX
from .materialise import (
    DAY_OF,
    countdown_minutes,
    is_past,
    is_stale,
    mark_done,
    materialise_week,
    reconcile_day_of,
)
from .pings import audience
from .rescan import RescanWorker
from .rsvp import EMOJI_NO, EMOJI_YES, apply_reaction
from .util import positive_float, positive_int, roster_rows

log = logging.getLogger(__name__)

#: How many times a post is attempted when the network, rather than Discord,
#: is what failed -- and how long the first wait between attempts is, doubling.
POST_ATTEMPTS = 3
POST_BACKOFF_SECONDS = 1.0

#: What the embed's *image* attachment is called on the wire. A card can carry
#: two pictures of the same boss -- `boss/portraits/Star.png` in the corner
#: and `boss/artwork/entry/Star.png` along the bottom -- and on disk those are
#: both `Star.png`. Two attachments with one name make `attachment://Star.png`
#: ambiguous, and Discord resolves it to whichever it likes, which is how a
#: 550px splash ends up in the thumbnail slot.
#:
#: Only the newcomer is renamed, deliberately. `edit_card` rewrites an embed
#: without re-uploading anything, so every card already posted still has an
#: attachment called `Star.png`; prefixing the thumbnail too would point every
#: future edit at a filename those messages do not have, and break the portrait
#: on all of them.
IMAGE_PREFIX = "image-"


@dataclass(frozen=True)
class ChannelLookup:
    """Somewhere to post, or the reason there is nowhere."""

    channel: discord.abc.Messageable | None
    problem: str | None

    @property
    def ok(self) -> bool:
        return self.channel is not None


CFG_PING_TIME = "day_of_ping_time"
CFG_COUNTDOWNS = "countdown_minutes"
CFG_PAUSED = "paused"
CFG_EXTRACT = "extract_enabled"
CFG_QUIET = "quiet_mode"
CFG_CHAT = "chat_mode"
#: Which file in ``config/personas/`` the bot is wearing, by name. A runtime row for
#: the same reason :data:`CFG_QUIET` is one: a voice is the thing an operator
#: most wants to change without a redeploy, and the environment only seeds it.
CFG_PERSONA = "persona"
#: The chatbot's four capacity numbers. Runtime rows rather than environment
#: variables for the same reason :data:`CFG_QUIET` is one: they are what an
#: operator reaches for while the guild is busy, and "edit `.env` and restart"
#: is the wrong answer at that moment. The environment seeds them and the row
#: wins from then on.
CFG_RATE_COUNT = "chat_pilot_rate_count"
CFG_RATE_WINDOW = "chat_pilot_rate_window_s"
CFG_POOL_COUNT = "chat_pilot_global_rate_count"
CFG_POOL_WINDOW = "chat_pilot_global_rate_window_s"
CFG_LAST_WEEK = "last_materialised_week"
#: The boss week whose reset digest has already gone out, and the local day the
#: database was last snapshotted. Both are "have I done this yet?" markers rather
#: than schedules: the host is a laptop that sleeps, so every periodic job is
#: driven by comparing state to the clock on each tick, never by firing at an
#: instant that may pass while the machine is off.
CFG_LAST_DIGEST = "last_digest_week"
CFG_LAST_BACKUP = "last_backup_day"

#: A person's "can't make it" notice is posted at most this often per run, so a
#: ❌ toggled on and off (or mashed) never floods the channel.
DECLINE_NOTICE_COOLDOWN = timedelta(hours=6)


class BossBot(discord.Client):
    """discord.py client plus an application-command tree."""

    def __init__(
        self,
        settings: Settings,
        repo: Repo,
        bosses: BossTable,
        boss_knowledge: BossKnowledgeBase | None = None,
    ):
        intents = discord.Intents.default()
        intents.members = True  # roster sync from the bossing role
        intents.message_content = True  # phase 2 extractor reads chat
        intents.guilds = True
        intents.reactions = True
        super().__init__(intents=intents)

        self.settings = settings
        self.repo = repo
        self.bosses = bosses
        self.boss_knowledge = boss_knowledge
        self.tz: ZoneInfo = settings.zoneinfo
        self.guild_object = discord.Object(id=settings.guild_id)
        self.tree = discord.app_commands.CommandTree(self)
        self._synced = False
        self._warned_manage_messages = False
        #: The day a backup failed on, so a broken backups directory is logged
        #: once rather than every 30 s until midnight. Deliberately in memory:
        #: a restart is a good moment to try again.
        self._backup_failed_on: str | None = None
        #: Runs whose posted cards no longer match the database, and the task
        #: draining them. Every write that a card displays goes through
        #: :attr:`bot.infrastructure.db.Repo.on_run_changed`, so no caller has to remember.
        self._stale_cards: set[str] = set()
        self._card_refresh: asyncio.Task | None = None
        repo.on_run_changed = self.card_needs_refresh
        # The chat extractor: buffers each channel's messages and runs one model
        # call per burst. Constructing it does not touch Ollama.
        self.extractor = Pipeline(self)
        # The speech pilot: answers when mentioned by somebody holding the chat
        # role, in a channel listed in CHAT_PILOT_CHANNEL_IDS. Gated entirely at
        # its own door (`bot.chat.gate`), so it sees every message and answers
        # almost none. Constructing it reads no persona file and opens no client.
        self.chat = ChatPilot(self)
        # The portal/CLI HTTP API, served on this same loop (DESIGN.md §5) so it
        # reads live state and drives Discord without a second process fighting
        # over SQLite. Constructing the bot does not bind the port.
        self.api = ApiServer(self)
        # Re-reading a channel is minutes of model time, so requests are queued
        # and drained by one task rather than blocking whoever asked.
        self.rescans = RescanWorker(self)
        self.tick.change_interval(seconds=settings.tick_seconds)

    # -- runtime config (DB-backed, seeded from env on first run) ----------
    @property
    def ping_time(self) -> time:
        return parse_hhmm(self.repo.get_config(CFG_PING_TIME, self.settings.day_of_ping_time))

    @property
    def countdowns(self) -> list[int]:
        raw = self.repo.get_config(CFG_COUNTDOWNS, self.settings.countdown_minutes) or ""
        return sorted({int(p) for p in raw.replace(";", ",").split(",") if p.strip()}, reverse=True)

    @property
    def paused(self) -> bool:
        return (self.repo.get_config(CFG_PAUSED, "0") or "0") == "1"

    @property
    def extract_enabled(self) -> bool:
        """Runtime master switch for reading chat, seeded from ``EXTRACT_ENABLED``.

        Kept in the ``config`` table rather than read from the environment so
        the portal and ``bossctl config set`` can turn the model off without a
        redeploy, and the choice survives a restart.
        """
        default = "1" if self.settings.extract_enabled else "0"
        return (self.repo.get_config(CFG_EXTRACT, default) or default) == "1"

    @property
    def quiet_mode(self) -> bool:
        """Post everything, notify nobody.

        For developing against a live guild: the bot keeps saying exactly what
        it would say, but every message goes out with an empty mention
        allow-list, so a week of testing never puts a red badge on anyone's
        Discord. Enforced in :meth:`_prepared` (which every card goes through,
        sent or edited) and :meth:`post_plain`, the only two places the bot
        builds a non-empty one.
        """
        return (self.repo.get_config(CFG_QUIET, "0") or "0") == "1"

    @property
    def chat_mode(self) -> bool:
        """Runtime kill switch for the chatbot, seeded from whether it is configured.

        Turning it off stops the bot answering without touching ``.env`` -- the
        same reason :attr:`extract_enabled` is a config row rather than an
        environment variable. It is the *third* gate: with no chat role or no
        chat channel the pilot is off whatever this says, so a deployment that
        never configured it cannot be switched on by accident.
        """
        default = "1" if self.settings.chat_pilot_configured else "0"
        return (self.repo.get_config(CFG_CHAT, default) or default) == "1"

    @property
    def persona_name(self) -> str:
        """The persona file this deployment has chosen, by name, or ``""``.

        A name and never a path: what it points at is resolved against the real
        directory listing (:func:`bot.chat.persona.chosen_path`), so a row
        hand-edited into something with a slash in it selects nothing rather
        than reaching anywhere.
        """
        return self.repo.get_config(CFG_PERSONA, "") or ""

    @staticmethod
    def persona_choices() -> list[str]:
        """The personas on offer, read off the bind mount every time it is asked.

        On the client rather than in `bot.api.service` because that module is
        imported *by* the chat package (`bot.chat.tools` dispatches over it), so
        reaching the other way would close a circle.
        """
        return persona.available()

    @property
    def chat_rate_count(self) -> int:
        """Answers per person per window, seeded from ``CHAT_PILOT_RATE_COUNT``."""
        return positive_int(
            self.repo.get_config(CFG_RATE_COUNT), self.settings.chat_pilot_rate_count
        )

    @property
    def chat_rate_window_s(self) -> float:
        return positive_float(
            self.repo.get_config(CFG_RATE_WINDOW), self.settings.chat_pilot_rate_window_s
        )

    @property
    def chat_pool_count(self) -> int:
        """Answers per window across everybody, seeded from the environment."""
        return positive_int(
            self.repo.get_config(CFG_POOL_COUNT), self.settings.chat_pilot_global_rate_count
        )

    @property
    def chat_pool_window_s(self) -> float:
        return positive_float(
            self.repo.get_config(CFG_POOL_WINDOW), self.settings.chat_pilot_global_rate_window_s
        )

    @property
    def portal_actor_id(self) -> str:
        """Who a portal-driven change is attributed to.

        ``PORTAL_ACTOR_ID`` when set, otherwise the guild owner -- the one
        account that is always allowed to confirm anything (see
        :func:`bot.extract.commit.may_commit`).
        """
        if self.settings.portal_actor_id is not None:
            return str(self.settings.portal_actor_id)
        guild = self.get_guild(self.settings.guild_id)
        owner_id = getattr(guild, "owner_id", None)
        return str(owner_id) if owner_id else "0"

    def is_admin(self, user: discord.abc.User) -> bool:
        role_id = self.settings.admin_role_id
        if role_id is None:
            return False
        roles = getattr(user, "roles", [])
        return any(role.id == role_id for role in roles)

    def is_watched(self, channel: object) -> bool:
        """The single gate for chat handling: explicit channel, category, or thread parent."""
        return is_watched(
            channel,
            self.settings.chat_channel_id_list,
            self.settings.chat_category_id_list,
        )

    def has_bossing_role(self, user: discord.abc.User) -> bool:
        roles = getattr(user, "roles", None)
        if roles is not None:
            return any(role.id == self.settings.bossing_role_id for role in roles)
        return self.repo.has_role(user.id)

    # -- lifecycle --------------------------------------------------------
    async def setup_hook(self) -> None:
        from .commands import register_commands

        register_commands(self)
        # Guild-scoped commands appear immediately instead of the ~1h global TTL.
        self.tree.copy_global_to(guild=self.guild_object)
        await self.tree.sync(guild=self.guild_object)
        self._synced = True
        log.info("synced application commands to guild %s", self.settings.guild_id)
        self.tick.start()
        await self.rescans.start()
        await self.api.start()

    async def close(self) -> None:
        if self.tick.is_running():
            self.tick.cancel()
        await self.api.stop()
        await self.rescans.stop()
        await self.extractor.shutdown()
        await self.chat.close()
        await super().close()

    async def on_ready(self) -> None:
        log.info("logged in as %s (%s)", self.user, getattr(self.user, "id", "?"))
        # Which role runs the bot, at INFO: `/say` and `/debug` turn on this one
        # id, and "it says I'm not an admin" is otherwise a guessing game
        # between a wrong id, a missing role and Discord's own permissions.
        admin_role = self.settings.admin_role_id
        log.info(
            "admin role for /say and /debug: %s",
            admin_role
            if admin_role is not None
            else "unset (ADMIN_ROLE_ID) - server administrators and the owner only",
        )
        await self.sync_roster()
        self.materialise_weeks()
        await self.cache_identity()
        if self.settings.backfill_on_start:
            await self.backfill_all()

    async def cache_identity(self) -> None:
        """Keep the portal a copy of the bot's own avatar and banner.

        Purely cosmetic (:mod:`bot.infrastructure.identity`), so it is placed after the roster
        and the week -- the two things a start actually owes the guild -- and
        nothing it does is allowed to escape. A failure leaves whatever was
        cached last time in place, so the sign-in page keeps its artwork through
        an outage.
        """
        try:
            written = await identity.refresh(self)
        except Exception:
            log.debug("could not cache the bot's identity art", exc_info=True)
            return
        if written:
            log.info("cached the bot's %s", " and ".join(name[:-4] for name in written))

    # -- history ----------------------------------------------------------
    def watched_text_channels(self) -> list[discord.abc.GuildChannel]:
        """Every text channel the bot watches, resolved from the live guild.

        Categories are expanded here rather than from config, so a channel added
        to a watched category is picked up without a restart -- the same rule
        :func:`bot.infrastructure.watch.is_watched` applies per message.
        """
        guild = self.get_guild(self.settings.guild_id)
        if guild is None:
            return []
        return [c for c in getattr(guild, "text_channels", []) if self.is_watched(c)]

    def resolve_channel(self, channel_id: int | str) -> discord.abc.GuildChannel | None:
        """A channel by id, with **no** ``POST_CHANNEL_ID`` fallback.

        Distinct from :meth:`post_channel`, whose fallback is right for posting
        and wrong for reading: backfilling "the channel we could not find" from
        the digest channel would file one party's chat under another's.
        """
        try:
            return self.get_channel(int(channel_id))
        except (TypeError, ValueError):  # pragma: no cover - defensive
            return None

    async def backfill(
        self, channel: object, since: datetime, until: datetime | None = None
    ) -> int:
        """Pull one watched channel's history into ``messages``; returns the count.

        Reads only -- no model call, no card, no reaction. Idempotent, because
        ``record_message`` ignores ids it already has, so running it on every
        start costs one paginated read and writes nothing new.
        """
        if not self.is_watched(channel):
            log.debug("refusing to backfill unwatched channel %s", getattr(channel, "id", "?"))
            return 0
        try:
            return await record_channel(self.repo, channel, since, until)
        except AccessDenied:
            log.warning("no access to #%s's history; skipping", getattr(channel, "name", "?"))
            return 0
        except discord.HTTPException:
            log.exception("backfill of #%s failed", getattr(channel, "name", "?"))
            return 0

    async def backfill_channel(
        self, channel_id: int | str, since: datetime, until: datetime | None = None
    ) -> int:
        """:meth:`backfill` by id -- what the extractor's rescan calls."""
        channel = self.resolve_channel(channel_id)
        if channel is None:
            log.warning("channel %s is not visible; nothing to backfill", channel_id)
            return 0
        return await self.backfill(channel, since, until)

    async def backfill_all(self, since: datetime | None = None) -> int:
        """Sweep every watched channel for the current boss week, one at a time.

        Sequential on purpose: discord.py handles rate limits, and fanning a
        category of channels out concurrently only makes it throttle harder.
        """
        since = since or current_week_start(
            self.tz, self.settings.reset_weekday, self.settings.reset_time
        )
        channels = self.watched_text_channels()
        if not channels:
            log.info("no watched channels visible; nothing to backfill")
            return 0
        total = 0
        for channel in channels:
            count = await self.backfill(channel, since)
            total += count
            log.info("backfilled #%s: %d message(s) since %s", channel.name, count, since.date())
        return total

    # -- roster -----------------------------------------------------------
    async def sync_roster(self) -> None:
        guild = self.get_guild(self.settings.guild_id)
        if guild is None:
            log.error("guild %s not visible to the bot", self.settings.guild_id)
            return
        if not guild.chunked:
            try:
                await guild.chunk()
            except discord.ClientException:
                log.warning("could not chunk members; is the Server Members intent enabled?")
        role = guild.get_role(self.settings.bossing_role_id)
        if role is None:
            log.error("bossing role %s not found in guild", self.settings.bossing_role_id)
            return
        rows = roster_rows(role.members)
        self.repo.sync_roster(rows)
        skipped = len(role.members) - len(rows)
        log.info(
            "roster synced: %d members with @%s%s",
            len(rows),
            role.name,
            f" ({skipped} bot account(s) skipped)" if skipped else "",
        )

    async def on_member_update(self, before: discord.Member, after: discord.Member) -> None:
        self._track_member(after)

    async def on_member_join(self, member: discord.Member) -> None:
        self._track_member(member)

    def _track_member(self, member: discord.Member) -> None:
        if member.guild.id != self.settings.guild_id or member.bot:
            return
        has_role = any(r.id == self.settings.bossing_role_id for r in member.roles)
        self.repo.upsert_member(member.id, member.display_name, member.nick, has_role)

    # -- chat ---------------------------------------------------------------
    async def on_message(self, message: discord.Message) -> None:
        """Store watched messages, then offer them to the chatbot and the extractor.

        Storing happens whether or not extraction is enabled, and whoever ends up
        acting on the message, so `/rescan` can always look back over history
        that was captured while it was paused.

        **The chatbot goes first, and a message it handles is not offered to the
        extractor.** The two are gated on different lists, but those lists may
        overlap -- live, the pilot's channel turned out to sit under a category
        in ``CHAT_CATEGORY_IDS``, and one "@bot move hstar to wednesday" got both
        a chat reply *and* an extractor proposal card for the same sentence. A
        message addressed to the bot is a conversation, not ambient party chat:
        the pilot acts on it through its tools, and the extractor has no business
        also reading it over the asker's shoulder.

        "Handled" is the pilot's own verdict (:class:`bot.chat.agent.Handling`),
        decided once inside :meth:`~bot.chat.agent.ChatPilot.offer`. It must not
        be recomputed here: the gate consulted the rate limiter to reach it, and
        asking twice would spend two of somebody's four answers on one message.
        Every other refusal -- not a chat channel, no mention, no role, chat off
        -- leaves the extractor's behaviour exactly as it was.

        Neither offer is allowed to break the other, or the bot. A chat pilot
        that throws is treated as "not handled", so the extractor still runs.
        """
        if message.author.bot or message.guild is None:
            return
        if message.guild.id != self.settings.guild_id:
            return
        watched = self.is_watched(message.channel)
        if watched:
            # Store under the parent channel so a thread's messages group with
            # its channel's -- `python -m bot.export` writes the same id.
            channel_id, _thread_id = origin_ids(message.channel)
            self.repo.record_message(
                message.id,
                channel_id,
                message.author.id,
                message.created_at,
                message.content,
            )

        handled = False
        try:
            handled = (await self.chat.offer(message)).handled
        except Exception:  # pragma: no cover - chat must never break the bot
            log.exception("the chat pilot rejected a message")

        if watched and not handled:
            try:
                await self.extractor.offer(message)
            except Exception:  # pragma: no cover - chat must never break the bot
                log.exception("extractor rejected a message")

    # -- materialisation --------------------------------------------------
    def materialise_weeks(self) -> None:
        """Materialise the current and next boss week; safe to call repeatedly."""
        now = utcnow()
        ping_time, countdowns = self.ping_time, self.countdowns
        for week in (
            current_week_start(self.tz, self.settings.reset_weekday, self.settings.reset_time, now),
            next_week_start(self.tz, self.settings.reset_weekday, self.settings.reset_time, now),
        ):
            created = materialise_week(self.repo, week, self.tz, ping_time, countdowns, now=now)
            if created:
                log.info("materialised %d run(s) for week starting %s", len(created), week)
        reconciled = reconcile_day_of(self.repo, self.tz, ping_time, now=now)
        if reconciled:
            log.info(
                "reconciled %d day-of reminder(s) at %s",
                reconciled,
                ping_time.strftime("%H:%M"),
            )
        self.repo.set_config(
            CFG_LAST_WEEK,
            to_iso(
                current_week_start(
                    self.tz, self.settings.reset_weekday, self.settings.reset_time, now
                )
            ),
        )

    def _week_rolled_over(self, now: datetime) -> bool:
        current = to_iso(
            current_week_start(self.tz, self.settings.reset_weekday, self.settings.reset_time, now)
        )
        return self.repo.get_config(CFG_LAST_WEEK) != current

    # -- the tick ---------------------------------------------------------
    @tasks.loop(seconds=30)
    async def tick(self) -> None:
        now = utcnow()
        try:
            self.repo.heartbeat(now)
            if self._week_rolled_over(now):
                log.info("boss week rolled over; materialising")
                self.materialise_weeks()
            # Retire runs whose night has been and gone, before anything else
            # looks at them: a `done` run gets no pings and shows in nobody's
            # schedule or dropdown.
            mark_done(self.repo, now)
            # After `mark_done`, so the week the guild reads has last week's
            # leftovers already retired out of it.
            await self.post_week_digest(now)
            await self.expire_proposals(now)
            await self.dispatch_reminders(now)
            # Last: a snapshot is worth a few hundred milliseconds of the tick,
            # but never worth delaying somebody's reminder by them.
            self.back_up(now)
        except Exception:  # pragma: no cover - keep the loop alive
            log.exception("tick failed")

    @tick.before_loop
    async def _before_tick(self) -> None:
        await self.wait_until_ready()

    async def dispatch_reminders(self, now: datetime) -> None:
        due = self.repo.due_reminders(now)
        if not due:
            return

        # Day-of pings are grouped per home channel, so each party's channel gets
        # one message covering that party's runs (DESIGN.md §1, "Party channels").
        day_of: dict[str | None, list[tuple[dict, dict]]] = {}
        for reminder in due:
            run = self.repo.get_run(reminder["run_id"])
            if run is None or run["status"] == "cancelled":
                # The run went away after the reminder was queued; retire it quietly.
                self.repo.mark_reminder_sent(reminder["id"])
                continue
            if is_stale(reminder["kind"], reminder["fire_at"], now):
                # The host was down (asleep laptop) and this is too late to be useful.
                log.info(
                    "skipping %s for run %s: due %s, now %s",
                    reminder["kind"],
                    run["id"],
                    reminder["fire_at"],
                    now,
                )
                self.repo.mark_reminder_sent(reminder["id"])
                continue
            if reminder["kind"] == DAY_OF:
                day_of.setdefault(run["channel_id"], []).append((reminder, run))
            else:
                await self._send_countdown(reminder, run)

        for channel_id, entries in day_of.items():
            await self._send_day_of(channel_id, entries)

    async def find_channel(self, channel_id: int | str | None = None) -> ChannelLookup:
        """Resolve somewhere to post, and say why if there is nowhere.

        The home channel can disappear (deleted, or the bot loses access), so
        ``POST_CHANNEL_ID`` is always tried as a backstop before giving up. When
        both fail the *reason* matters far more than the failure: "no access"
        and "no such channel" need completely different fixes, and a bare
        "couldn't post" sends the owner hunting.
        """
        problems: list[str] = []
        for candidate in (channel_id, self.settings.post_channel_id):
            if candidate is None:
                continue
            try:
                target = int(candidate)
            except (TypeError, ValueError):
                problems.append(f"`{candidate}` is not a channel id")
                continue
            channel = self.get_channel(target)
            if channel is None:
                try:
                    channel = await self.fetch_channel(target)
                except discord.Forbidden:
                    problems.append(self.no_access(target))
                    continue
                except discord.NotFound:
                    problems.append(
                        f"channel {target} does not exist, or the bot is not in that server"
                    )
                    continue
                except discord.HTTPException as exc:
                    problems.append(f"Discord would not hand over channel {target}: {exc}")
                    continue
            if not isinstance(channel, discord.abc.Messageable):
                problems.append(f"channel {target} is not a text channel")
                continue
            if not self.can_send_in(channel):
                problems.append(self.no_access(target, channel))
                continue
            return ChannelLookup(channel=channel, problem=None)

        if not problems:
            problems.append(
                "no channel was given and POST_CHANNEL_ID is not set in .env - "
                "set it, or pass a channel"
            )
        log.warning("nowhere to post: %s", "; ".join(problems))
        return ChannelLookup(channel=None, problem="; ".join(problems))

    def no_access(self, channel_id: int, channel: object | None = None) -> str:
        name = getattr(channel, "name", None)
        where = f"#{name}" if name else f"channel {channel_id}"
        who = getattr(self.user, "name", "the bot")
        return (
            f"the bot has no access to {where} - grant the {who} role "
            "View Channel + Send Messages there"
        )

    def can_send_in(self, channel: object) -> bool:
        """Whether the bot may actually post in a channel it can see.

        A guild the bot has not finished loading has no ``me``, and a DM has no
        permissions at all; both are treated as "go ahead and try", so this can
        only ever turn a *known* refusal into a better message.
        """
        guild = getattr(channel, "guild", None)
        me = getattr(guild, "me", None)
        permissions_for = getattr(channel, "permissions_for", None)
        if me is None or permissions_for is None:
            return True
        permissions = permissions_for(me)
        return bool(permissions.view_channel and permissions.send_messages)

    async def post_channel(
        self, channel_id: int | str | None = None
    ) -> discord.abc.Messageable | None:
        """Just the channel, for callers that have nothing useful to say about failure."""
        return (await self.find_channel(channel_id)).channel

    def access_report(self) -> list[dict]:
        """Every channel the bot is meant to use, and what it may do there.

        The permissions a Discord bot ends up with are the product of role
        permissions, category overwrites and per-channel overwrites, which is
        genuinely hard to reason about in the client. This says what it can
        actually do, per channel.
        """
        guild = self.get_guild(self.settings.guild_id)
        if guild is None:
            return []
        me = getattr(guild, "me", None)
        post_channel_id = self.settings.post_channel_id
        wanted = {c.id: c for c in self.watched_text_channels()}
        if post_channel_id is not None:
            channel = self.get_channel(int(post_channel_id))
            if channel is not None:
                wanted.setdefault(channel.id, channel)

        rows: list[dict] = []
        for channel in sorted(wanted.values(), key=lambda c: getattr(c, "name", "")):
            permissions = (
                channel.permissions_for(me)
                if me is not None and hasattr(channel, "permissions_for")
                else None
            )
            rows.append(
                {
                    "id": str(channel.id),
                    "name": f"#{channel.name}",
                    "watched": self.is_watched(channel),
                    "is_digest_channel": post_channel_id is not None
                    and int(post_channel_id) == channel.id,
                    "view": permissions is None or permissions.view_channel,
                    "send": permissions is None or permissions.send_messages,
                    "history": permissions is None or permissions.read_message_history,
                    "embed": permissions is None or permissions.embed_links,
                    "react": permissions is None or permissions.add_reactions,
                    # Needed to take the *other* reaction off when somebody
                    # switches ✅ <-> ❌; without it both stick and the tally lies.
                    "manage_messages": permissions is None or permissions.manage_messages,
                    "unknown": permissions is None,
                }
            )
        return rows

    def missing_manage_messages(self) -> list[str]:
        """Watched channels where ✅/❌ cannot be kept exclusive, by name.

        Read live from ``channel.permissions_for(guild.me)`` every time rather
        than from :attr:`_warned_manage_messages`, which only says whether the
        bot has *already tripped over* the missing permission -- it stays false
        until someone reacts, and stays true after the permission is granted.
        """
        return [
            row["name"]
            for row in self.access_report()
            if row["watched"] and not row["unknown"] and not row["manage_messages"]
        ]

    # -- building the reminder cards --------------------------------------
    #
    # Both builders read the run and its answers out of the database every
    # time, so the same call that first posts a card can re-render it later
    # (:meth:`refresh_run_cards`) and get wording identical to the original
    # but a current tally. Nothing about a card is remembered in memory.
    def day_of_card_for(self, runs: list[dict]) -> formatting.Card:
        """The morning card for one channel's runs, sorted by time."""
        runs = sorted(runs, key=lambda run: run["datetime"])
        rsvps = {run["id"]: self.repo.get_rsvps(run["id"]) for run in runs}
        # The morning card asks everyone on tonight's runs to answer, so it is
        # one of the four posts that may actually notify people.
        who = audience(self.repo, formatting.everyone_on(runs), "day_of")
        return formatting.day_of_card(
            runs, self.tz, rsvps, table=self.bosses, today=runs[0]["datetime"], who=who
        )

    def countdown_card_for(self, run: dict, minutes: int) -> formatting.Card:
        """The T-minus card: the whole party bar anyone who has already declined."""
        rsvps = self.repo.get_rsvps(run["id"])
        who = audience(
            self.repo,
            run["participants"],
            "countdown",
            candidates=formatting.not_declined(run, rsvps),
        )
        return formatting.countdown_card(run, minutes, self.tz, rsvps, table=self.bosses, who=who)

    # -- keeping posted cards in step -------------------------------------
    def card_needs_refresh(self, run_id: str) -> None:
        """A run changed; queue its posted cards for a re-render.

        Called *synchronously* from :class:`bot.infrastructure.db.Repo` on every write a card
        displays, so a reaction, `/rsvp`, the portal and a chat-extracted answer
        all arrive here without any of them knowing this exists. The work is
        queued rather than done: a single handler often writes several times
        (an rsvp row, then a status), and the drain task does not start until
        that handler yields, so the burst collapses into one edit per card.
        """
        self._stale_cards.add(str(run_id))
        if self._card_refresh is not None and not self._card_refresh.done():
            return  # already draining; it will pick this one up
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            # No event loop: a CLI, an export or a test wrote to the database.
            # Nothing was posted from this process, so there is nothing to fix.
            # Asked before building the coroutine, so none is left un-awaited.
            self._stale_cards.clear()
            return
        self._card_refresh = loop.create_task(self._drain_stale_cards())

    async def _drain_stale_cards(self) -> None:
        while self._stale_cards:
            run_id = self._stale_cards.pop()
            try:
                await self.refresh_run_cards(run_id)
            except Exception:  # pragma: no cover - a stale card is not worth a crash
                log.exception("could not refresh the cards for run %s", run_id)

    async def refresh_run_cards(self, run_id: str) -> int:
        """Re-render this run's already-posted reminders; returns how many were edited.

        Discord does not notify anyone about an edit, so this is the cheap way
        to keep a morning card honest: the tally on it goes on meaning what it
        said at 09:00 unless somebody rewrites it, which is how a run everybody
        had ✅'d still read "2/4 ✅" at 21:00.
        """
        run = self.repo.get_run(run_id)
        if run is None or is_past(run["datetime"], utcnow()):
            # The night has been and gone: its cards are a record of it now,
            # not a live tally, and editing them would only cost API calls.
            return 0
        edited = 0
        seen: set[str] = set()
        for reminder in self.repo.list_reminders(run_id):
            message_id = reminder["message_id"]
            if not message_id or message_id in seen:
                continue  # never posted, or already re-rendered as part of a group
            seen.add(message_id)
            card = self._rebuild_card(reminder, run)
            if card is not None and await self.edit_card(run["channel_id"], message_id, card):
                edited += 1
        # A `/debug ping` is deliberately not a reminder row -- the real ping
        # must still fire -- but it is a real card in a real channel whose ✅/❌
        # drive the real RSVP flow, so its tally goes stale in exactly the same
        # way. This is the card the "frozen 2/4 ✅" report was actually about.
        for test in self.repo.debug_messages_for_run(run_id):
            message_id = test["message_id"]
            if not message_id or message_id in seen:
                continue
            seen.add(message_id)
            card = self._rebuild_test_card(test["kind"], run)
            if card is not None and await self.edit_card(
                test["channel_id"] or run["channel_id"], message_id, card
            ):
                edited += 1
        return edited

    def _rebuild_card(self, reminder: dict, run: dict) -> formatting.Card | None:
        """What that message would say if it were posted right now."""
        if reminder["kind"] == DAY_OF:
            # One morning message covers every run that channel has that day,
            # so it is rebuilt from all of them: re-rendering it from the one
            # run that changed would drop the others off the card.
            runs = [
                self.repo.get_run(other["run_id"])
                for other in self.repo.reminders_by_message(reminder["message_id"])
            ]
            live = [r for r in runs if r is not None]
            return self.day_of_card_for(live) if live else None
        minutes = countdown_minutes(reminder["kind"])
        return None if minutes is None else self.countdown_card_for(run, minutes)

    def _rebuild_test_card(self, kind: str, run: dict) -> formatting.Card | None:
        """A `/debug ping` card, re-rendered exactly as `/debug ping` built it.

        Its own audience (``test``), so a rehearsal that named the party without
        summoning it does not start notifying anyone on the way through, and its
        own ``🧪 TEST —`` prefix, so it still cannot be mistaken for the real
        thing. The static notices `/debug ping` can also post (an amend, a
        decline) carry no tally, so there is nothing in them to bring up to date.
        """
        rsvps = self.repo.get_rsvps(run["id"])
        who = audience(self.repo, run["participants"], "test")
        if kind == DAY_OF:
            card = formatting.day_of_card(
                [run],
                self.tz,
                {run["id"]: rsvps},
                table=self.bosses,
                today=run["datetime"],
                who=who,
            )
        else:
            minutes = countdown_minutes(kind)
            if minutes is None:
                return None
            card = formatting.countdown_card(
                run, minutes, self.tz, rsvps, table=self.bosses, who=who
            )
        card.content = TEST_PREFIX + card.content
        return card

    async def edit_card(
        self, channel_id: int | str | None, message_id: int | str, card: formatting.Card
    ) -> bool:
        """Rewrite one of the bot's own cards in place; ``True`` if it stuck.

        An edit never notifies anyone, but it still goes out with the same
        explicit allow-list a send would build, so nothing here can become a
        second way to ping. Attachments are left untouched, so a card's boss
        portrait survives the edit and its ``attachment://`` thumbnail keeps
        resolving. A message that has been deleted, or that lives in a channel
        the bot can no longer see, is skipped without comment -- there is
        nothing to fix and nothing anybody can do about it.
        """
        if channel_id is None:
            return False
        card, allowed = self._prepared(card)
        try:
            channel = self.get_channel(int(channel_id)) or await self.fetch_channel(int(channel_id))
            message = await channel.fetch_message(int(message_id))  # type: ignore[union-attr]
            await message.edit(
                content=card.content, embed=self._embed(card), allowed_mentions=allowed
            )
            return True
        except (
            discord.NotFound,
            discord.Forbidden,
            discord.HTTPException,
            AttributeError,
            TypeError,
            ValueError,
            OSError,
            TimeoutError,
        ):
            log.debug("could not refresh card %s", message_id, exc_info=True)
            return False

    async def _send_day_of(self, channel_id: str | None, entries: list[tuple[dict, dict]]) -> None:
        """One message per home channel, one line per run, each tagging its own people."""
        channel = await self.post_channel(channel_id)
        if channel is None:
            log.error("no channel available for the day-of ping; leaving it queued")
            return
        entries.sort(key=lambda pair: pair[1]["datetime"])
        message = await self._post(channel, self.day_of_card_for([run for _, run in entries]))
        if message is None:
            return
        for reminder, _ in entries:
            self.repo.mark_reminder_sent(reminder["id"], message.id)

    async def _send_countdown(self, reminder: dict, run: dict) -> None:
        minutes = countdown_minutes(reminder["kind"])
        if minutes is None:  # pragma: no cover - defensive
            self.repo.mark_reminder_sent(reminder["id"])
            return
        channel = await self.post_channel(run["channel_id"])
        if channel is None:
            log.error("no channel available for run %s; leaving its countdown queued", run["id"])
            return
        message = await self._post(channel, self.countdown_card_for(run, minutes))
        if message is not None:
            self.repo.mark_reminder_sent(reminder["id"], message.id)

    @staticmethod
    def _embed(card: formatting.Card) -> discord.Embed | None:
        if not card.has_embed:
            return None
        embed = discord.Embed(
            title=card.title, description=card.description, colour=discord.Colour(card.colour)
        )
        for name, value in card.fields:
            embed.add_field(name=name, value=value, inline=False)
        if card.footer:
            embed.set_footer(text=card.footer)
        if card.thumbnail_path is not None:
            # The file travels with the message, so the thumbnail keeps working
            # without hosting anything: `attachment://` refers to it by name.
            embed.set_thumbnail(url=f"attachment://{card.thumbnail_path.name}")
        if card.image_path is not None:
            embed.set_image(url=f"attachment://{IMAGE_PREFIX}{card.image_path.name}")
        return embed

    @staticmethod
    def _file(path: Path | None, prefix: str = "") -> discord.File | None:
        """One attachment, or nothing when the file is not there to be read."""
        if path is None:
            return None
        try:
            return discord.File(str(path), filename=f"{prefix}{path.name}")
        except OSError:
            log.warning("could not read the card image at %s", path)
            return None

    @staticmethod
    def _attachments(card: formatting.Card) -> list[discord.File]:
        """The pictures that travel with a card: a portrait, a splash, or both.

        A list because a day-of card carries two, and `send(file=...)` takes
        one. Either can be missing on its own -- a boss with a portrait and no
        entry art sends exactly the message it did before the splash existed.
        """
        files = (
            BossBot._file(card.thumbnail_path),
            BossBot._file(card.image_path, IMAGE_PREFIX),
        )
        return [item for item in files if item is not None]

    def _prepared(
        self, card: formatting.Card | str, mention_users: list[str] | None = None
    ) -> tuple[formatting.Card, discord.AllowedMentions]:
        """A card and the allow-list it may go out with -- the one quiet-mode gate.

        Shared by :meth:`_post` and :meth:`edit_card` so a card cannot notify
        anybody through one path that it could not through the other.
        """
        if isinstance(card, str):
            card = formatting.Card(content=card)
        if self.quiet_mode:
            # `AllowedMentions.none()` rather than `users=[]`: it also clears
            # `replied_user`, which defaults to *on* and would otherwise notify
            # whoever is being replied to.
            return formatting.quieted(card), discord.AllowedMentions.none()
        wanted = card.mention_users if mention_users is None else mention_users
        return card, discord.AllowedMentions(
            everyone=False,
            roles=False,
            users=[discord.Object(id=int(uid)) for uid in wanted],
        )

    async def _post(
        self,
        channel: discord.abc.Messageable,
        card: formatting.Card | str,
        mention_users: list[str] | None = None,
        react: bool = True,
    ) -> discord.Message | None:
        """Send a reminder and attach the ✅/❌ reactions the RSVP flow reads back.

        The allow-list comes from the card itself (already resolved against the
        mention policy) unless the caller overrides it; either way it is
        explicit, so nothing in a message can ping by accident.

        ``react=False`` sends the card without them, for a post that reports
        rather than asks -- the weekly digest, whose runs are each answered on
        their own reminder.
        """
        card, allowed = self._prepared(card, mention_users)
        attachments = self._attachments(card)
        message: discord.Message | None = None
        for attempt in range(1, POST_ATTEMPTS + 1):
            try:
                # `files=` is omitted entirely when there is no artwork, so a
                # guild that ships none sends exactly the message it did before.
                message = await channel.send(
                    card.content,
                    embed=self._embed(card),
                    allowed_mentions=allowed,
                    **({"files": attachments} if attachments else {}),
                )
                break
            except discord.HTTPException:
                # Discord answered and refused. Sending the same rejected
                # request again just gets refused again.
                log.exception("failed to post reminder")
                return None
            except (OSError, TimeoutError):
                # Never reached Discord at all -- a DNS failure or a dropped
                # connection. One of these during the first morning rescan took
                # the whole job down and left its rows without a card, so it is
                # worth a few seconds of waiting before giving up.
                if attempt == POST_ATTEMPTS:
                    log.exception("failed to post reminder after %d attempts", attempt)
                    return None
                delay = POST_BACKOFF_SECONDS * 2 ** (attempt - 1)
                log.warning(
                    "could not reach Discord to post (attempt %d/%d); retrying in %.0fs",
                    attempt,
                    POST_ATTEMPTS,
                    delay,
                    exc_info=True,
                )
                await asyncio.sleep(delay)
        if message is None:  # pragma: no cover - the loop returns on every failure
            return None
        if not react:
            return message
        try:
            await message.add_reaction(EMOJI_YES)
            await message.add_reaction(EMOJI_NO)
        except (discord.HTTPException, OSError, TimeoutError):
            # The card is up; losing its reactions is a worse card, not a lost
            # one, and the caller still needs the message id to record.
            log.warning("posted the card but could not add its reactions", exc_info=True)
        return message

    # -- reactions --------------------------------------------------------
    async def on_raw_reaction_add(self, payload: discord.RawReactionActionEvent) -> None:
        await self._handle_reaction(payload, added=True)

    async def on_raw_reaction_remove(self, payload: discord.RawReactionActionEvent) -> None:
        await self._handle_reaction(payload, added=False)

    async def _handle_reaction(self, payload: discord.RawReactionActionEvent, added: bool) -> None:
        if self.user is not None and payload.user_id == self.user.id:
            return
        emoji = str(payload.emoji)
        if emoji not in (EMOJI_YES, EMOJI_NO):
            return
        # A /debug ping is not a reminder row, but reacting to one must still
        # drive the real RSVP flow so the whole path can be tested.
        sources = self.repo.reminders_by_message(payload.message_id) or (
            self.repo.debug_messages_for(payload.message_id)
        )
        if not sources:
            # ...and a ✅ on a proposal card is a confirmation, not an RSVP.
            proposals = self.repo.amendments_by_message(payload.message_id)
            if proposals and added:
                await self._handle_proposal_reaction(payload, proposals, emoji)
            return

        declines: list[dict] = []
        confirmations: list[dict] = []
        retractions: list[dict] = []
        applied = False
        for reminder in sources:
            run = self.repo.get_run(reminder["run_id"])
            if run is None:
                continue
            # A grouped day-of message covers several runs: apply to each run the
            # reactor actually belongs to.
            result = apply_reaction(self.repo, run, payload.user_id, emoji, added)
            if not result.applied:
                continue
            applied = True
            fresh = self.repo.get_run(run["id"]) or run
            if result.state == "no":
                declines.append(fresh)
            elif result.state == "yes":
                confirmations.append(fresh)
            elif not added and emoji == EMOJI_NO:
                # They took the ❌ back without putting a ✅ up. The run stops
                # being `at_risk`, so the "can't make it - reschedule?" notice
                # has to go with it: left standing it has the party re-planning
                # a night around somebody who is available again. `/rsvp` and
                # the portal already retract on any answer that is not "no".
                retractions.append(fresh)

        if not applied:
            return
        if added:
            # One answer per person: putting ✅ takes their ❌ off, and vice versa.
            await self._drop_opposite_reaction(payload, emoji)
        for run in confirmations + retractions:
            await self.retract_decline(run, payload.user_id)
        if declines:
            name = self._display_name(payload)
            for run in declines:
                await self.notify_decline(
                    run,
                    payload.user_id,
                    name,
                    channel_id=payload.channel_id,
                    reference_id=payload.message_id,
                )

    # -- proposal cards ---------------------------------------------------
    async def _handle_proposal_reaction(
        self, payload: discord.RawReactionActionEvent, proposals: list[dict], emoji: str
    ) -> None:
        """✅/❌ on an extractor card: commit or reject every amendment on it.

        Only a participant of the run being changed (or an admin, or the guild
        owner) counts; anyone else's reaction is ignored in silence, exactly as a
        reaction from a non-participant is on a reminder.
        """
        actor = payload.user_id
        member = payload.member
        is_admin = self.is_admin(member) if member is not None else False
        guild = self.get_guild(self.settings.guild_id)
        is_owner = guild is not None and guild.owner_id == actor
        # The role is the outer gate: a card that names nobody (a new run, a new
        # fixed timing) must not be confirmable by whoever happens to be in the
        # channel. `has_bossing_role` reads the live member when there is one and
        # falls back to the synced roster when there is not.
        has_role = (
            self.has_bossing_role(member) if member is not None else self.repo.has_role(actor)
        )

        open_proposals = [a for a in proposals if a["status"] == "proposed"]
        if not open_proposals:
            return
        allowed = [
            a
            for a in open_proposals
            if may_commit(
                a,
                self.repo.get_run(a["run_id"]) if a["run_id"] else None,
                actor,
                has_role=has_role,
                is_admin=is_admin,
                is_owner=is_owner,
            )
        ]
        if not allowed:
            log.info("ignoring %s on a proposal card from %s: not theirs to confirm", emoji, actor)
            return

        name = self._display_name(payload)
        if emoji == EMOJI_NO:
            for amendment in allowed:
                reject(self.repo, amendment)
            await self._annotate_card(payload, formatting.rejected_notice(name))
            await self._followup_on_rejection(payload, allowed)
            return

        results = [await self._commit_one(amendment, actor, payload) for amendment in allowed]
        applied = [r for r in results if r.applied]
        problems = [r.problem for r in results if r.problem]
        if applied:
            await self._annotate_card(payload, formatting.applied_notice(name))
        if problems:
            channel = await self.post_channel(payload.channel_id)
            if channel is not None:
                await self.post_plain(
                    channel,
                    "⚠️ " + "; ".join(problems),
                    [],
                    reference_id=payload.message_id,
                )

    async def _followup_on_rejection(
        self, payload: discord.RawReactionActionEvent, rejected: list[dict]
    ) -> None:
        """Let the chat pilot ask what the card should have said.

        Only its own cards, only to the member who asked for one, and at most
        once per card -- all of which :meth:`bot.chat.agent.ChatPilot.on_rejection`
        decides. Everything here does is hand it the rejection and refuse to let
        a chatbot fault break a reaction: a ❌ has already been applied and
        annotated by this point, and losing the card's rejection because the
        model was down would be the worse bug by far.

        Reached only from the ❌ path above, so a rejection made in the portal
        does nothing new: nobody is standing in the channel to answer.
        """
        try:
            await self.chat.on_rejection(
                rejected,
                reactor_id=payload.user_id,
                card_message_id=payload.message_id,
            )
        except Exception:  # noqa: BLE001 - chat must never break a reaction
            log.exception("the chat pilot could not follow up on a rejected card")

    async def _commit_one(
        self, amendment: dict, actor: int | str, payload: discord.RawReactionActionEvent
    ) -> CommitResult:
        # A card can carry several amendments for one run; committing the first
        # supersedes the rest, and a superseded row must not then be applied.
        fresh = self.repo.get_amendment(amendment["id"])
        if fresh is None or fresh["status"] != "proposed":
            return CommitResult(amendment_id=amendment["id"], kind=amendment["kind"])
        result = commit(
            self.repo,
            amendment,
            tz=self.tz,
            reset_weekday=self.settings.reset_weekday,
            reset_time=self.settings.reset_time,
            ping_time=self.ping_time,
            countdowns=self.countdowns,
            actor_id=actor,
            channel_id=amendment.get("channel_id") or payload.channel_id,
            on_fixed_created=lambda _fixed_id: self.materialise_weeks(),
        )
        if not result.applied:
            return result
        # The reacting member, by id: a ✅ on a card is the one change to the
        # schedule that already knows exactly whose decision it was.
        audit.record(
            self.repo,
            audit.Actor("card", str(actor)),
            result.kind or amendment["kind"],
            result.run_id or amendment["id"],
            f"{self._display_name(payload)} confirmed the {result.kind} on its card",
        )
        await self._mark_superseded(result.superseded)
        # A move is the one change the party needs to see spelled out, and the
        # phase-1 notice already says it exactly right.
        if result.kind == "move" and result.run_id:
            run = self.repo.get_run(result.run_id)
            if run is not None and result.old_datetime is not None:
                await self._announce_move(run, result.old_datetime)
        return result

    async def _announce_move(self, run: dict, old_at: datetime) -> None:
        channel = await self.post_channel(run["channel_id"])
        if channel is None:
            return
        # The move has already been applied and the party will be pinged again
        # on the day, so this is a receipt: names, not notifications.
        who = audience(self.repo, run["participants"], "amend")
        await self.post_plain(
            channel,
            formatting.amend_notice(run, old_at, self.tz, who),
            list(who.mentioned),
        )

    async def _annotate_card(self, payload: discord.RawReactionActionEvent, notice: str) -> None:
        """Append "✅ applied by X" to the card so its state is visible in the channel."""
        await self.annotate_message(payload.channel_id, payload.message_id, notice)

    async def annotate_message(
        self, channel_id: int | str | None, message_id: int | str | None, notice: str
    ) -> bool:
        """Append a line to one of the bot's own messages; ``True`` if it stuck.

        Used for "✅ applied by ...", "↪ superseded by a newer card", and the
        portal's "✅ applied via portal", so a card's state is visible in the
        channel however the decision was actually made.
        """
        if channel_id is None or message_id is None:
            return False
        try:
            channel = self.get_channel(int(channel_id)) or await self.fetch_channel(int(channel_id))
            message = await channel.fetch_message(int(message_id))  # type: ignore[union-attr]
            if notice in (message.content or ""):
                return True
            await message.edit(
                content=f"{message.content}\n{notice}",
                allowed_mentions=discord.AllowedMentions.none(),
            )
            return True
        except (
            discord.NotFound,
            discord.Forbidden,
            discord.HTTPException,
            AttributeError,
            TypeError,
            ValueError,
        ):
            log.debug("could not annotate message %s", message_id, exc_info=True)
            return False

    async def _mark_superseded(self, retired: list[dict]) -> None:
        """Put the superseded notice on every card this commit retired."""
        seen: set[str] = set()
        for amendment in retired:
            message_id = amendment.get("proposal_message_id")
            if not message_id or message_id in seen:
                continue
            seen.add(message_id)
            await self.annotate_message(
                amendment.get("channel_id"), message_id, formatting.SUPERSEDED_NOTICE
            )

    # -- weekly digest ----------------------------------------------------
    async def post_digest(
        self, channel_id: int | str | None = None, week: str = "this"
    ) -> discord.Message | None:
        """Post the whole guild's week (DESIGN.md §3, "Weekly digest").

        Goes to ``channel_id`` if given, else ``POST_CHANNEL_ID``. It names
        people rather than mentioning them: a guild-wide post must not notify
        thirty bossers about every party's run.
        """
        now = utcnow()
        ws = (
            next_week_start(self.tz, self.settings.reset_weekday, self.settings.reset_time, now)
            if week == "next"
            else current_week_start(
                self.tz, self.settings.reset_weekday, self.settings.reset_time, now
            )
        )
        runs = self.repo.list_runs(week_start=ws)
        channel = await self.post_channel(channel_id)
        if channel is None:
            log.error("no channel available for the weekly digest")
            return None
        card = formatting.digest_card(
            runs, ws, self.tz, {run["id"]: self.repo.get_rsvps(run["id"]) for run in runs}
        )
        # Through `_post` like every other card, so quiet mode marks it and a
        # dropped connection is retried rather than losing the week's digest.
        # `mention_users=[]` because the card names people instead of tagging
        # them; `react=False` because a summary is not something to answer.
        return await self._post(channel, card, mention_users=[], react=False)

    async def post_week_digest(self, now: datetime) -> discord.Message | None:
        """Post the digest once at each boss-week reset (DESIGN.md §3).

        Idempotent through a key of its own rather than :data:`CFG_LAST_WEEK`,
        which ``materialise_weeks`` stamps on every start: sharing it would mean
        a restart between the reset and the first tick swallowed the digest
        entirely. The key holds the week already posted for, so a Mac that slept
        through Thursday midnight posts exactly one digest when it wakes --
        not one per missed tick, and not none.
        """
        current = to_iso(
            current_week_start(self.tz, self.settings.reset_weekday, self.settings.reset_time, now)
        )
        last = self.repo.get_config(CFG_LAST_DIGEST)
        if last == current:
            return None
        if self.settings.post_channel_id is None:
            # Nowhere guild-wide to post. The week is stamped anyway, so setting
            # POST_CHANNEL_ID on a Sunday does not then back-post a digest for a
            # reset three days gone.
            self.repo.set_config(CFG_LAST_DIGEST, current)
            log.debug("no POST_CHANNEL_ID, so no weekly digest for %s", current)
            return None
        if last is None:
            # A database that has never seen a reset. This week began without
            # the bot, so it has no reset of its own to report.
            self.repo.set_config(CFG_LAST_DIGEST, current)
            log.info("the weekly digest starts at the next reset (this week is %s)", current)
            return None
        message = await self.post_digest()
        if message is None:
            # Leave the key alone and try again next tick, the way a day-of ping
            # stays queued when its channel is briefly unreachable.
            log.warning("the weekly digest for %s did not post; will retry", current)
            return None
        self.repo.set_config(CFG_LAST_DIGEST, current)
        log.info("posted the weekly digest for the boss week starting %s", current)
        return message

    # -- daily backup -----------------------------------------------------
    def back_up(self, now: datetime) -> Path | None:
        """Snapshot the database once a local day; returns the file if it wrote one.

        The database lives in a named volume, so this is what puts a copy back
        on the host (``bot.infrastructure.backup``). Failing to write one is a thing to fix, not
        a reason to stop reminding people about tonight's boss, so nothing here
        is allowed to escape into the tick.
        """
        directory = backup.backup_dir(self.repo.path)
        if directory is None:  # an in-memory database has nothing to snapshot
            return None
        local = now.astimezone(self.tz)
        day = backup.due_day(self.repo.get_config(CFG_LAST_BACKUP), local)
        if day is None or day == self._backup_failed_on:
            return None
        try:
            path, removed = backup.take(self.repo, directory, local)
        except Exception:
            self._backup_failed_on = day
            log.exception("could not back up the database to %s; retrying tomorrow", directory)
            return None
        self.repo.set_config(CFG_LAST_BACKUP, day)
        log.info("backed up the database to %s", path)
        for stale in removed:
            log.info("pruned old backup %s", stale.name)
        return path

    # -- deleted cards ----------------------------------------------------
    async def on_raw_message_delete(self, payload: discord.RawMessageDeleteEvent) -> None:
        self.withdraw_card(payload.message_id)

    async def on_raw_bulk_message_delete(self, payload: discord.RawBulkMessageDeleteEvent) -> None:
        for message_id in payload.message_ids:
            self.withdraw_card(message_id)

    def withdraw_card(self, message_id: int | str) -> list[dict]:
        """Retire the amendments on a card somebody deleted; returns them.

        Without this the rows stay ``proposed`` forever: the tick keeps them
        alive until the 24 h TTL, `/pending` and the portal's inbox keep
        offering an Approve button for a card nobody can see, and a supersede
        check still treats them as the live proposal for that run.
        """
        withdrawn = [
            a for a in self.repo.amendments_by_message(message_id) if a["status"] == "proposed"
        ]
        for amendment in withdrawn:
            self.repo.set_amendment_status(amendment["id"], "withdrawn")
        if withdrawn:
            log.info(
                "card %s was deleted; withdrew %d proposal(s): %s",
                message_id,
                len(withdrawn),
                ", ".join(a["kind"] for a in withdrawn),
            )
        return withdrawn

    async def expire_proposals(self, now: datetime) -> None:
        """Drop proposal cards nobody answered (see `bot.extract.commit.PROPOSAL_TTL`)."""
        for amendment in expire_stale(self.repo, now):
            log.info(
                "expired %s proposal %s (posted %s)",
                amendment["kind"],
                amendment["id"][:8],
                amendment["created_at"],
            )

    async def _drop_opposite_reaction(
        self, payload: discord.RawReactionActionEvent, emoji: str
    ) -> None:
        opposite = EMOJI_NO if emoji == EMOJI_YES else EMOJI_YES
        try:
            channel = self.get_channel(payload.channel_id) or await self.fetch_channel(
                payload.channel_id
            )
            message = await channel.fetch_message(payload.message_id)  # type: ignore[union-attr]
            await message.remove_reaction(opposite, discord.Object(id=payload.user_id))
        except discord.Forbidden:
            if not self._warned_manage_messages:
                self._warned_manage_messages = True
                log.warning(
                    "can't remove other people's reactions: grant the bot 'Manage Messages' "
                    "in the party channels so ✅/❌ stay one-or-the-other"
                )
        except (discord.NotFound, discord.HTTPException, AttributeError):
            log.debug("could not remove the opposite reaction", exc_info=True)

    async def notify_decline(
        self,
        run: dict,
        user_id: int | str,
        display_name: str,
        channel_id: int | str | None = None,
        reference_id: int | None = None,
    ) -> None:
        """Tell the rest of the run someone can't make it - once per person per run.

        Re-posting is suppressed for :data:`DECLINE_NOTICE_COOLDOWN` after the
        last notice, so toggling or spamming ❌ produces one message, not many.
        """
        now = utcnow()
        existing = self.repo.get_decline_notice(run["id"], user_id)
        if existing and now - existing["notified_at"] < DECLINE_NOTICE_COOLDOWN:
            return
        channel = await self.post_channel(channel_id or run["channel_id"])
        if channel is None:
            return
        others = [uid for uid in run["participants"] if uid != str(user_id)]
        # The rest of the run have to decide whether to re-plan the night, so
        # this is one of the four posts that may notify people.
        who = audience(self.repo, others, "decline")
        content = formatting.decline_notice(run, str(user_id), display_name, self.tz, who)
        message = await self.post_plain(
            channel, content, list(who.mentioned), reference_id=reference_id
        )
        self.repo.set_decline_notice(
            run["id"],
            user_id,
            getattr(message, "id", None),
            getattr(channel, "id", None),
            now,
        )

    async def retract_decline(self, run: dict, user_id: int | str) -> None:
        """They're back in: delete the "can't make it" notice so it doesn't linger."""
        existing = self.repo.get_decline_notice(run["id"], user_id)
        if not existing or not existing["message_id"]:
            return
        try:
            channel = self.get_channel(int(existing["channel_id"])) or await self.fetch_channel(
                int(existing["channel_id"])
            )
            message = await channel.fetch_message(int(existing["message_id"]))  # type: ignore[union-attr]
            await message.delete()
        except (discord.NotFound, discord.Forbidden, discord.HTTPException, AttributeError):
            log.debug("could not delete decline notice", exc_info=True)
        except (TypeError, ValueError):
            pass
        self.repo.clear_decline_notice_message(run["id"], user_id)

    def _display_name(self, payload: discord.RawReactionActionEvent) -> str:
        if payload.member is not None:
            return payload.member.display_name
        member = self.repo.get_member(payload.user_id)
        if member:
            return member["nickname"] or member["display_name"]
        return f"<@{payload.user_id}>"

    async def post_plain(
        self,
        channel: discord.abc.Messageable,
        content: str,
        mention_users: list[str],
        reference_id: int | None = None,
        mention_roles: list[str] | None = None,
        silent: bool = False,
    ) -> discord.Message | None:
        """Send a plain message, notifying exactly the ids the caller lists.

        ``mention_roles`` defaults to none at all, which is what every notice
        the bot writes for itself wants: a run's people are named individually,
        and a role ping would reach a guild rather than a party. `/say` is the
        one caller that passes any, because an admin writing a role mention by
        hand means it.

        ``silent`` sends with no pings. Placeholders must use it.
        """
        if self.quiet_mode:
            content, mention_users, mention_roles = formatting.quiet_line(content), [], []
        allowed = (
            # See `_post`: `none()` also clears `replied_user`, and this is the
            # path that actually replies to a message.
            discord.AllowedMentions.none()
            if self.quiet_mode or silent
            else discord.AllowedMentions(
                # `@everyone` / `@here` is never allowed, from any caller: it is
                # the one mention nobody can opt out of.
                everyone=False,
                # `False` rather than an empty list when there are none, so every
                # existing caller sends exactly the allow-list it always did.
                roles=(
                    [discord.Object(id=int(rid)) for rid in mention_roles]
                    if mention_roles
                    else False
                ),
                users=[discord.Object(id=int(uid)) for uid in mention_users],
            )
        )
        reference = (
            discord.MessageReference(
                message_id=reference_id,
                channel_id=getattr(channel, "id", 0),
                fail_if_not_exists=False,
            )
            if reference_id
            else None
        )
        try:
            return await channel.send(content, allowed_mentions=allowed, reference=reference)
        except discord.HTTPException:
            log.exception("failed to post decline notice")
            return None

    async def edit_plain(self, placeholder: discord.Message, content: str) -> bool:
        """Silent edit of a staging placeholder. Never pings."""
        if self.quiet_mode:
            content = formatting.quiet_line(content)
        try:
            await placeholder.edit(content=content, allowed_mentions=discord.AllowedMentions.none())
            return True
        except (discord.HTTPException, OSError, TimeoutError):
            log.warning("could not edit the staging placeholder", exc_info=True)
            return False

    @staticmethod
    async def delete_placeholder(placeholder: object) -> None:
        delete = getattr(placeholder, "delete", None)
        if delete is None:
            return
        try:
            await delete()
        except Exception:  # noqa: BLE001 - a leftover placeholder is cosmetic
            log.debug("could not delete the staging placeholder", exc_info=True)
