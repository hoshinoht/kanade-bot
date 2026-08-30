"""The Discord client: roster sync, the reminder tick loop, and reaction RSVPs.

Scheduling deliberately avoids APScheduler (see DESIGN.md §3).  The ``reminders``
table is the source of truth and a :class:`discord.ext.tasks.Loop` ticks every
``TICK_SECONDS``, sending anything due and stamping ``sent_at``.  Nothing lives
in memory, so a container restart neither loses nor replays a reminder.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import datetime, time, timedelta
from zoneinfo import ZoneInfo

import discord
from discord.ext import tasks

from . import formatting
from .api.server import ApiServer
from .backfill import AccessDenied, record_channel
from .bosses import BossTable
from .config import Settings
from .db import Repo
from .extract.commit import CommitResult, commit, expire_stale, may_commit, reject
from .extract.pipeline import Pipeline
from .materialise import (
    DAY_OF,
    countdown_minutes,
    is_stale,
    mark_done,
    materialise_week,
    reconcile_day_of,
)
from .pings import audience
from .rescan import RescanWorker
from .rsvp import EMOJI_NO, EMOJI_YES, apply_reaction
from .timeutil import to_iso, utcnow
from .util import roster_rows
from .watch import is_watched, origin_ids
from .weeks import current_week_start, next_week_start, parse_hhmm

log = logging.getLogger(__name__)


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
CFG_LAST_WEEK = "last_materialised_week"

#: A person's "can't make it" notice is posted at most this often per run, so a
#: ❌ toggled on and off (or mashed) never floods the channel.
DECLINE_NOTICE_COOLDOWN = timedelta(hours=6)


class BossBot(discord.Client):
    """discord.py client plus an application-command tree."""

    def __init__(self, settings: Settings, repo: Repo, bosses: BossTable):
        intents = discord.Intents.default()
        intents.members = True  # roster sync from the bossing role
        intents.message_content = True  # phase 2 extractor reads chat
        intents.guilds = True
        intents.reactions = True
        super().__init__(intents=intents)

        self.settings = settings
        self.repo = repo
        self.bosses = bosses
        self.tz: ZoneInfo = settings.zoneinfo
        self.guild_object = discord.Object(id=settings.guild_id)
        self.tree = discord.app_commands.CommandTree(self)
        self._synced = False
        self._warned_manage_messages = False
        # The chat extractor: buffers each channel's messages and runs one model
        # call per burst. Constructing it does not touch Ollama.
        self.extractor = Pipeline(self)
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
        await super().close()

    async def on_ready(self) -> None:
        log.info("logged in as %s (%s)", self.user, getattr(self.user, "id", "?"))
        await self.sync_roster()
        self.materialise_weeks()
        if self.settings.backfill_on_start:
            await self.backfill_all()

    # -- history ----------------------------------------------------------
    def watched_text_channels(self) -> list[discord.abc.GuildChannel]:
        """Every text channel the bot watches, resolved from the live guild.

        Categories are expanded here rather than from config, so a channel added
        to a watched category is picked up without a restart -- the same rule
        :func:`bot.watch.is_watched` applies per message.
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
        """Store messages from watched channels, then offer them to the extractor.

        Storing happens whether or not extraction is enabled, so `/rescan` can
        always look back over history that was captured while it was paused.
        """
        if message.author.bot or message.guild is None:
            return
        if message.guild.id != self.settings.guild_id:
            return
        if not self.is_watched(message.channel):
            return
        # Store under the parent channel so a thread's messages group with its
        # channel's -- `python -m bot.export` writes the same id.
        channel_id, _thread_id = origin_ids(message.channel)
        self.repo.record_message(
            message.id,
            channel_id,
            message.author.id,
            message.created_at,
            message.content,
        )
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
        moved = reconcile_day_of(self.repo, self.tz, ping_time)
        if moved:
            log.info("re-placed %d day-of reminder(s) at %s", moved, ping_time.strftime("%H:%M"))
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
            await self.expire_proposals(now)
            await self.dispatch_reminders(now)
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
                    problems.append(self._no_access(target))
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
                problems.append(self._no_access(target, channel))
                continue
            return ChannelLookup(channel=channel, problem=None)

        if not problems:
            problems.append(
                "no channel was given and POST_CHANNEL_ID is not set in .env - "
                "set it, or pass a channel"
            )
        log.warning("nowhere to post: %s", "; ".join(problems))
        return ChannelLookup(channel=None, problem="; ".join(problems))

    def _no_access(self, channel_id: int, channel: object | None = None) -> str:
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

    async def _send_day_of(self, channel_id: str | None, entries: list[tuple[dict, dict]]) -> None:
        """One message per home channel, one line per run, each tagging its own people."""
        channel = await self.post_channel(channel_id)
        if channel is None:
            log.error("no channel available for the day-of ping; leaving it queued")
            return
        entries.sort(key=lambda pair: pair[1]["datetime"])
        runs = [run for _, run in entries]
        rsvps = {run["id"]: self.repo.get_rsvps(run["id"]) for run in runs}
        # The morning card asks everyone on tonight's runs to answer, so it is
        # one of the four posts that may actually notify people.
        who = audience(self.repo, formatting.everyone_on(runs), "day_of")
        card = formatting.day_of_card(
            runs, self.tz, rsvps, table=self.bosses, today=runs[0]["datetime"], who=who
        )
        message = await self._post(channel, card)
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
        rsvps = self.repo.get_rsvps(run["id"])
        # Only the people who haven't answered get pinged; the rest just see it.
        who = audience(
            self.repo,
            run["participants"],
            "countdown",
            candidates=formatting.unconfirmed(run, rsvps),
        )
        card = formatting.countdown_card(
            run, minutes, self.tz, rsvps, table=self.bosses, who=who
        )
        message = await self._post(channel, card)
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
        return embed

    @staticmethod
    def _attachment(card: formatting.Card) -> discord.File | None:
        """The boss portrait to send alongside a card, if there is one."""
        path = card.thumbnail_path
        if path is None:
            return None
        try:
            return discord.File(str(path), filename=path.name)
        except OSError:
            log.warning("could not read the portrait at %s", path)
            return None

    async def _post(
        self,
        channel: discord.abc.Messageable,
        card: formatting.Card | str,
        mention_users: list[str] | None = None,
    ) -> discord.Message | None:
        """Send a reminder and attach the ✅/❌ reactions the RSVP flow reads back.

        The allow-list comes from the card itself (already resolved against the
        mention policy) unless the caller overrides it; either way it is
        explicit, so nothing in a message can ping by accident.
        """
        if isinstance(card, str):
            card = formatting.Card(content=card)
        wanted = card.mention_users if mention_users is None else mention_users
        allowed = discord.AllowedMentions(
            everyone=False,
            roles=False,
            users=[discord.Object(id=int(uid)) for uid in wanted],
        )
        attachment = self._attachment(card)
        try:
            # `file=` is omitted entirely when there is no portrait, so a guild
            # that ships none sends exactly the message it did before.
            message = await channel.send(
                card.content,
                embed=self._embed(card),
                allowed_mentions=allowed,
                **({"file": attachment} if attachment is not None else {}),
            )
            await message.add_reaction(EMOJI_YES)
            await message.add_reaction(EMOJI_NO)
            return message
        except discord.HTTPException:
            log.exception("failed to post reminder")
            return None

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

        if not applied:
            return
        if added:
            # One answer per person: putting ✅ takes their ❌ off, and vice versa.
            await self._drop_opposite_reaction(payload, emoji)
        for run in confirmations:
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
        try:
            return await channel.send(
                card.content,
                embed=self._embed(card),
                allowed_mentions=discord.AllowedMentions.none(),
            )
        except discord.HTTPException:
            log.exception("failed to post the weekly digest")
            return None

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
    ) -> discord.Message | None:
        allowed = discord.AllowedMentions(
            everyone=False,
            roles=False,
            users=[discord.Object(id=int(uid)) for uid in mention_users],
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
