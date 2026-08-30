"""Slash commands.

Everything is registered on the client's tree and copied to the configured guild
so it shows up immediately.  Replies are ephemeral except ``/schedule``, which is
public but posts with ``AllowedMentions.none()`` so it never pings.
"""

from __future__ import annotations

import logging
from collections.abc import Sequence
from datetime import datetime, time
from typing import TYPE_CHECKING

import dateparser
import discord
from discord import app_commands

from . import formatting
from .bosses import BossParseError
from .debug import DebugGroup, DebugNotAllowed
from .extract.window import DEFAULT_WINDOW, WINDOWS
from .ids import IdAmbiguous, IdError, resolve_id, short_id
from .materialise import LIVE_STATUSES, refresh_run_reminders
from .rsvp import compute_status
from .timeutil import local_naive, utcnow
from .util import can_modify_fixed, can_modify_run, mention, resolve_participant_text
from .watch import origin_ids
from .weeks import (
    WEEKDAY_NAMES,
    current_week_start,
    next_week_start,
    parse_hhmm,
    parse_weekday,
    slot_in_week,
    week_start,
)

if TYPE_CHECKING:  # pragma: no cover
    from .client import BossBot

log = logging.getLogger(__name__)

DAY_CHOICES = [
    app_commands.Choice(name=name, value=key)
    for name, key in zip(
        WEEKDAY_NAMES, ["mon", "tue", "wed", "thu", "fri", "sat", "sun"], strict=True
    )
]

WINDOW_LABELS = {
    "week": "this boss week",
    "2weeks": "this and last boss week",
    "48h": "the last 48 hours",
    "24h": "the last 24 hours",
}

WINDOW_CHOICES = [app_commands.Choice(name=WINDOW_LABELS[value], value=value) for value in WINDOWS]

#: Runs a human is offered to act on, and the ones `/schedule` shows: a night
#: that has been and gone is neither. `/debug` deliberately still sees everything.
ACTIONABLE_STATUSES = LIVE_STATUSES

#: `/restore` and `/status` have to reach the runs the others hide -- putting a
#: cancelled run back is the whole point of them.
RESTORABLE_STATUSES = ("planned", "confirmed", "at_risk", "otot", "done", "cancelled")

STATUS_CHOICES = [
    app_commands.Choice(name=name, value=value)
    for value, name in (
        ("planned", "planned"),
        ("confirmed", "confirmed"),
        ("otot", "own time"),
        ("done", "done"),
        ("cancelled", "cancelled"),
    )
]


class MissingBossingRole(app_commands.CheckFailure):
    """Raised when a non-member tries to use a scheduling command."""


class NotAllowed(app_commands.CheckFailure):
    """Raised when a member touches a run they are not part of."""


def _bot(interaction: discord.Interaction) -> BossBot:
    return interaction.client  # type: ignore[return-value]


async def _require_role(interaction: discord.Interaction) -> bool:
    if _bot(interaction).has_bossing_role(interaction.user):
        return True
    raise MissingBossingRole()


def require_role():
    return app_commands.check(_require_role)


# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------


def _resolve_participants(
    bot: BossBot,
    raw: str | None,
    invoker_id: int,
    guild: discord.Guild | None = None,
    picked: Sequence[discord.Member | None] = (),
    include_invoker: bool = True,
) -> tuple[list[str], str | None]:
    """Work out a run's participants from the pickers and the text field.

    Order is invoker (``/fixed add`` only), then the ``memberN`` pickers, then
    anything typed into ``participants:`` -- de-duplicated, order preserved.

    ``include_invoker`` is off for both ``/fixed add`` and ``/fixed edit``: the
    person setting a timing up (an admin, a pilot) is not necessarily on the
    run, and only listed participants get pinged. Pick yourself if you're on it.

    Returns ``(participant_ids, error)``.
    """
    ids: list[str] = []

    def add(uid: int | str) -> None:
        if str(uid) not in ids:
            ids.append(str(uid))

    if include_invoker:
        add(invoker_id)

    bots: list[str] = []
    for member in picked:
        if member is None:
            continue
        if member.bot:
            bots.append(mention(member.id))
            continue
        add(member.id)

    # Free text is a fallback for people typing names instead of using a picker.
    resolution = resolve_participant_text(raw, bot.repo.list_members())
    for uid in resolution.ids:
        add(uid)

    problems: list[str] = []
    if resolution.ambiguous:
        for token, names in resolution.ambiguous.items():
            problems.append(f"`{token}` could be {' or '.join(names)} - use the member pickers")
    if resolution.unknown:
        problems.append(
            "couldn't match: "
            + ", ".join(f"`{t}`" for t in resolution.unknown)
            + " - use the member pickers or `/nick`"
        )

    outsiders = [mention(uid) for uid in ids if not bot.repo.has_role(uid)]
    for uid in ids:
        member = guild.get_member(int(uid)) if guild is not None else None
        if member is not None and member.bot and mention(uid) not in bots:
            bots.append(mention(uid))
    if bots:
        ids = [uid for uid in ids if mention(uid) not in bots]
        problems.append(f"bots can't be participants: {', '.join(bots)}")
    if outsiders:
        problems.append(f"not in the bossing role: {', '.join(outsiders)}")
    if not ids and not problems:
        problems.append("a run needs at least one participant")
    return ids, "; ".join(problems) if problems else None


