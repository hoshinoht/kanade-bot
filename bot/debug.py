"""The `/debug` command group: exercise the reminder machinery on demand.

Reminders normally only fire on their own schedule, which makes the whole
day-of / countdown / RSVP path awkward to check. These commands post the *real*
messages -- prefixed `🧪 TEST — ` so nobody mistakes one for the genuine ping --
without touching the run's reminder rows, so the scheduled pings still go out
exactly as planned.

Access is restricted to the guild owner, ``ADMIN_ROLE_ID`` members, and anyone
listed in ``DEBUG_USER_IDS``.
"""

from __future__ import annotations

import logging
import time as _time
import urllib.request
from datetime import datetime, timedelta
from typing import TYPE_CHECKING
from zoneinfo import ZoneInfo

import discord
from discord import app_commands

from . import formatting
from .ids import IdError, resolve_id, short_id
from .materialise import DAY_OF, countdown_minutes
from .timeutil import from_iso, utcnow

if TYPE_CHECKING:  # pragma: no cover
    from .client import BossBot

log = logging.getLogger(__name__)

TEST_PREFIX = "🧪 TEST — "
STARTED_AT = utcnow()


class DebugNotAllowed(app_commands.CheckFailure):
    """Raised when someone without debug access uses /debug."""


# ---------------------------------------------------------------------------
# pure helpers (unit tested)
# ---------------------------------------------------------------------------


def may_debug(
    user_id: int | str,
    role_ids: list[int],
    guild_owner_id: int | None,
    admin_role_id: int | None,
    debug_user_ids: list[int],
) -> bool:
    """Guild owner, an admin-role holder, or an explicitly listed user."""
    uid = int(user_id)
    if guild_owner_id is not None and uid == int(guild_owner_id):
        return True
    if admin_role_id is not None and int(admin_role_id) in role_ids:
        return True
    return uid in [int(i) for i in debug_user_ids]


def upcoming_window(reminders: list[dict], now: datetime, hours: int) -> list[dict]:
    """Unsent reminders due in the next ``hours``, soonest first."""
    horizon = now + timedelta(hours=hours)
    due = [r for r in reminders if r["sent_at"] is None and now <= r["fire_at"] <= horizon]
    return sorted(due, key=lambda r: r["fire_at"])


def format_uptime(seconds: float) -> str:
    seconds = int(max(seconds, 0))
    days, rem = divmod(seconds, 86400)
    hours, rem = divmod(rem, 3600)
    minutes, _ = divmod(rem, 60)
    if days:
        return f"{days}d {hours}h {minutes}m"
    if hours:
        return f"{hours}h {minutes}m"
    return f"{minutes}m"


def ollama_reachable(host: str, timeout: float = 2.0) -> tuple[bool, str]:
    """Quick GET /api/tags so `/debug status` can say whether phase 2 will work."""
    url = host.rstrip("/") + "/api/tags"
    try:
        started = _time.monotonic()
        with urllib.request.urlopen(url, timeout=timeout) as response:  # noqa: S310
            response.read(1)
        return True, f"reachable ({(_time.monotonic() - started) * 1000:.0f} ms)"
    except Exception as exc:  # noqa: BLE001 - any failure is "not reachable"
        return False, f"unreachable ({type(exc).__name__})"


