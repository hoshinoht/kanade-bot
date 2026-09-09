"""The model-callable weekly schedule lookup."""

from __future__ import annotations

from datetime import UTC, date, datetime, time, timedelta

from bot.agent.util import resolve_participant_text
from bot.domain.weeks import (
    calendar_week_end,
    calendar_week_start,
    current_week_start,
    next_week_start,
    week_end,
    week_start,
)

from ...api import service
from ...extract.resolve import WEEKDAY_ALIASES
from .clock import utcnow
from .contracts import MAX_MEMBER_REPLY, MAX_RUNS, ToolContext, ToolError
from .rendering import is_over, run_line
from .resolution import _RELATIVE_DAYS


def _schedule_participant(ctx: ToolContext, args: dict) -> str | None:
    """Resolve the optional participant filter to one roster id, or ``None``."""
    if "participant" not in args:
        return None
    value = args["participant"]
    if not isinstance(value, str) or not value.strip():
        return None
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
        # Small models copy the bot mention into participant; treat as first-person.
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


def _dates_in_interval(start: datetime, end: datetime, weekday: int) -> set[date]:
    """All local dates with ``weekday`` that overlap the half-open interval."""
    cursor = start.astimezone(start.tzinfo).date()
    last = (end - timedelta(microseconds=1)).astimezone(end.tzinfo).date()
    dates = set()
    while cursor <= last:
        if cursor.weekday() == weekday:
            dates.add(cursor)
        cursor += timedelta(days=1)
    return dates


def _local_day_bounds(chosen: date, tz) -> tuple[datetime, datetime]:
    """Return one local calendar date as a wall-time half-open interval."""
    return (
        datetime.combine(chosen, time(), tzinfo=tz),
        datetime.combine(chosen + timedelta(days=1), time(), tzinfo=tz),
    )


def _schedule_interval(
    ctx: ToolContext, args: dict, start: datetime, end: datetime, now: datetime, week: str
) -> tuple[datetime, datetime, set[date] | None]:
    """Narrow a selected period to its requested local date, when any."""
    if "day" not in args:
        return start, end, None
    raw_day = args["day"]
    if not isinstance(raw_day, str) or not raw_day.strip():
        return start, end, None
    value = raw_day.strip()

    today = now.astimezone(ctx.bot.tz).date()
    raw = value.lower()
    if raw in _RELATIVE_DAYS:
        chosen = today + timedelta(days=_RELATIVE_DAYS[raw])
        day_start, day_end = _local_day_bounds(chosen, ctx.bot.tz)
        return day_start, day_end, {chosen}
    elif raw in WEEKDAY_ALIASES:
        weekday = WEEKDAY_ALIASES[raw]
        if week == "auto":
            chosen = today + timedelta(days=(weekday - today.weekday()) % 7)
            day_start, day_end = _local_day_bounds(chosen, ctx.bot.tz)
            return day_start, day_end, {chosen}
        return start, end, _dates_in_interval(start, end, weekday)
    else:
        raise ToolError("day must be today, tonight, tomorrow, or one weekday.")


def _intersecting_buckets(ctx: ToolContext, start: datetime, end: datetime) -> list[datetime]:
    """Return every boss-week storage bucket overlapping a local interval."""
    bucket = week_start(
        start, ctx.bot.tz, ctx.bot.settings.reset_weekday, ctx.bot.settings.reset_time
    )
    end_utc = end.astimezone(UTC)
    buckets = []
    while bucket.astimezone(UTC) < end_utc:
        buckets.append(bucket)
        bucket = week_end(bucket, ctx.bot.tz)
    return buckets


def _bounded_schedule(heading: str, records: list[str], footer: str = "") -> str:
    """Fit complete schedule records inside the member reply budget."""
    fallback = (
        "**Schedule omitted**\n\n*Runs could not be displayed safely within the message limit.*"
    )
    if not records:
        return fallback
    selected: list[str] = []
    for record in records:
        omitted = len(records) - len(selected) - 1
        tail = (f"\n\n*(and {omitted} more)*" if omitted else "") + footer
        candidate = "\n\n".join((heading, *selected, record)) + tail
        if len(selected) >= MAX_RUNS or len(candidate) > MAX_MEMBER_REPLY:
            break
        selected.append(record)
    if not selected:
        # Never split a pathological record or its Markdown merely to fill a reply.
        return fallback + f"\n\n*(and {len(records)} more)*"
    omitted = len(records) - len(selected)
    tail = (f"\n\n*(and {omitted} more)*" if omitted else "") + footer
    return "\n\n".join((heading, *selected)) + tail