def _resolve(bot: BossBot, raw: str, candidates: list[str], noun: str) -> str:
    """Turn typed text into one id, or raise :class:`NotAllowed` with advice.

    Accepts a full uuid or any unique prefix, so the `#a1b2c3d4` the bot prints
    can be pasted straight back in -- though autocomplete usually fills it.
    """
    try:
        return resolve_id(raw, candidates)
    except IdAmbiguous as exc:
        listed = ", ".join(f"`#{short_id(c)}`" for c in exc.candidates[:8])
        raise NotAllowed(f"`{raw}` matches several {noun}s: {listed} - be more specific") from None
    except IdError as exc:
        raise NotAllowed(f"{exc} - pick a {noun} from the dropdown or check `/schedule`") from None


def _visible_runs(bot: BossBot, interaction: discord.Interaction) -> list[dict]:
    """Runs the invoker may act on: this week and next, theirs unless admin.

    "Theirs" is on-the-run *or* owner of the fixed timing behind it, and a run
    whose night has passed is left out -- see :data:`ACTIONABLE_STATUSES`.
    """
    runs: list[dict] = []
    for which in ("this", "next"):
        runs.extend(
            bot.repo.list_runs(week_start=_week_for(bot, which), statuses=ACTIONABLE_STATUSES)
        )
    if bot.is_admin(interaction.user):
        return runs
    mine = {r["id"] for r in bot.repo.list_runs(involving=interaction.user.id)}
    return [r for r in runs if r["id"] in mine]


def _visible_fixed(bot: BossBot, interaction: discord.Interaction) -> list[dict]:
    if bot.is_admin(interaction.user):
        return bot.repo.list_fixed_runs()
    return bot.repo.list_fixed_runs(involving=interaction.user.id)


def _channel_name(interaction: discord.Interaction, channel_id: str | None) -> str:
    if not channel_id:
        return "no channel"
    channel = interaction.guild.get_channel(int(channel_id)) if interaction.guild else None
    return f"#{channel.name}" if channel is not None else "#unknown"


def _run_label(bot: BossBot, run: dict, interaction: discord.Interaction) -> str:
    when = (
        "own time"
        if run["status"] == "otot"
        else f"{formatting.local_day(run['datetime'], bot.tz)} "
        f"{formatting.local_time(run['datetime'], bot.tz)}"
    )
    label = (
        f"{formatting.format_bosses(run['bosses'])} · {when} · "
        f"{_channel_name(interaction, run['channel_id'])} · {short_id(run['id'])}"
    )
    return label[:100]  # Discord's choice-name limit


def _fixed_label(bot: BossBot, fixed: dict, interaction: discord.Interaction) -> str:
    label = (
        f"{formatting.format_bosses(fixed['bosses'])} · "
        f"{WEEKDAY_NAMES[fixed['weekday']]} {fixed['time']} · "
        f"{_channel_name(interaction, fixed['channel_id'])} · {short_id(fixed['id'])}"
    )
    return label[:100]


def _matches(text: str, label: str, identifier: str) -> bool:
    text = text.strip().lstrip("#").lower()
    return not text or text in label.lower() or short_id(identifier).startswith(text)


async def run_autocomplete(
    interaction: discord.Interaction, current: str
) -> list[app_commands.Choice[str]]:
    """Suggest runs. Autocomplete must never raise, or the option just breaks."""
    try:
        bot = _bot(interaction)
        out = []
        for run in sorted(_visible_runs(bot, interaction), key=lambda r: r["datetime"]):
            label = _run_label(bot, run, interaction)
            if _matches(current, label, run["id"]):
                out.append(app_commands.Choice(name=label, value=run["id"]))
        return out[:25]
    except Exception:  # noqa: BLE001
        log.exception("run autocomplete failed")
        return []


async def any_run_autocomplete(
    interaction: discord.Interaction, current: str
) -> list[app_commands.Choice[str]]:
    """Like :func:`run_autocomplete`, but including cancelled/own-time/done runs.

    `/restore` and `/status` exist precisely to reach those, so hiding them
    would make the commands unusable from the dropdown.
    """
    try:
        bot = _bot(interaction)
        runs: list[dict] = []
        for which in ("this", "next"):
            runs.extend(bot.repo.list_runs(week_start=_week_for(bot, which)))
        if not bot.is_admin(interaction.user):
            mine = {r["id"] for r in bot.repo.list_runs(involving=interaction.user.id)}
            runs = [r for r in runs if r["id"] in mine]
        out = []
        for run in sorted(runs, key=lambda r: r["datetime"]):
            label = f"{_run_label(bot, run, interaction)} · {run['status']}"[:100]
            if _matches(current, label, run["id"]):
                out.append(app_commands.Choice(name=label, value=run["id"]))
        return out[:25]
    except Exception:  # noqa: BLE001
        log.exception("run autocomplete failed")
        return []


async def fixed_autocomplete(
    interaction: discord.Interaction, current: str
) -> list[app_commands.Choice[str]]:
    try:
        bot = _bot(interaction)
        out = []
        ordered = sorted(_visible_fixed(bot, interaction), key=lambda f: (f["weekday"], f["time"]))
        for fixed in ordered:
            label = _fixed_label(bot, fixed, interaction)
            if _matches(current, label, fixed["id"]):
                out.append(app_commands.Choice(name=label, value=fixed["id"]))
        return out[:25]
    except Exception:  # noqa: BLE001
        log.exception("fixed autocomplete failed")
        return []


