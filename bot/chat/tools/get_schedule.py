"""The model-callable weekly schedule lookup."""

from __future__ import annotations

from datetime import date, datetime, timedelta

from bot.agent.util import resolve_participant_text
from bot.domain.weeks import current_week_start, next_week_start, week_end

from ...api import service
from ...extract.resolve import WEEKDAY_ALIASES
from .clock import utcnow
from .contracts import MAX_RUNS, ToolContext, ToolError
from .rendering import is_over, run_line
from .resolution import _RELATIVE_DAYS


def _schedule_participant(ctx: ToolContext, args: dict) -> str | None:
    """Resolve the schedule's optional participant filter to one roster id.

    An omitted field means the group-wide schedule. A supplied field must be the
    string ``"me"`` or exactly one resolvable roster member; invalid values are
    refused rather than silently widened to every member's runs.
    """
    if "participant" not in args:
        return None
    value = args["participant"]
    if not isinstance(value, str) or not value.strip():
        raise ToolError("Ask whose schedule they want: their own, or one roster member's.")
    raw = value.strip()
    if raw.lower() == "me":
        return ctx.author_id

    bot_user = str(ctx.bot_user_id or "")
    self_role = str(ctx.self_role_id or "")
    bot_references = {
        reference
        for reference in (
            bot_user,
            f"<@{bot_user}>" if bot_user else "",
            f"<@!{bot_user}>" if bot_user else "",
            self_role,
            f"<@&{self_role}>" if self_role else "",
        )
        if reference
    }
    if raw in bot_references:
        # Small models sometimes copy the mention that summoned the bot into the
        # participant field. In a first-person schedule question that mention is
        # conversational routing, not the person whose runs were requested.
        return ctx.author_id

    resolution = resolve_participant_text(raw, ctx.bot.repo.list_members())
    if resolution.unknown:
        raise ToolError(
            f"Nobody on the roster matches {', '.join(resolution.unknown)}. "
            "Ask whose schedule they want; if they mean their own, ask them to say so."
        )
    if resolution.ambiguous:
        options = "; ".join(
            f"{name}: {', '.join(matches)}" for name, matches in resolution.ambiguous.items()
        )
        raise ToolError(f"Ask which person they mean -- {options}.")
    if len(resolution.ids) != 1 or ctx.bot.repo.get_member(resolution.ids[0]) is None:
        raise ToolError("That does not identify one person on the roster. Ask who they mean.")
    return str(resolution.ids[0])


def _schedule_date(
    ctx: ToolContext, args: dict, selected_week: datetime, now: datetime
) -> date | None:
    """Resolve an optional day to one local date inside ``selected_week``.

    Relative words name a date from the captured clock. Weekdays instead name
    the unique occurrence inside the requested boss week, so ``week='next',
    day='friday'`` cannot accidentally point at this week's Friday.
    """
    if "day" not in args:
        return None
    value = args["day"]
    if not isinstance(value, str) or not value.strip():
        raise ToolError("day must be today, tonight, tomorrow, or one weekday.")

    raw = value.strip().lower()
    today = now.astimezone(ctx.bot.tz).date()
    week_date = selected_week.astimezone(ctx.bot.tz).date()
    if raw in _RELATIVE_DAYS:
        chosen = today + timedelta(days=_RELATIVE_DAYS[raw])
    elif raw in WEEKDAY_ALIASES:
        weekday = WEEKDAY_ALIASES[raw]
        chosen = week_date + timedelta(days=(weekday - week_date.weekday()) % 7)
    else:
        raise ToolError("day must be today, tonight, tomorrow, or one weekday.")

    # A reset need not be midnight. In that case both reset-day calendar dates
    # overlap the boss week (the opening evening and the closing daytime), so
    # validate against the actual interval rather than assuming seven dates.
    last_week_date = (week_end(selected_week, ctx.bot.tz) - timedelta(microseconds=1)).date()
    if not week_date <= chosen <= last_week_date:
        other_week = "next" if chosen > last_week_date else "this"
        raise ToolError(
            f"{value.strip()} is not in the requested boss week. It belongs to the "
            f"{other_week} boss week; ask whether they want that week instead."
        )
    return chosen


