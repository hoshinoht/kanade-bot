"""The Discord client: roster sync, the reminder tick loop, and reaction RSVPs.

Scheduling deliberately avoids APScheduler (see DESIGN.md §3).  The ``reminders``
table is the source of truth and a :class:`discord.ext.tasks.Loop` ticks every
``TICK_SECONDS``, sending anything due and stamping ``sent_at``.  Nothing lives
in memory, so a container restart neither loses nor replays a reminder.
"""

from __future__ import annotations

import logging
from datetime import datetime, time, timedelta
from zoneinfo import ZoneInfo

import discord
from discord.ext import tasks

from . import formatting
from .bosses import BossTable
from .config import Settings
from .db import Repo
from .materialise import DAY_OF, countdown_minutes, is_stale, materialise_week, reconcile_day_of
from .rsvp import EMOJI_NO, EMOJI_YES, apply_reaction
from .timeutil import to_iso, utcnow
from .util import roster_rows
from .watch import is_watched, origin_ids
from .weeks import current_week_start, next_week_start, parse_hhmm

log = logging.getLogger(__name__)

CFG_PING_TIME = "day_of_ping_time"
CFG_COUNTDOWNS = "countdown_minutes"
CFG_PAUSED = "paused"
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

    async def close(self) -> None:
        if self.tick.is_running():
            self.tick.cancel()
        await super().close()

    async def on_ready(self) -> None:
        log.info("logged in as %s (%s)", self.user, getattr(self.user, "id", "?"))
        await self.sync_roster()
        self.materialise_weeks()

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
        """Log messages from watched channels.

        Phase 1 only stores them; the phase-2 extractor reads the table back.
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

    async def post_channel(
        self, channel_id: int | str | None = None
    ) -> discord.abc.Messageable | None:
        """Resolve a run's home channel, falling back to ``POST_CHANNEL_ID``.

        The home channel can disappear (deleted, or the bot loses access), so the
        guild-wide channel is always tried as a backstop before giving up.
        """
        for candidate in (channel_id, self.settings.post_channel_id):
            if candidate is None:
                continue
            channel = self.get_channel(int(candidate))
            if channel is None:
                try:
                    channel = await self.fetch_channel(int(candidate))
                except (discord.NotFound, discord.Forbidden, discord.HTTPException):
                    log.warning("channel %s unavailable", candidate)
                    continue
            if isinstance(channel, discord.abc.Messageable):
                return channel
        return None

    async def _send_day_of(self, channel_id: str | None, entries: list[tuple[dict, dict]]) -> None:
        """One message per home channel, one line per run, each tagging its own people."""
        channel = await self.post_channel(channel_id)
        if channel is None:
            log.error("no channel available for the day-of ping; leaving it queued")
            return
        entries.sort(key=lambda pair: pair[1]["datetime"])
        runs = [run for _, run in entries]
        rsvps = {run["id"]: self.repo.get_rsvps(run["id"]) for run in runs}
        card = formatting.day_of_card(
            runs, self.tz, rsvps, table=self.bosses, today=runs[0]["datetime"]
        )
        message = await self._post(channel, card, mention_users=formatting.everyone_on(runs))
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
        card = formatting.countdown_card(run, minutes, self.tz, rsvps, table=self.bosses)
        # Only the people who haven't answered get pinged; the rest just see it.
        message = await self._post(channel, card, mention_users=formatting.unconfirmed(run, rsvps))
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
        return embed

    async def _post(
        self,
        channel: discord.abc.Messageable,
        card: formatting.Card | str,
        mention_users: list[str] | None = None,
    ) -> discord.Message | None:
        """Send a reminder and attach the ✅/❌ reactions the RSVP flow reads back."""
        if isinstance(card, str):
            card = formatting.Card(content=card)
        allowed = discord.AllowedMentions(
            everyone=False,
            roles=False,
            users=[discord.Object(id=int(uid)) for uid in (mention_users or [])],
        )
        try:
            message = await channel.send(
                card.content, embed=self._embed(card), allowed_mentions=allowed
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
        content = formatting.decline_notice(run, str(user_id), display_name, self.tz)
        others = [uid for uid in run["participants"] if uid != str(user_id)]
        message = await self.post_plain(channel, content, others, reference_id=reference_id)
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