def _week_for(bot: BossBot, which: str) -> datetime:
    now = utcnow()
    if which == "next":
        return next_week_start(bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, now)
    return current_week_start(bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, now)


def _owner_of(bot: BossBot, run: dict) -> str | None:
    if run["fixed_run_id"] is None:
        return None
    fixed = bot.repo.get_fixed_run(run["fixed_run_id"])
    return fixed["owner_id"] if fixed else None


def _load_run(bot: BossBot, interaction: discord.Interaction, run_id: str) -> dict:
    all_ids = [r["id"] for r in bot.repo.list_runs()]
    resolved = _resolve(bot, run_id, all_ids, "run")
    run = bot.repo.get_run(resolved)
    if run is None:  # pragma: no cover - resolve() guarantees it exists
        raise NotAllowed(f"No run `#{short_id(run_id)}`.")
    if not can_modify_run(
        run, interaction.user.id, bot.is_admin(interaction.user), _owner_of(bot, run)
    ):
        raise NotAllowed(f"You're not on run `#{short_id(run['id'])}`, so you can't change it.")
    return run


def _sync_run_reminders(bot: BossBot, run_id: str) -> None:
    refresh_run_reminders(bot.repo, run_id, bot.tz, bot.ping_time, bot.countdowns)


async def _announce(
    bot: BossBot, content: str, mention_users: list[str], channel_id: str | None = None
) -> None:
    """Post a run notice in that run's home channel (POST_CHANNEL_ID as fallback)."""
    channel = await bot.post_channel(channel_id)
    if channel is None:
        log.error("no channel available; dropping announcement")
        return
    await bot.post_plain(channel, content, mention_users)


def _apply_fixed_to_runs(bot: BossBot, fixed_id: str, changed: set[str]) -> None:
    """Push a fixed-run edit onto the already-materialised runs of both weeks.

    Only the fields the edit actually touched are pushed. Re-snapping every field
    would undo this week's `/amend`: editing just the note would drag a run that
    was moved Mon -> Wed back to Monday.
    """
    fixed = bot.repo.get_fixed_run(fixed_id)
    if fixed is None or not changed:
        return
    reschedule = bool(changed & {"weekday", "time"})
    for which in ("this", "next"):
        ws = _week_for(bot, which)
        run = bot.repo.run_for_fixed(fixed_id, ws)
        if run is None or run["status"] in ("done", "cancelled"):
            continue
        if "bosses" in changed:
            bot.repo.set_run_bosses(run["id"], fixed["bosses"])
        if "participants" in changed:
            bot.repo.set_run_participants(run["id"], fixed["participants"])
        if "channel_id" in changed:
            bot.repo.set_run_channel(run["id"], fixed["channel_id"])
        if reschedule:
            hour, minute = (int(p) for p in fixed["time"].split(":"))
            run_at = slot_in_week(ws, bot.tz, fixed["weekday"], time(hour, minute))
            bot.repo.set_run_datetime(run["id"], run_at, ws)
            # Only the time moved, so only the reminders need re-placing.
            _sync_run_reminders(bot, run["id"])


# ---------------------------------------------------------------------------
# /fixed
# ---------------------------------------------------------------------------