def handle(ctx: ToolContext, args: dict) -> str:
    """Return one calendar or boss week, optionally narrowed by channel, member, or day."""
    week = str(args.get("week") or "").strip().lower()
    week = week or "this"
    if week not in ("this", "next", "this_boss", "next_boss", "auto"):
        raise ToolError("Ask whether they mean this week, next week, or a specific day.")
    if week in ("this", "next"):
        basis = str(args.get("week_basis") or "calendar").strip().lower()
        if basis not in ("calendar", "boss"):
            raise ToolError("Ask whether they mean a calendar week or a boss week.")
    else:
        basis = "boss" if week.endswith("_boss") else "calendar"
    period_week = week.removesuffix("_boss") if week != "auto" else "this"
    week_label = f"{period_week}{' boss week' if basis == 'boss' else ' week'}"
    raw_scope = str(args.get("scope") or "all").strip().lower()
    scope = "channel" if ctx.force_channel_scope else "all" if ctx.force_all_channels else raw_scope
    if scope not in ("all", "channel"):
        raise ToolError(
            f"scope must be 'all' or 'channel' (got '{args.get('scope')}'). "
            "Ask whether they want this channel or all channels."
        )
    upcoming_only = ctx.upcoming_only
    participant_id = None if ctx.force_group_schedule else _schedule_participant(ctx, args)
    for_me = participant_id is not None and participant_id == str(ctx.author_id)
    participant_name = service.member_name(ctx.bot, participant_id) if participant_id else None

    now = utcnow()
    if basis == "calendar":
        start = calendar_week_start(now, ctx.bot.tz) + timedelta(
            days=7 if period_week == "next" else 0
        )
        end = calendar_week_end(start, ctx.bot.tz)
    else:
        start = (
            next_week_start(
                ctx.bot.tz, ctx.bot.settings.reset_weekday, ctx.bot.settings.reset_time, now
            )
            if period_week == "next"
            else current_week_start(
                ctx.bot.tz, ctx.bot.settings.reset_weekday, ctx.bot.settings.reset_time, now
            )
        )
        end = week_end(start, ctx.bot.tz)
    start, end, selected_dates = _schedule_interval(ctx, args, start, end, now, week)
    buckets = _intersecting_buckets(ctx, start, end)
    date_label = (
        " or ".join(day.strftime("%a %d %b") for day in sorted(selected_dates))
        if selected_dates is not None
        else None
    )

    seen: set[str] = set()
    everything = []
    start_utc = start.astimezone(UTC)
    end_utc = end.astimezone(UTC)
    for bucket in buckets:
        for run in ctx.bot.repo.list_runs(week_start=bucket):
            if run["id"] in seen:
                continue
            seen.add(run["id"])
            run_at = run["datetime"].astimezone(UTC)
            if start_utc <= run_at < end_utc:
                everything.append(run)
    everything.sort(key=lambda run: (run["datetime"].astimezone(UTC), run["id"]))
    everything = [run for run in everything if run["status"] != "cancelled"]
    dated = (
        [
            run
            for run in everything
            if run["datetime"].astimezone(ctx.bot.tz).date() in selected_dates
        ]
        if selected_dates is not None
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
    matching_runs = runs
    if upcoming_only:
        runs = [run for run in runs if not is_over(run)]
    if not runs:
        if upcoming_only:
            scope_label = "This channel" if scope == "channel" else "All channels"
            period = f"on {date_label}" if date_label else f"in {week_label}"
            if participant_id is not None:
                subject = "you" if for_me else participant_name
                here = " in this channel" if scope == "channel" else ""
                answer = f"**No upcoming runs for {subject}{here} {period}.**"
                elsewhere = [
                    run
                    for run in dated
                    if str(run["channel_id"]) != str(ctx.channel_id)
                    and participant_id in [str(p) for p in run["participants"]]
                    and not is_over(run)
                ]
                if matching_runs and all(is_over(run) for run in matching_runs):
                    answer += (
                        " Your matching scheduled runs are already done."
                        if for_me
                        else " Their matching scheduled runs are already done."
                    )
                if elsewhere and scope == "channel":
                    count = len(elsewhere)
                    subject_with_verb = "You have" if for_me else subject.capitalize() + " has"
                    answer += (
                        f" {subject_with_verb} {count} upcoming "
                        f"{'run' if count == 1 else 'runs'} in "
                        f"{'another channel' if count == 1 else 'other channels'}."
                    )
                return answer
            if scope == "channel":
                answer = f"**No upcoming runs in this channel {period}.**"
                elsewhere = [
                    run
                    for run in dated
                    if str(run["channel_id"]) != str(ctx.channel_id) and not is_over(run)
                ]
                if matching_runs and all(is_over(run) for run in matching_runs):
                    answer += " The runs scheduled here are already done."
                if elsewhere:
                    count = len(elsewhere)
                    answer += (
                        f" The group has {count} upcoming {'run' if count == 1 else 'runs'} "
                        f"in {'another channel' if count == 1 else 'other channels'}."
                    )
                return answer
            if matching_runs and all(is_over(run) for run in matching_runs):
                return (
                    f"**No runs left {date_label or week_label} · {scope_label}**\n\n"
                    "Everything scheduled in this period is already done."
                )
            return f"**No upcoming runs {period} · {scope_label}.**"
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
            period = f"on {date_label}" if date_label else f"for {week_label}"
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
            # Never report "nothing here" as "nothing at all".
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
            period = f"on {date_label}" if date_label else f"for {week_label}"
            return f"No runs are scheduled in this channel {period}.{elsewhere}"
        return (
            f"Nothing is scheduled on {date_label}."
            if date_label
            else f"Nothing is scheduled for {week_label}."
        )

    with_channel = scope == "all"
    records = [run_line(ctx.bot, run, with_channel=with_channel) for run in runs]
    scope_label = "This channel" if scope == "channel" else "All channels"
    run_count = f"{len(runs)} {'run' if len(runs) == 1 else 'runs'}"
    remaining = " left" if upcoming_only else ""
    period = date_label or week_label
    if participant_id is not None:
        owner = "Your" if for_me else f"{participant_name}'s"
        heading = f"**{owner} {run_count}{remaining} {period} · {scope_label}**"
    else:
        heading = f"**{run_count}{remaining} {period} · {scope_label}**"
    footer = ""
    if all(is_over(run) for run in runs):
        # State outright when nothing upcoming is left — the model otherwise
        # picks a finished run as "the next one".
        period = f"on {date_label}" if date_label else f"in {week_label}"
        footer = f"\n\n*Every run listed has already happened — nothing upcoming is left {period}.*"
    return _bounded_schedule(heading, records, footer)