def render_reminder_rows(reminders: list[dict], tz: ZoneInfo) -> str:
    if not reminders:
        return "_none_"
    lines = []
    for rem in reminders:
        when = rem["fire_at"].astimezone(tz).strftime("%a %d %b %H:%M")
        state = "sent" if rem["sent_at"] else "pending"
        msg = f" · msg `{rem['message_id']}`" if rem.get("message_id") else ""
        lines.append(f"run `#{short_id(rem['run_id'])}` · `{rem['kind']}` · {when} · {state}{msg}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# the group
# ---------------------------------------------------------------------------

KIND_CHOICES = [
    app_commands.Choice(name=n, value=n)
    for n in ("day_of", "countdown_60", "countdown_15", "amend", "decline")
]


def _bot(interaction: discord.Interaction) -> BossBot:
    return interaction.client  # type: ignore[return-value]


def _lookup_run(bot: BossBot, raw: str) -> dict | None:
    """Resolve a full uuid or unique prefix. /debug can reach any run."""
    try:
        resolved = resolve_id(raw, [r["id"] for r in bot.repo.list_runs()])
    except IdError:
        return None
    return bot.repo.get_run(resolved)


async def _run_autocomplete(
    interaction: discord.Interaction, current: str
) -> list[app_commands.Choice[str]]:
    """Every run, newest week first. Must never raise."""
    try:
        from .commands import _run_label

        bot = _bot(interaction)
        typed = current.strip().lstrip("#").lower()
        out = []
        for run in sorted(bot.repo.list_runs(), key=lambda r: r["datetime"]):
            label = _run_label(bot, run, interaction)
            if not typed or typed in label.lower() or short_id(run["id"]).startswith(typed):
                out.append(app_commands.Choice(name=label, value=run["id"]))
        return out[:25]
    except Exception:  # noqa: BLE001
        log.exception("debug run autocomplete failed")
        return []


class DebugGroup(app_commands.Group):
    """Testing aids. Restricted; see DEBUG_USER_IDS."""

    def __init__(self) -> None:
        super().__init__(name="debug", description="Testing aids (restricted)")

    async def interaction_check(self, interaction: discord.Interaction) -> bool:
        bot = _bot(interaction)
        guild = interaction.guild
        role_ids = [r.id for r in getattr(interaction.user, "roles", [])]
        if may_debug(
            interaction.user.id,
            role_ids,
            guild.owner_id if guild else None,
            bot.settings.admin_role_id,
            bot.settings.debug_user_id_list,
        ):
            return True
        raise DebugNotAllowed()

    # -- ping -------------------------------------------------------------
    @app_commands.command(name="ping", description="Post a test reminder for a run right now")
    @app_commands.describe(
        run_id="Pick from the dropdown, or paste an id like `a1b2c3d4`",
        kind="Which message to post",
    )
    @app_commands.autocomplete(run_id=_run_autocomplete)
    @app_commands.choices(kind=KIND_CHOICES)
    async def ping(
        self, interaction: discord.Interaction, run_id: str, kind: app_commands.Choice[str]
    ) -> None:
        bot = _bot(interaction)
        run = _lookup_run(bot, run_id)
        if run is None:
            await interaction.response.send_message(
                f"❌ No run matches `{run_id}`.", ephemeral=True
            )
            return

        body = self._render(bot, run, kind.value, interaction)
        if body is None:
            await interaction.response.send_message(
                f"❌ Don't know how to render `{kind.value}`.", ephemeral=True
            )
            return

        channel = await bot.post_channel(run["channel_id"])
        if channel is None:
            await interaction.response.send_message(
                "❌ That run's home channel isn't reachable.", ephemeral=True
            )
            return

        await interaction.response.defer(ephemeral=True)
        message = await bot._post(channel, TEST_PREFIX + body, mention_users=run["participants"])
        if message is None:
            await interaction.followup.send("❌ Couldn't post the test message.", ephemeral=True)
            return
        # Deliberately NOT a `reminders` row: the real ping must still fire.
        bot.repo.add_debug_message(message.id, run["id"], getattr(channel, "id", None), kind.value)
        await interaction.followup.send(
            f"✅ Posted a `{kind.value}` test for run `#{short_id(run['id'])}` in "
            f"<#{getattr(channel, 'id', 0)}>. Its ✅/❌ drive the real RSVP flow; "
            f"the scheduled reminders are untouched.",
            ephemeral=True,
        )

    @staticmethod
    def _render(bot: BossBot, run: dict, kind: str, interaction: discord.Interaction) -> str | None:
        if kind == DAY_OF:
            return formatting.day_of_message([run], bot.tz, today=run["datetime"])
        minutes = countdown_minutes(kind)
        if minutes is not None:
            return formatting.countdown_message(run, minutes, bot.tz)
        if kind == "amend":
            return formatting.amend_notice(run, run["datetime"] - timedelta(days=1), bot.tz)
        if kind == "decline":
            return formatting.decline_notice(
                run, interaction.user.id, interaction.user.display_name, bot.tz
            )
        return None

    # -- reminders ---------------------------------------------------------
    @app_commands.command(name="reminders", description="List reminder rows")
    @app_commands.describe(run_id="Limit to one run (optional)")
    @app_commands.autocomplete(run_id=_run_autocomplete)
    async def reminders(self, interaction: discord.Interaction, run_id: str | None = None) -> None:
        bot = _bot(interaction)
        if run_id:
            run = _lookup_run(bot, run_id)
            if run is None:
                await interaction.response.send_message(
                    f"❌ No run matches `{run_id}`.", ephemeral=True
                )
                return
            rows = bot.repo.list_reminders(run["id"])
        else:
            rows = []
            for run in bot.repo.list_runs():
                rows.extend(bot.repo.list_reminders(run["id"]))
            rows.sort(key=lambda r: r["fire_at"])
        await interaction.response.send_message(
            render_reminder_rows(rows, bot.tz)[:1900], ephemeral=True
        )

    # -- tick --------------------------------------------------------------
    @app_commands.command(name="tick", description="Run the reminder tick immediately")
    async def tick(self, interaction: discord.Interaction) -> None:
        bot = _bot(interaction)
        now = utcnow()
        due = bot.repo.due_reminders(now)
        await interaction.response.defer(ephemeral=True)
        await bot.dispatch_reminders(now)
        if not due:
            await interaction.followup.send("Nothing was due; nothing sent.", ephemeral=True)
            return
        sent = [f"run `#{short_id(r['run_id'])}` `{r['kind']}`" for r in due]
        await interaction.followup.send(
            f"Dispatched {len(sent)} reminder(s):\n" + "\n".join(sent[:20]), ephemeral=True
        )

    # -- materialise -------------------------------------------------------
    @app_commands.command(name="materialise", description="Force materialisation of both weeks")
    async def materialise(self, interaction: discord.Interaction) -> None:
        bot = _bot(interaction)
        before = {r["id"] for r in bot.repo.list_runs()}
        bot.materialise_weeks()
        created = [r for r in bot.repo.list_runs() if r["id"] not in before]
        if not created:
            await interaction.response.send_message(
                "Nothing new - both weeks were already materialised.", ephemeral=True
            )
            return
        lines = [
            f"run `#{short_id(r['id'])}` · {formatting.format_bosses(r['bosses'])} · "
            f"{formatting.local_day(r['datetime'], bot.tz)} "
            f"{formatting.local_time(r['datetime'], bot.tz)}"
            for r in created
        ]
        await interaction.response.send_message(
            f"Created {len(created)} run(s):\n" + "\n".join(lines[:20]), ephemeral=True
        )

    # -- upcoming ----------------------------------------------------------
    @app_commands.command(name="upcoming", description="What would fire in the next N hours")
    @app_commands.describe(hours="How far ahead to look (default 24)")
    async def upcoming(self, interaction: discord.Interaction, hours: int = 24) -> None:
        bot = _bot(interaction)
        rows: list[dict] = []
        for run in bot.repo.list_runs():
            rows.extend(bot.repo.list_reminders(run["id"]))
        due = upcoming_window(rows, utcnow(), hours)
        header = f"**{len(due)}** reminder(s) in the next {hours}h (dry run, nothing sent)\n"
        await interaction.response.send_message(
            (header + render_reminder_rows(due, bot.tz))[:1900], ephemeral=True
        )

    # -- status ------------------------------------------------------------
    @app_commands.command(name="status", description="Bot health and configuration")
    async def status(self, interaction: discord.Interaction) -> None:
        bot = _bot(interaction)
        now = utcnow()
        heartbeat = bot.repo.get_config("heartbeat")
        hb_age = f"{(now - from_iso(heartbeat)).total_seconds():.0f}s ago" if heartbeat else "never"
        week = bot.repo.get_config("last_materialised_week")
        week_local = from_iso(week).astimezone(bot.tz).strftime("%a %d %b %H:%M") if week else "?"
        reachable, detail = ollama_reachable(bot.settings.ollama_host)
        lines = [
            f"**uptime** {format_uptime((now - STARTED_AT).total_seconds())}",
            f"**heartbeat** {hb_age}",
            f"**boss week starts** {week_local} ({bot.settings.tz})",
            f"**day-of ping** {bot.ping_time.strftime('%H:%M')} · "
            f"**countdowns** {', '.join(str(m) for m in bot.countdowns)}m",
            f"**watched** {len(bot.settings.chat_channel_id_list)} channel(s), "
            f"{len(bot.settings.chat_category_id_list)} categor(y/ies)",
            f"**paused** {'yes' if bot.paused else 'no'}",
            f"**roster** {len(bot.repo.list_members())} with the bossing role",
            f"**runs** {len(bot.repo.list_runs())} · **fixed** {len(bot.repo.list_fixed_runs())}",
            f"**model** `{bot.settings.ollama_model}` · "
            f"**ollama** {'✅' if reachable else '❌'} {detail}",
        ]
        await interaction.response.send_message("\n".join(lines), ephemeral=True)

    # -- clear_test --------------------------------------------------------
    @app_commands.command(
        name="clear_test", description="Delete this channel's 🧪 TEST messages from the last 24h"
    )
    async def clear_test(self, interaction: discord.Interaction) -> None:
        bot = _bot(interaction)
        await interaction.response.defer(ephemeral=True)
        cutoff = utcnow() - timedelta(hours=24)
        rows = bot.repo.recent_debug_messages(cutoff, channel_id=interaction.channel_id)
        deleted = failed = 0
        for row in rows:
            try:
                message = await interaction.channel.fetch_message(int(row["message_id"]))
                await message.delete()
                deleted += 1
            except discord.NotFound:
                deleted += 1  # already gone; drop the row anyway
            except discord.HTTPException:
                failed += 1
                continue
            bot.repo.delete_debug_message(row["message_id"])
        await interaction.followup.send(
            f"🧹 Removed {deleted} test message(s)"
            + (f", {failed} could not be deleted." if failed else "."),
            ephemeral=True,
        )


__all__ = [
    "TEST_PREFIX",
    "DebugGroup",
    "DebugNotAllowed",
    "format_uptime",
    "may_debug",
    "ollama_reachable",
    "render_reminder_rows",
    "upcoming_window",
]