class FixedGroup(app_commands.Group):
    """Baseline weekly timings that get materialised into runs each boss week."""

    def __init__(self) -> None:
        super().__init__(name="fixed", description="Manage the weekly baseline boss timings")

    async def interaction_check(self, interaction: discord.Interaction) -> bool:
        return await _require_role(interaction)

    @app_commands.command(name="add", description="Add a fixed weekly run")
    @app_commands.describe(
        bosses="e.g. `hstar, hfa` - each boss needs a difficulty prefix (e/n/h/c/x)",
        day="Day of the week the run happens",
        time="Start time, HH:MM in the guild timezone",
        member1="Someone on the run (include yourself if you're on it)",
        member2="Someone else on the run",
        member3="Someone else on the run",
        member4="Someone else on the run",
        member5="Someone else on the run",
        member6="Someone else on the run",
        participants="Extra people by name, e.g. `MY, alvin` - the pickers above are easier",
        note="Optional note",
    )
    @app_commands.choices(day=DAY_CHOICES)
    async def add(
        self,
        interaction: discord.Interaction,
        bosses: str,
        day: app_commands.Choice[str],
        time: str,
        member1: discord.Member | None = None,
        member2: discord.Member | None = None,
        member3: discord.Member | None = None,
        member4: discord.Member | None = None,
        member5: discord.Member | None = None,
        member6: discord.Member | None = None,
        participants: str | None = None,
        note: str | None = None,
    ) -> None:
        bot = _bot(interaction)
        try:
            boss_list = bot.bosses.parse(bosses)
        except BossParseError as exc:
            await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
            return
        try:
            hhmm = parse_hhmm(time)
        except ValueError as exc:
            await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
            return

        if not bot.is_watched(interaction.channel):
            await interaction.response.send_message(
                "❌ This channel isn't watched, so a run here would never get its pings. "
                "Run `/fixed add` in your party's channel, or add this channel to "
                "`CHAT_CHANNEL_IDS` / its category to `CHAT_CATEGORY_IDS`.",
                ephemeral=True,
            )
            return

        ids, problem = _resolve_participants(
            bot,
            participants,
            interaction.user.id,
            interaction.guild,
            picked=(member1, member2, member3, member4, member5, member6),
            # The person setting up a party's timing is not necessarily on the
            # run (a guild admin, a pilot) - only listed participants get pinged.
            include_invoker=False,
        )
        if problem:
            await interaction.response.send_message(f"❌ {problem}", ephemeral=True)
            return

        fixed_id = bot.repo.add_fixed_run(
            owner_id=interaction.user.id,
            bosses=boss_list,
            weekday=parse_weekday(day.value),
            time_hhmm=hhmm.strftime("%H:%M"),
            participants=ids,
            note=note,
            channel_id=interaction.channel_id,
        )
        bot.materialise_weeks()
        # The invoker is not added automatically, so say plainly when they have
        # set up a run that will never ping them.
        not_on_it = (
            "\n(you're the owner but not on this run — it won't ping you; "
            "`/fixed edit` to add yourself)"
            if str(interaction.user.id) not in ids
            else ""
        )
        await interaction.response.send_message(
            f"✅ Fixed run `#{short_id(fixed_id)}` added — this channel is its home channel, "
            f"so its pings land here.\n"
            f"{formatting.fixed_run_line(bot.repo.get_fixed_run(fixed_id), bot.bosses)}"
            f"{not_on_it}",
            ephemeral=True,
        )

    @app_commands.command(name="list", description="List the fixed weekly runs")
    @app_commands.describe(scope="`mine` (default: on it or you own it) or `all`")
    @app_commands.choices(
        scope=[
            app_commands.Choice(name="mine", value="mine"),
            app_commands.Choice(name="all", value="all"),
        ]
    )
    async def list_(
        self, interaction: discord.Interaction, scope: app_commands.Choice[str] | None = None
    ) -> None:
        bot = _bot(interaction)
        only_mine = (scope.value if scope else "mine") == "mine"
        # "Mine" is owner *or* participant: `/fixed add` does not put the
        # invoker on the run, so filtering on participation alone hides a
        # pilot's own timings from them and reads as data loss.
        rows = bot.repo.list_fixed_runs(involving=interaction.user.id if only_mine else None)
        if not rows:
            await interaction.response.send_message(
                "No fixed runs yet - add one with `/fixed add`."
                if not only_mine
                else "None of the fixed runs are yours. "
                "`/fixed list scope:all` shows every party's.",
                ephemeral=True,
            )
            return
        body = "\n".join(formatting.fixed_run_line(f, bot.bosses) for f in rows)
        await interaction.response.send_message(body, ephemeral=True)

    @app_commands.command(name="edit", description="Edit a fixed weekly run")
    @app_commands.autocomplete(id=fixed_autocomplete)
    @app_commands.describe(
        id="Pick from the dropdown, or paste an id like `a1b2c3d4`",
        member1="Replaces the whole participant list (you are NOT added automatically)",
        member2="Someone else on the run",
        member3="Someone else on the run",
        member4="Someone else on the run",
        member5="Someone else on the run",
        member6="Someone else on the run",
        participants="Extra people by name, e.g. `MY, alvin` - the pickers above are easier",
        channel="Move the run's home channel - where its pings are posted",
    )
    @app_commands.choices(day=DAY_CHOICES)
    async def edit(
        self,
        interaction: discord.Interaction,
        id: str,
        bosses: str | None = None,
        day: app_commands.Choice[str] | None = None,
        time: str | None = None,
        member1: discord.Member | None = None,
        member2: discord.Member | None = None,
        member3: discord.Member | None = None,
        member4: discord.Member | None = None,
        member5: discord.Member | None = None,
        member6: discord.Member | None = None,
        participants: str | None = None,
        channel: discord.TextChannel | None = None,
        note: str | None = None,
    ) -> None:
        bot = _bot(interaction)
        try:
            id = _resolve(bot, id, [f["id"] for f in bot.repo.list_fixed_runs()], "fixed run")
        except NotAllowed as exc:
            await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
            return
        fixed = bot.repo.get_fixed_run(id)
        if not can_modify_fixed(fixed, interaction.user.id, bot.is_admin(interaction.user)):
            await interaction.response.send_message(
                f"❌ You're not on fixed run `#{short_id(id)}`.", ephemeral=True
            )
            return

        fields: dict = {}
        try:
            if bosses is not None:
                fields["bosses"] = bot.bosses.parse(bosses)
            if time is not None:
                fields["time"] = parse_hhmm(time).strftime("%H:%M")
        except (BossParseError, ValueError) as exc:
            await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
            return
        if day is not None:
            fields["weekday"] = parse_weekday(day.value)
        picked = (member1, member2, member3, member4, member5, member6)
        if participants is not None or any(picked):
            ids, problem = _resolve_participants(
                bot,
                participants,
                interaction.user.id,
                interaction.guild,
                picked=picked,
                include_invoker=False,
            )
            if problem:
                await interaction.response.send_message(f"❌ {problem}", ephemeral=True)
                return
            fields["participants"] = ids
        if channel is not None:
            if not bot.is_watched(channel):
                await interaction.response.send_message(
                    f"❌ {channel.mention} isn't a watched channel, so its runs would "
                    f"never get their pings.",
                    ephemeral=True,
                )
                return
            fields["channel_id"] = str(channel.id)
        if note is not None:
            fields["note"] = note
        if not fields:
            await interaction.response.send_message("Nothing to change.", ephemeral=True)
            return

        bot.repo.update_fixed_run(id, **fields)
        _apply_fixed_to_runs(bot, id, set(fields))
        bot.materialise_weeks()
        await interaction.response.send_message(
            f"✅ Updated.\n{formatting.fixed_run_line(bot.repo.get_fixed_run(id), bot.bosses)}",
            ephemeral=True,
        )

    @app_commands.command(name="remove", description="Remove a fixed weekly run")
    @app_commands.autocomplete(id=fixed_autocomplete)
    @app_commands.describe(id="Pick from the dropdown, or paste an id like `a1b2c3d4`")
    async def remove(self, interaction: discord.Interaction, id: str) -> None:
        bot = _bot(interaction)
        try:
            id = _resolve(bot, id, [f["id"] for f in bot.repo.list_fixed_runs()], "fixed run")
        except NotAllowed as exc:
            await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
            return
        fixed = bot.repo.get_fixed_run(id)
        if not can_modify_fixed(fixed, interaction.user.id, bot.is_admin(interaction.user)):
            await interaction.response.send_message(
                f"❌ You're not on fixed run `#{short_id(id)}`.", ephemeral=True
            )
            return
        cancelled = 0
        for which in ("this", "next"):
            run = bot.repo.run_for_fixed(id, _week_for(bot, which))
            if run is not None and run["status"] not in ("done", "cancelled"):
                bot.repo.set_run_status(run["id"], "cancelled")
                _sync_run_reminders(bot, run["id"])
                cancelled += 1
        bot.repo.delete_fixed_run(id)
        await interaction.response.send_message(
            f"🗑️ Fixed run `#{short_id(id)}` removed ({cancelled} upcoming run(s) cancelled).",
            ephemeral=True,
        )