def handle(ctx: ToolContext, args: dict) -> str:
    """Return one boss week, optionally narrowed by channel, member, or day."""
    week = str(args.get("week") or "this").strip().lower()
    if week not in ("this", "next"):
        raise ToolError("Ask whether they mean this boss week or next boss week.")
    scope = "all" if ctx.force_all_channels else str(args.get("scope") or "all").strip().lower()
    if scope not in ("all", "channel"):
        raise ToolError("Ask whether they want this channel or all channels.")
    participant_id = None if ctx.force_group_schedule else _schedule_participant(ctx, args)
    for_me = participant_id is not None and participant_id == str(ctx.author_id)
    participant_name = service.member_name(ctx.bot, participant_id) if participant_id else None

    now = utcnow()
    selected_week = (
        (
            next_week_start(
                ctx.bot.tz, ctx.bot.settings.reset_weekday, ctx.bot.settings.reset_time, now
            )
            if week == "next"
            else current_week_start(
                ctx.bot.tz, ctx.bot.settings.reset_weekday, ctx.bot.settings.reset_time, now
            )
        )
        if "day" in args
        else service.week_for(ctx.bot, week)
    )
    selected_date = _schedule_date(ctx, args, selected_week, now)
    date_label = selected_date.strftime("%a %d %b") if selected_date is not None else None

    everything = [
        run
        for run in ctx.bot.repo.list_runs(week_start=selected_week)
        if run["status"] != "cancelled"
    ]
    dated = (
        [
            run
            for run in everything
            if run["datetime"].astimezone(ctx.bot.tz).date() == selected_date
        ]
        if selected_date is not None
        else everything
    )

    here = ctx.channel_id
    runs = (
        [run for run in dated if str(run["channel_id"]) == str(here)]
        if scope == "channel"
        else dated
    )
    if participant_id is not None:
        runs = [run for run in runs if participant_id in [str(p) for p in run["participants"]]]
    if not runs:
        if participant_id is not None:
            participant_elsewhere = (
                [
                    run
                    for run in dated
                    if str(run["channel_id"]) != str(here)
                    and participant_id in [str(p) for p in run["participants"]]
                ]
                if scope == "channel"
                else []
            )
            subject = "You are" if for_me else f"{participant_name} is"
            period = f"on {date_label}" if date_label else f"for {week} boss week"
            count = len(participant_elsewhere)
            counted_runs = f"{count} {'run' if count == 1 else 'runs'}"
            channels = "another channel" if count == 1 else "other channels"
            elsewhere = (
                f" {subject} on {counted_runs} in {channels} {period}. If the original "
                "question did not explicitly limit the channel, check all channels before "
                "answering; otherwise ask whether they want to see those runs too."
                if participant_elsewhere
                else ""
            )
            where = " in this channel" if scope == "channel" else ""
            return f"{subject} not on any runs{where} {period}.{elsewhere}"
        if scope == "channel":
            # Never let "nothing here" be reported as "nothing at all".
            count = len(dated)
            counted_runs = f"{count} {'run' if count == 1 else 'runs'}"
            channels = "another channel" if count == 1 else "other channels"
            elsewhere = (
                f" The group has {counted_runs} in {channels} "
                f"{'on ' + date_label if date_label else 'this week'}. If the original "
                "question did not explicitly limit the channel, check all channels before "
                "answering; otherwise ask whether they want to see those runs too."
                if dated
                else ""
            )
            period = f"on {date_label}" if date_label else f"for {week} boss week"
            return f"No runs are scheduled in this channel {period}.{elsewhere}"
        return (
            f"Nothing is scheduled on {date_label}."
            if date_label
            else f"Nothing is scheduled for {week} boss week."
        )

    runs.sort(key=lambda run: run["datetime"])
    with_channel = scope == "all"
    lines = [run_line(ctx.bot, run, with_channel=with_channel) for run in runs[:MAX_RUNS]]
    more = len(runs) - len(lines)
    if participant_id is not None:
        owner = "Your" if for_me else f"{participant_name}'s"
        period = f"on {date_label}" if date_label else f"in {week} boss week"
        heading = (
            f"**{owner} runs {period}, in this channel:**"
            if scope == "channel"
            else f"**{owner} runs {period}, all channels** (say which channel each run is in):"
        )
    else:
        period = f"Runs on {date_label}" if date_label else f"{week.capitalize()} boss week"
        heading = (
            f"**{period}, in this channel only:**"
            if scope == "channel"
            else f"**{period}, ALL channels** (say which channel each run is in):"
        )
    answer = "\n".join([heading, "", *lines]) + (f"\n*(and {more} more)*" if more > 0 else "")

    if all(is_over(run) for run in runs):
        # The per-line markers are enough when only some are past; a week with
        # nothing left at all is what made the model pick a finished run as "the
        # next one", so that case is stated outright.
        answer += (
            "\n\n*Every run listed has already happened — nothing upcoming is left "
            f"{'on ' + date_label if date_label else f'in {week} boss week'}.*"
        )
    return answer