# ---------------------------------------------------------------------------
# /bot
# ---------------------------------------------------------------------------


class BotGroup(app_commands.Group):
    """Bot-level switches.  Phase 1 only stores the flag; the extractor reads it."""

    def __init__(self) -> None:
        super().__init__(name="bot", description="Bot controls")

    async def interaction_check(self, interaction: discord.Interaction) -> bool:
        return await _require_role(interaction)

    @app_commands.command(name="pause", description="Stop watching chat (reminders keep running)")
    async def pause(self, interaction: discord.Interaction) -> None:
        _bot(interaction).repo.set_config("paused", "1")
        await interaction.response.send_message(
            "⏸️ Chat watching paused. Reminders and slash commands still work.", ephemeral=True
        )

    @app_commands.command(name="resume", description="Resume watching chat")
    async def resume(self, interaction: discord.Interaction) -> None:
        _bot(interaction).repo.set_config("paused", "0")
        await interaction.response.send_message("▶️ Chat watching resumed.", ephemeral=True)


# ---------------------------------------------------------------------------
# top-level commands
# ---------------------------------------------------------------------------


@app_commands.command(name="schedule", description="Show the boss schedule for a week")
@app_commands.describe(
    scope="`channel` (default in a party channel), `mine`, or `all`",
    week="`this` (default) or `next`",
    show_past="Include runs that already happened, and cancelled ones",
)
@app_commands.choices(
    scope=[
        app_commands.Choice(name="mine", value="mine"),
        app_commands.Choice(name="all", value="all"),
        app_commands.Choice(name="channel", value="channel"),
    ],
    week=[
        app_commands.Choice(name="this", value="this"),
        app_commands.Choice(name="next", value="next"),
    ],
)
@require_role()
async def schedule(
    interaction: discord.Interaction,
    scope: app_commands.Choice[str] | None = None,
    week: app_commands.Choice[str] | None = None,
    show_past: bool = False,
) -> None:
    bot = _bot(interaction)
    which_week = week.value if week else "this"
    # Inside a party channel the useful default is "this channel's runs";
    # anywhere else it is "my runs".
    default_scope = "channel" if bot.is_watched(interaction.channel) else "mine"
    which_scope = scope.value if scope else default_scope
    ws = _week_for(bot, which_week)
    # "Mine" counts runs whose fixed timing you own as well as ones you are on.
    everything = bot.repo.list_runs(
        week_start=ws,
        involving=interaction.user.id if which_scope == "mine" else None,
        channel_id=interaction.channel_id if which_scope == "channel" else None,
    )
    # A boss week is materialised whole, so by Sunday it already holds
    # Thursday's finished runs. They are hidden unless asked for.
    runs = everything if show_past else [r for r in everything if r["status"] in LIVE_STATUSES]
    hidden = len(everything) - len(runs)

    local_ws = ws.astimezone(bot.tz)
    title = f"Boss week of {local_ws.strftime('%a %d %b')} ({which_scope})"
    embed = discord.Embed(title=title, colour=discord.Colour.blurple())
    if not runs:
        embed.description = {
            "all": "Nothing still to come. Add a baseline with `/fixed add`.",
            "mine": "You have nothing left this week. `/schedule scope:all` shows everyone's.",
            "channel": (
                "Nothing left in this channel this week. `/schedule scope:mine` shows yours, "
                "`scope:all` the whole guild's."
            ),
        }[which_scope]
        if hidden:
            embed.description += f"\n{_hidden_note(hidden)}"
    else:
        for heading, day_runs in formatting.group_by_day(runs, bot.tz):
            lines = [
                formatting.schedule_line(run, bot.tz, bot.repo.get_rsvps(run["id"]))
                for run in day_runs
            ]
            embed.add_field(name=heading, value="\n".join(lines), inline=False)
        footer = "✅/❌ react on a reminder to RSVP · /amend to move a run"
        if hidden:
            footer += f" · {_hidden_note(hidden)}"
        embed.set_footer(text=footer)

    await interaction.response.send_message(
        embed=embed, allowed_mentions=discord.AllowedMentions.none()
    )


def _hidden_note(hidden: int) -> str:
    return f"{hidden} past/cancelled run(s) hidden — `show_past:True` to see them"


@app_commands.command(name="amend", description="Move a run to a new day/time")
@app_commands.describe(
    run_id="Pick from the dropdown, or paste an id like `a1b2c3d4`",
    to="e.g. `wed 21:30`, `tomorrow 9:45pm`, `in 2 hours`",
)
@app_commands.autocomplete(run_id=run_autocomplete)
@require_role()
async def amend(interaction: discord.Interaction, run_id: str, to: str) -> None:
    bot = _bot(interaction)
    try:
        run = _load_run(bot, interaction, run_id)
    except NotAllowed as exc:
        await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
        return

    parsed = dateparser.parse(
        to,
        settings={
            "RELATIVE_BASE": local_naive(utcnow(), bot.tz),
            "PREFER_DATES_FROM": "future",
            "TIMEZONE": bot.settings.tz,
            "RETURN_AS_TIMEZONE_AWARE": True,
        },
    )
    if parsed is None:
        await interaction.response.send_message(
            f"❌ Couldn't understand `{to}`. Try `wed 21:30` or `2026-09-02 21:30`.",
            ephemeral=True,
        )
        return

    old_at = run["datetime"]
    new_ws = week_start(parsed, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time)
    bot.repo.set_run_datetime(run["id"], parsed, new_ws)
    if run["status"] in ("confirmed", "at_risk"):
        # Moving a run invalidates the old attendance answers.
        bot.repo.set_run_status(run["id"], "planned")
    _sync_run_reminders(bot, run["id"])

    updated = bot.repo.get_run(run["id"])
    await interaction.response.send_message(
        f"✅ Run `#{short_id(run['id'])}` moved to "
        f"{formatting.local_day(parsed, bot.tz)} {formatting.local_time(parsed, bot.tz)}.",
        ephemeral=True,
    )
    await _announce(
        bot,
        formatting.amend_notice(updated, old_at, bot.tz),
        updated["participants"],
        channel_id=updated["channel_id"],
    )


@app_commands.command(name="cancel", description="Cancel a run for this week")
@app_commands.describe(run_id="Pick from the dropdown, or paste an id like `a1b2c3d4`")
@app_commands.autocomplete(run_id=run_autocomplete)
@require_role()
async def cancel(interaction: discord.Interaction, run_id: str) -> None:
    await _set_status(interaction, run_id, "cancelled")


@app_commands.command(name="otot", description="Mark a run as own-time (no countdown pings)")
@app_commands.describe(run_id="Pick from the dropdown, or paste an id like `a1b2c3d4`")
@app_commands.autocomplete(run_id=run_autocomplete)
@require_role()
async def otot(interaction: discord.Interaction, run_id: str) -> None:
    await _set_status(interaction, run_id, "otot")


@app_commands.command(name="restore", description="Put a run back on the schedule")
@app_commands.describe(run_id="A cancelled, own-time or finished run")
@app_commands.autocomplete(run_id=any_run_autocomplete)
@require_role()
async def restore(interaction: discord.Interaction, run_id: str) -> None:
    await _set_status(interaction, run_id, "planned")


@app_commands.command(name="done", description="Mark a run as cleared")
@app_commands.describe(run_id="Pick from the dropdown, or paste an id like `a1b2c3d4`")
@app_commands.autocomplete(run_id=any_run_autocomplete)
@require_role()
async def done(interaction: discord.Interaction, run_id: str) -> None:
    await _set_status(interaction, run_id, "done")


@app_commands.command(name="status", description="Set a run's status")
@app_commands.describe(
    run_id="Pick from the dropdown, or paste an id like `a1b2c3d4`",
    state="What the run is now",
)
@app_commands.choices(state=STATUS_CHOICES)
@app_commands.autocomplete(run_id=any_run_autocomplete)
@require_role()
async def status(
    interaction: discord.Interaction, run_id: str, state: app_commands.Choice[str]
) -> None:
    await _set_status(interaction, run_id, state.value)


async def _set_status(interaction: discord.Interaction, run_id: str, state: str) -> None:
    """The one path behind `/status`, `/otot`, `/cancel`, `/restore` and `/done`.

    It goes through the same service function the portal and `bossctl` use, so
    a transition means the same thing however it was asked for -- including the
    channel notice, which is posted once and only when something changed.
    """
    # Imported here rather than at module scope: `bot.api` pulls in FastAPI and
    # the whole portal, and the slash-command layer must not depend on that
    # being importable to work.
    from .api import service
    from .api.errors import ApiError

    bot = _bot(interaction)
    try:
        run = _load_run(bot, interaction, run_id)
    except NotAllowed as exc:
        await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
        return
    was = run["status"]
    await interaction.response.defer(ephemeral=True)
    try:
        # The reply is ephemeral, so the channel still needs telling -- but
        # without the "(via portal)" marker, because this *was* a chat decision.
        updated = await service.set_status(bot, run["id"], state, mark=False)
    except ApiError as exc:
        await interaction.followup.send(f"❌ {exc.message}", ephemeral=True)
        return
    if was == updated["status"]:
        await interaction.followup.send(
            f"Run `#{short_id(run['id'])}` is already {updated['status_label']}.", ephemeral=True
        )
        return
    await interaction.followup.send(
        f"{updated['status_label']} — run `#{short_id(run['id'])}` "
        f"({formatting.format_bosses(updated['bosses'])}, "
        f"{updated['local_day']} {updated['local_time']}).",
        ephemeral=True,
    )


@app_commands.command(name="rsvp", description="Say whether you're on a run")
@app_commands.describe(
    run_id="Pick from the dropdown, or paste an id like `a1b2c3d4`", answer="yes or no"
)
@app_commands.choices(
    answer=[
        app_commands.Choice(name="yes", value="yes"),
        app_commands.Choice(name="no", value="no"),
    ]
)
@app_commands.autocomplete(run_id=run_autocomplete)
@require_role()
async def rsvp(
    interaction: discord.Interaction, run_id: str, answer: app_commands.Choice[str]
) -> None:
    bot = _bot(interaction)
    try:
        resolved = _resolve(bot, run_id, [r["id"] for r in bot.repo.list_runs()], "run")
    except NotAllowed as exc:
        await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
        return
    run = bot.repo.get_run(resolved)
    if str(interaction.user.id) not in run["participants"]:
        await interaction.response.send_message(
            f"❌ You're not on run `#{short_id(resolved)}`.", ephemeral=True
        )
        return
    bot.repo.set_rsvp(resolved, interaction.user.id, answer.value, source="chat")
    new_status = compute_status(run["status"], run["participants"], bot.repo.get_rsvps(resolved))
    if new_status != run["status"]:
        bot.repo.set_run_status(resolved, new_status)
    await interaction.response.send_message(
        f"Noted: **{answer.value}** for run `#{short_id(resolved)}` "
        f"({formatting.STATUS_LABEL.get(new_status, new_status)}).",
        ephemeral=True,
    )
    if answer.value == "no":
        await bot.notify_decline(run, interaction.user.id, interaction.user.display_name)
    else:
        await bot.retract_decline(run, interaction.user.id)


@app_commands.command(
    name="rescan", description="Re-read this channel's chat from Discord and propose any changes"
)
@app_commands.describe(
    window="How far back to read (default: this boss week)",
    scope="This channel (default), or every watched channel",
)
@app_commands.choices(
    window=WINDOW_CHOICES,
    scope=[
        app_commands.Choice(name="this channel", value="this-channel"),
        app_commands.Choice(name="all channels", value="all-channels"),
    ],
)
@require_role()
async def rescan(
    interaction: discord.Interaction,
    window: app_commands.Choice[str] | None = None,
    scope: app_commands.Choice[str] | None = None,
) -> None:
    bot = _bot(interaction)
    everywhere = (scope.value if scope else "this-channel") == "all-channels"
    if not everywhere and not bot.is_watched(interaction.channel):
        await interaction.response.send_message(
            "❌ This channel isn't watched, so there's nothing to re-read. "
            "`/rescan scope:all channels` reads the ones that are.",
            ephemeral=True,
        )
        return
    if bot.paused:
        await interaction.response.send_message(
            "⏸️ Chat watching is paused - `/bot resume` first.", ephemeral=True
        )
        return
    if not bot.extract_enabled:
        await interaction.response.send_message(
            "❌ The extractor is switched off - turn it back on in the portal's Config page, "
            "or with `bossctl config set extract_enabled true`.",
            ephemeral=True,
        )
        return

    await interaction.response.defer(ephemeral=True)
    which = window.value if window else DEFAULT_WINDOW
    if everywhere:
        # Sweeping every party channel is minutes of model time; the interaction
        # token lasts 15, so say up front what is happening.
        await interaction.followup.send(
            "Re-reading every watched channel — one at a time, so this takes a few minutes. "
            "Each channel's cards appear in that channel.",
            ephemeral=True,
        )
        from .api import service

        result = await service.rescan(bot, None, window=which)
        await interaction.followup.send(_sweep_summary(result), ephemeral=True)
        return
    channel_id, _thread = origin_ids(interaction.channel)
    report = await bot.extractor.rescan_window(channel_id, window=which)
    await interaction.followup.send(rescan_summary(report), ephemeral=True)


def _sweep_summary(result: dict) -> str:
    """The reply for `/rescan scope:all channels` -- one line per channel."""
    lines = [
        f"• {row['channel_name']}: {row['backfilled']} pulled, {row['bursts']} conversation(s), "
        f"**{row['proposals']}** card(s)"
        + (" · checked last week too" if row["widened"] else "")
        + (f" · ⚠️ {row['error']}" if row["error"] else "")
        for row in result["channels"]
    ]
    head = (
        f"Read {len(result['channels'])} channel(s) in {result['elapsed_ms'] / 1000:.0f}s — "
        f"**{result['proposals']}** card(s) posted."
    )
    return (head + "\n" + "\n".join(lines))[:1900]


def rescan_summary(report) -> str:
    """The ephemeral reply for `/rescan`, shared with `/debug extract`.

    Leads with what it read rather than what it found, because "nothing found"
    means something quite different after backfilling 300 messages than after
    backfilling none.
    """
    label = WINDOW_LABELS.get(report.window, report.window)
    head = (
        f"Read **{label}** in {report.elapsed_ms / 1000:.1f}s — "
        f"{report.backfilled} message(s) pulled from Discord, "
        f"{report.gated} worth reading, {report.bursts} conversation(s), "
        f"{report.extracted} sent to the model."
    )
    if report.widened:
        head += "\nNothing this boss week, so I checked last week too."
    if report.errors:
        return head + f"\n❌ The model didn't answer: {report.errors[0]}"
    if not report.asked:
        return head + "\nNothing looked like scheduling, so the model wasn't asked."

    planned = report.planned
    if not planned:
        tail = f"\n**No change found.** ({report.proposals} card(s) posted.)"
        if report.stale:
            tail += f"\n{report.stale} change(s) were about runs that have already happened."
        return head + tail
    lines = [
        f"• `{p.kind}` "
        f"{formatting.format_bosses(p.amendment.bosses) if p.amendment.bosses else ''}"
        f" ({p.amendment.confidence:.2f})"
        for p in planned
    ]
    tail = f"\n**{report.proposals} card(s) posted:**\n" + "\n".join(lines)
    extra = []
    if report.stale:
        extra.append(f"{report.stale} already passed")
    below = report.dropped - report.stale
    if below > 0:
        extra.append(f"{below} below threshold or unmatched")
    if extra:
        tail += "\n_" + ", ".join(extra) + "._"
    return head + tail


@app_commands.command(name="nick", description="Attach a chat alias to a member")
@app_commands.describe(user="The member", alias="What they get called in chat, e.g. `MY`")
@require_role()
async def nick(interaction: discord.Interaction, user: discord.Member, alias: str) -> None:
    bot = _bot(interaction)
    alias = alias.strip()
    if not alias:
        await interaction.response.send_message("❌ Alias can't be empty.", ephemeral=True)
        return
    bot.repo.upsert_member(user.id, user.display_name, user.nick, bot.has_bossing_role(user))
    aliases = bot.repo.add_alias(user.id, alias)
    await interaction.response.send_message(
        f"✅ {mention(user.id)} is now also known as: {', '.join(f'`{a}`' for a in aliases)}",
        ephemeral=True,
    )


@app_commands.command(name="pingtime", description="Set the day-of ping time (guild timezone)")
@app_commands.describe(time="HH:MM, e.g. `09:00`")
@require_role()
async def pingtime(interaction: discord.Interaction, time: str) -> None:
    bot = _bot(interaction)
    try:
        parsed = parse_hhmm(time)
    except ValueError as exc:
        await interaction.response.send_message(f"❌ {exc}", ephemeral=True)
        return
    bot.repo.set_config("day_of_ping_time", parsed.strftime("%H:%M"))
    # Re-place every day-of reminder that has not fired yet.
    moved = 0
    for reminder in bot.repo.unsent_reminders(kind="day_of"):
        _sync_run_reminders(bot, reminder["run_id"])
        moved += 1
    await interaction.response.send_message(
        f"✅ Day-of pings now go out at **{parsed.strftime('%H:%M')}** "
        f"{bot.settings.tz} ({moved} pending reminder(s) rescheduled).",
        ephemeral=True,
    )


# ---------------------------------------------------------------------------
# registration + error handling
# ---------------------------------------------------------------------------


async def on_app_command_error(
    interaction: discord.Interaction, error: app_commands.AppCommandError
) -> None:
    if isinstance(error, DebugNotAllowed):
        message = (
            "❌ `/debug` is restricted to the server owner, the admin role, "
            "and users listed in `DEBUG_USER_IDS`."
        )
    elif isinstance(error, MissingBossingRole):
        message = "❌ You need the bossing role to use this bot."
    elif isinstance(error, (NotAllowed, app_commands.CheckFailure)):
        message = f"❌ {error}" if str(error) else "❌ You can't do that."
    else:
        log.exception("command error", exc_info=error)
        message = "❌ Something went wrong. Check the bot logs."
    try:
        if interaction.response.is_done():
            await interaction.followup.send(message, ephemeral=True)
        else:
            await interaction.response.send_message(message, ephemeral=True)
    except discord.HTTPException:  # pragma: no cover
        log.exception("could not report command error to the user")


def register_commands(bot: BossBot) -> None:
    """Attach every command to the client's tree."""
    tree = bot.tree
    tree.add_command(FixedGroup())
    tree.add_command(BotGroup())
    tree.add_command(DebugGroup())
    for command in (
        schedule,
        amend,
        cancel,
        otot,
        restore,
        done,
        status,
        rsvp,
        nick,
        pingtime,
        rescan,
    ):
        tree.add_command(command)
    tree.on_error = on_app_command_error
