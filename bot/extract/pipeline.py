"""Per-channel buffering, and the whole extraction flow on flush.

Chat is bursty (DESIGN.md §4, "Batching"), so messages are buffered per channel
and one model call covers the burst: after ``EXTRACT_DEBOUNCE_SECONDS`` of
silence, or immediately when a message carries a mention/@here *and* a boss or a
time, which is what an "@here 9:30 later tonight" looks like.

On flush the deterministic stages run in order -- merge the model's per-message
pieces into one candidate per run, resolve ``day_ref``/``time_ref`` against the
last evidence message, match each candidate to a run -- and then:

* an ``rsvp`` is applied straight away through the same code the ✅/❌ reactions
  use (``source="chat"``), because it records an opinion rather than changing a
  schedule;
* everything else becomes a ``proposed`` amendment row and one card per burst,
  which a participant has to ✅ before anything is written.

Anything below ``EXTRACT_MIN_CONFIDENCE`` is logged and never posted.
"""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import TYPE_CHECKING, Any
from zoneinfo import ZoneInfo

from .. import formatting
from ..materialise import RUN_DONE_AFTER
from ..rsvp import apply_reaction
from ..timeutil import utcnow
from ..watch import origin_ids
from ..weeks import week_end, week_start
from . import gate
from . import prompt as prompt_mod
from .commit import supersede
from .llm import Extractor
from .match import match_run, needs_run, runs_spanned
from .merge import merge
from .resolve import Resolved, resolve
from .schema import Amendment, Extraction
from .window import (
    DEFAULT_WINDOW,
    group_bursts,
    previous_week_start,
    should_widen,
    window_since,
)

if TYPE_CHECKING:  # pragma: no cover
    from ..client import BossBot

log = logging.getLogger(__name__)

#: How far back a burst may look for context, and how "was this channel talking
#: about scheduling" is decided for a burst of nothing but "ok"/"can".
CONTEXT_WINDOW_HOURS = 48

#: Kinds that can legitimately be about several runs at once, and are split into
#: one candidate per run rather than being forced onto the best single match.
#: "find temp for me for mon and tues" is one sentence and two stand-ins.
SPLIT_ACROSS_RUNS = frozenset({"sub"})

#: How far into the past a proposed time may point before the card is pointless.
#: A rescan reads a whole boss week, so without this it would cheerfully post
#: "move HStar to Monday 21:30" on Wednesday. The grace exists because live chat
#: routinely settles a run just after it was due to start ("start now lah") and
#: that is a real amendment, not a stale one.
STALE_GRACE = timedelta(hours=3)


# ---------------------------------------------------------------------------
# the deterministic half (pure, unit tested)
# ---------------------------------------------------------------------------


@dataclass
class Planned:
    """One merged amendment, resolved and matched, ready to become a row."""

    amendment: Amendment
    resolved: Resolved
    run: dict | None = None
    payload: dict = field(default_factory=dict)
    match_reason: str = ""

    @property
    def kind(self) -> str:
        return self.amendment.kind

    @property
    def is_rsvp(self) -> bool:
        return self.amendment.kind == "rsvp"

    @property
    def needs_answer(self) -> bool:
        """The card should be worded as a suggestion, not a decision."""
        return self.amendment.is_question or self.resolved.at is None


@dataclass
class Plan:
    """Everything one burst produced."""

    planned: list[Planned] = field(default_factory=list)
    dropped: list[Planned] = field(default_factory=list)
    summary: str = ""
    #: The model call behind this plan, so `/debug extract` can show its work.
    raw: str = ""
    latency_ms: int = 0
    error: str | None = None
    message_ids: list[str] = field(default_factory=list)
    amendment_ids: list[str] = field(default_factory=list)

    @property
    def rsvps(self) -> list[Planned]:
        return [p for p in self.planned if p.is_rsvp]

    @property
    def proposals(self) -> list[Planned]:
        return [p for p in self.planned if not p.is_rsvp]


def _fix_payload(resolved: Resolved) -> dict:
    """A ``fix`` becomes a weekday + HH:MM, which is what ``fixed_runs`` stores."""
    if resolved.day is None or resolved.clock is None:
        return {}
    return {"weekday": resolved.day.weekday(), "time": resolved.clock.strftime("%H:%M")}


def _payload_for(amendment: Amendment, resolved: Resolved, run: dict | None) -> dict:
    if amendment.kind == "fix":
        return _fix_payload(resolved)
    if amendment.kind == "split" and run is not None:
        moved = [b for b in amendment.bosses if b in run["bosses"]]
        return {
            "bosses": moved or list(amendment.bosses),
            "participants": list(amendment.participants),
        }
    if amendment.kind == "sub":
        # Whoever is asking for a temp is the one dropping out; who replaces them
        # is not something the chat states, so it is left to `/fixed edit`.
        return {"remove": list(amendment.participants)}
    return {}


def already_passed(entry: Planned, now: datetime) -> bool:
    """True when acting on this amendment would change something already over.

    Two ways that happens, and a rescan over old chat hits both:

    * the time it proposes is behind ``now`` by more than :data:`STALE_GRACE`;
    * the run it targets is finished, cancelled, or its slot has passed.

    Judged against *now* rather than the burst's anchor: the anchor is when the
    conversation happened, which during a rescan is exactly the point.
    """
    at = entry.resolved.at
    if at is not None and at < now - STALE_GRACE:
        return True
    run = entry.run
    if run is None:
        return False
    if run["status"] in ("done", "cancelled"):
        return True
    return run["datetime"] + RUN_DONE_AFTER < now


def plan_burst(
    extraction: Extraction,
    *,
    anchor: datetime,
    tz: ZoneInfo,
    channel_runs: list[dict],
    guild_runs: list[dict] = (),
    burst_order: list[str] = (),
    author_ids: dict[str, str] | None = None,
    min_confidence: float = 0.0,
    now: datetime | None = None,
) -> Plan:
    """Merge, resolve and match one extraction.  No database, no Discord.

    ``author_ids`` maps message id -> author id, so an amendment that names
    nobody still matches against whoever wrote the evidence.  ``now`` is what
    "already passed" is measured against; it defaults to the real now, so a
    rescan over last week's chat proposes nothing.
    """
    author_ids = author_ids or {}
    now = now or utcnow()
    plan = Plan(summary=extraction.summary)
    merged = merge(extraction.amendments, burst_order, [r["bosses"] for r in channel_runs])
    for amendment in merged:
        resolved = resolve(amendment.day_ref, amendment.time_ref, anchor, tz)
        author = next(
            (author_ids[m] for m in amendment.evidence_message_ids if m in author_ids), None
        )
        mentioned = list(amendment.participants)

        entries: list[Planned] = []
        spanned = (
            runs_spanned(amendment, channel_runs, author_id=author)
            if amendment.kind in SPLIT_ACROSS_RUNS
            else []
        )
        if len(spanned) > 1:
            # One candidate per run, each naming only that run's bosses, so the
            # card says "stand-in for HStar+HFA" and "for HCarling+XKalos"
            # instead of one line listing four bosses across two nights.
            for run in spanned:
                per_run = amendment.model_copy(deep=True)
                per_run.bosses = [b for b in run["bosses"]]
                entries.append(
                    Planned(
                        amendment=per_run,
                        resolved=resolved,
                        run=run,
                        payload=_payload_for(per_run, resolved, run),
                        match_reason=f"one of {len(spanned)} runs it spans",
                    )
                )
        else:
            result = match_run(
                amendment, channel_runs, guild_runs, author_id=author, mentioned=mentioned
            )
            entries.append(
                Planned(
                    amendment=amendment,
                    resolved=resolved,
                    run=result.run,
                    payload=_payload_for(amendment, resolved, result.run),
                    match_reason=result.reason,
                )
            )

        for entry in entries:
            if entry.amendment.confidence < min_confidence:
                plan.dropped.append(entry)
                continue
            if needs_run(entry.kind) and entry.run is None:
                # A move/cancel/otot/split/sub/rsvp with nothing to apply it to.
                entry.match_reason = f"no run matched ({entry.match_reason})"
                plan.dropped.append(entry)
                continue
            if already_passed(entry, now):
                entry.match_reason = "already passed"
                plan.dropped.append(entry)
                continue
            plan.planned.append(entry)
    return plan


def relevant_weeks(
    anchor: datetime, tz: ZoneInfo, reset_weekday: int, reset_time
) -> tuple[datetime, datetime]:
    """``(this week, next week)`` -- an amendment may point past the reset."""
    this = week_start(anchor, tz, reset_weekday, reset_time)
    return this, week_end(this, tz)


# ---------------------------------------------------------------------------
# the buffering half
# ---------------------------------------------------------------------------


@dataclass
class RescanReport:
    """What one `/rescan` did, in the terms the ephemeral reply reports."""

    channel_id: str
    window: str
    since: datetime
    #: True when the requested week held no scheduling chat and the search was
    #: widened to the boss week before it (once, never further).
    widened: bool = False
    backfilled: int = 0
    stored: int = 0
    gated: int = 0
    bursts: int = 0
    extracted: int = 0
    proposals: int = 0
    dropped: int = 0
    stale: int = 0
    elapsed_ms: int = 0
    errors: list[str] = field(default_factory=list)
    plans: list[Plan] = field(default_factory=list)

    @property
    def asked(self) -> bool:
        """Was the model called at all?"""
        return self.extracted > 0

    @property
    def planned(self) -> list[Planned]:
        return [entry for plan in self.plans for entry in plan.planned]


@dataclass
class Burst:
    """Messages waiting for the debounce to run out in one channel."""

    channel_id: str
    message_ids: list[str] = field(default_factory=list)
    results: list[gate.GateResult] = field(default_factory=list)
    task: asyncio.Task | None = None

    def add(self, message_id: str, result: gate.GateResult) -> None:
        if message_id not in self.message_ids:
            self.message_ids.append(message_id)
            self.results.append(result)


def urgent(result: gate.GateResult) -> bool:
    """A mention or @here plus a boss or a time: extract without waiting."""
    has_target = "here" in result.signals or "mention" in result.signals
    return has_target and bool(result.signals & {"boss", "time"})


class Pipeline:
    """Owns the buffers and drives one extraction per burst."""

    def __init__(self, bot: BossBot, extractor: Extractor | None = None):
        self.bot = bot
        self.extractor = extractor or Extractor(bot.settings)
        self._bursts: dict[str, Burst] = {}

    # -- intake ------------------------------------------------------------
    @property
    def enabled(self) -> bool:
        # `extract_enabled` is the DB-backed runtime flag (seeded from the env
        # var of the same name), so the portal can switch the model off without
        # a redeploy. `paused` is `/bot pause`.
        return bool(getattr(self.bot, "extract_enabled", True)) and not self.bot.paused

    def gate_message(self, content: str) -> gate.GateResult:
        roster = [m["user_id"] for m in self.bot.repo.list_members()]
        return gate.evaluate(content, self.bot.bosses, roster)

    async def offer(self, message: Any) -> gate.GateResult | None:
        """Called by ``on_message`` for a stored, watched, non-bot message."""
        if not self.enabled:
            return None
        if not self.bot.repo.has_role(message.author.id):
            # DESIGN.md §1: messages from non-members are ignored by the extractor.
            return None
        result = self.gate_message(message.content or "")
        if not result.hit:
            return None
        channel_id, _thread = origin_ids(message.channel)
        burst = self._bursts.setdefault(str(channel_id), Burst(channel_id=str(channel_id)))
        burst.add(str(message.id), result)
        if urgent(result):
            await self._cancel_timer(burst)
            await self.flush(str(channel_id))
        else:
            self._restart_timer(burst)
        return result

    def _restart_timer(self, burst: Burst) -> None:
        if burst.task is not None:
            burst.task.cancel()
        burst.task = asyncio.create_task(self._debounce(burst.channel_id))

    async def _cancel_timer(self, burst: Burst) -> None:
        if burst.task is not None:
            burst.task.cancel()
            burst.task = None

    async def _debounce(self, channel_id: str) -> None:
        try:
            await asyncio.sleep(self.bot.settings.extract_debounce_seconds)
            await self.flush(channel_id)
        except asyncio.CancelledError:  # pragma: no cover - a newer message arrived
            raise
        except Exception:  # pragma: no cover - a burst must never kill the bot
            log.exception("burst flush failed for channel %s", channel_id)

    async def shutdown(self) -> None:
        for burst in self._bursts.values():
            await self._cancel_timer(burst)
        await self.extractor.close()

    # -- the flow ----------------------------------------------------------
    async def flush(self, channel_id: str) -> Plan | None:
        """Extract from the buffered burst in one channel."""
        burst = self._bursts.pop(str(channel_id), None)
        if burst is None or not burst.message_ids:
            return None
        await self._cancel_timer(burst)
        if not self.enabled:
            return None

        repo = self.bot.repo
        rows = [repo.get_message(mid) for mid in burst.message_ids]
        rows = [row for row in rows if row is not None]
        if not rows:
            return None

        context_is_scheduling = self._recent_scheduling(channel_id, exclude=burst.message_ids)
        if not gate.should_extract(burst.results, context_is_scheduling):
            log.debug(
                "channel %s: burst of %d message(s) was answers only, not extracting",
                channel_id,
                len(rows),
            )
            repo.mark_messages_processed(burst.message_ids)
            return None
        return await self.extract(channel_id, rows)

    async def rescan(
        self, channel_id: int | str, hours: int = 24, post: bool = True
    ) -> Plan | None:
        """One model call over a channel's stored history for the last ``hours``.

        The narrow form, kept for callers that want a single burst and a single
        :class:`Plan`.  `/rescan` uses :meth:`rescan_window`, which backfills
        from Discord first and splits a whole week into conversations.
        """
        since = utcnow() - timedelta(hours=hours)
        gated = self._gated_since(channel_id, since)[1]
        if not gated:
            return None
        return await self.extract(str(channel_id), gated, post=post)

    def _gated_since(
        self, channel_id: int | str, since: datetime, until: datetime | None = None
    ) -> tuple[list[dict], list[dict]]:
        """``(stored rows, the ones worth a model call)`` for a window.

        Same two filters the live listener applies: only roster members count
        (DESIGN.md §1), and only messages the keyword gate hits.
        """
        rows = self.bot.repo.recent_messages(channel_id, since, until)
        rows = [r for r in rows if self.bot.repo.has_role(r["author_id"])]
        return rows, [row for row in rows if self.gate_message(row["content"]).hit]

    async def _backfill(self, channel_id: int | str, since: datetime) -> int:
        """Pull history from Discord, if there is a live client to pull it with."""
        backfill = getattr(self.bot, "backfill_channel", None)
        if backfill is None:  # pragma: no cover - only a stand-in bot lacks it
            return 0
        try:
            return await backfill(channel_id, since)
        except Exception:  # noqa: BLE001 - a rescan must still read what is stored
            log.exception("backfill failed for channel %s", channel_id)
            return 0

    async def rescan_window(
        self,
        channel_id: int | str,
        window: str = DEFAULT_WINDOW,
        post: bool = True,
    ) -> RescanReport:
        """Backfill from Discord, then re-read a whole window, burst by burst.

        The order matters. Reading only the ``messages`` table finds nothing at
        all after a database reset, which is exactly when a rescan is wanted, so
        history is pulled from Discord first. A boss week of chat is then split
        back into the conversations it came from (:func:`group_bursts`) and each
        one gets its own model call, oldest first -- one prompt per week would
        be both enormous and impossible for the model to attribute.

        Calls are sequential: :data:`bot.extract.llm.MODEL_LOCK` serialises them
        anyway, and issuing them in order keeps "latest stated value wins" true
        across the week.
        """
        started = utcnow()
        bot = self.bot
        since = window_since(
            window, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time, started
        )
        report = RescanReport(channel_id=str(channel_id), window=window, since=since)
        report.backfilled = await self._backfill(channel_id, since)
        rows, gated = self._gated_since(channel_id, since)

        if should_widen(window, len(gated)):
            # A quiet week is normal just after a Thursday reset; the useful
            # answer is last week's plan. Once only -- anything older would be
            # dropped as `already passed` regardless.
            report.widened = True
            report.since = since = previous_week_start(
                since, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time
            )
            report.backfilled += await self._backfill(channel_id, since)
            rows, gated = self._gated_since(channel_id, since)

        report.stored = len(rows)
        report.gated = len(gated)
        groups = group_bursts(gated)
        report.bursts = len(groups)

        for group in groups:
            plan = await self.extract(str(channel_id), group, post=post)
            if plan is None:
                continue
            report.extracted += 1
            report.plans.append(plan)
            report.proposals += len(plan.amendment_ids)
            report.dropped += len(plan.dropped)
            report.stale += sum(1 for e in plan.dropped if e.match_reason == "already passed")
            if plan.error:
                report.errors.append(plan.error)

        report.elapsed_ms = int((utcnow() - started).total_seconds() * 1000)
        log.info(
            "rescan of channel %s (%s%s): %d backfilled, %d gated, %d burst(s), %d proposal(s)",
            channel_id,
            window,
            ", widened to the previous week" if report.widened else "",
            report.backfilled,
            report.gated,
            report.bursts,
            report.proposals,
        )
        return report

    def _recent_scheduling(self, channel_id: str, exclude: list[str]) -> bool:
        """Was this channel talking about a schedule recently?  Gates bare answers."""
        since = utcnow() - timedelta(hours=6)
        skip = set(exclude)
        for row in self.bot.repo.recent_messages(channel_id, since):
            if str(row["id"]) in skip:
                continue
            if self.gate_message(row["content"]).strong:
                return True
        return False

    def _context_rows(self, channel_id: str, burst_ids: set[str]) -> list[dict]:
        since = utcnow() - timedelta(hours=CONTEXT_WINDOW_HOURS)
        limit = self.bot.settings.extract_context_messages
        rows = [
            row
            for row in self.bot.repo.recent_messages(channel_id, since)
            if str(row["id"]) not in burst_ids
        ]
        return rows[-limit:] if limit else []

    def _name_for(self, user_id: str) -> str:
        member = self.bot.repo.get_member(user_id)
        if member:
            return member["nickname"] or member["display_name"] or str(user_id)
        return f"user{str(user_id)[-4:]}"

    def _msgs(self, rows: list[dict]) -> list[prompt_mod.Msg]:
        return [prompt_mod.Msg.from_row(row, self._name_for(row["author_id"])) for row in rows]

    async def extract(self, channel_id: str, rows: list[dict], post: bool = True) -> Plan | None:
        """Prompt -> model -> resolve/match -> rows + one card.  Never raises."""
        try:
            return await self._extract(channel_id, rows, post=post)
        except Exception:  # noqa: BLE001 - the extractor must never take the bot down
            log.exception("extraction failed for channel %s", channel_id)
            return None

    async def _extract(self, channel_id: str, rows: list[dict], post: bool) -> Plan | None:
        bot, repo = self.bot, self.bot.repo
        burst_ids = {str(row["id"]) for row in rows}
        burst = self._msgs(rows)
        context = self._msgs(self._context_rows(channel_id, burst_ids))
        anchor = burst[-1].created_at

        this_week, next_week = relevant_weeks(
            anchor, bot.tz, bot.settings.reset_weekday, bot.settings.reset_time
        )
        channel_runs = [
            run
            for week in (this_week, next_week)
            for run in repo.list_runs(week_start=week, channel_id=channel_id)
        ]
        guild_runs = [
            run for week in (this_week, next_week) for run in repo.list_runs(week_start=week)
        ]
        fixed_runs = [f for f in repo.list_fixed_runs() if f["channel_id"] == str(channel_id)]

        context_obj = prompt_mod.PromptContext(
            tz=bot.tz,
            table=bot.bosses,
            burst=burst,
            context=context,
            runs=channel_runs,
            fixed_runs=fixed_runs,
            roster=repo.list_members(),
            channel_name=self._channel_name(channel_id),
            guild_runs=guild_runs,
        )
        messages = prompt_mod.build_messages(context_obj)
        call = await self.extractor.extract(messages)
        extraction_id = repo.log_extraction(
            model=bot.settings.ollama_model,
            prompt=call.prompt,
            raw_response=call.raw or (call.error or ""),
            latency_ms=call.latency_ms,
            message_ids=sorted(burst_ids),
        )
        if not call.ok:
            log.warning("channel %s: extraction failed (%s)", channel_id, call.error)
            return Plan(
                raw=call.raw,
                latency_ms=call.latency_ms,
                error=call.error,
                message_ids=sorted(burst_ids),
            )

        plan = plan_burst(
            call.extraction,
            anchor=anchor,
            tz=bot.tz,
            channel_runs=channel_runs,
            guild_runs=guild_runs,
            burst_order=[m.id for m in context + burst],
            author_ids={m.id: m.author_id for m in context + burst},
            min_confidence=bot.settings.extract_min_confidence,
            now=utcnow(),
        )
        plan.raw = call.raw
        plan.latency_ms = call.latency_ms
        plan.message_ids = sorted(burst_ids)
        for entry in plan.dropped:
            log.info(
                "channel %s: dropped %s (confidence %.2f, %s)",
                channel_id,
                entry.kind,
                entry.amendment.confidence,
                entry.match_reason,
            )

        amendment_ids: list[str] = []
        if post:
            for entry in plan.rsvps:
                await self._apply_rsvp(entry, channel_id)
            if plan.proposals:
                amendment_ids, retired = self._record(
                    plan.proposals, channel_id, this_week, plan.summary
                )
                await self._mark_superseded(retired)
                await self._post_card(channel_id, amendment_ids)
            # Only a real pass consumes the messages; a dry run leaves them for it.
            repo.mark_messages_processed(sorted(burst_ids))
        plan.amendment_ids = amendment_ids
        repo.set_extraction_amendments(extraction_id, amendment_ids)
        log.info(
            "channel %s: %d message(s) -> %d proposal(s), %d rsvp(s) in %d ms",
            channel_id,
            len(rows),
            len(plan.proposals),
            len(plan.rsvps),
            call.latency_ms,
        )
        return plan

    def _channel_name(self, channel_id: str) -> str:
        channel = self.bot.get_channel(int(channel_id)) if str(channel_id).isdigit() else None
        return getattr(channel, "name", "") or ""

    # -- effects -----------------------------------------------------------
    async def _apply_rsvp(self, entry: Planned, channel_id: str) -> None:
        """Apply a chat answer through the same path the ✅/❌ reactions use."""
        run = entry.run
        if run is None or not entry.amendment.rsvp:
            return
        emoji = {"yes": "✅", "no": "❌"}.get(entry.amendment.rsvp)
        if emoji is None:  # "maybe" is recorded, not acted on
            return
        for user_id in entry.amendment.participants:
            result = apply_reaction(self.bot.repo, run, user_id, emoji, added=True)
            if not result.applied:
                continue
            self.bot.repo.set_rsvp(run["id"], user_id, entry.amendment.rsvp, source="chat")
            fresh = self.bot.repo.get_run(run["id"]) or run
            name = self._name_for(str(user_id))
            if result.state == "no":
                await self.bot.notify_decline(fresh, user_id, name, channel_id=channel_id)
            else:
                await self.bot.retract_decline(fresh, user_id)

    def _record(
        self, entries: list[Planned], channel_id: str, week: datetime, summary: str
    ) -> tuple[list[str], list[dict]]:
        """Insert the proposal rows, retiring any older card about the same run.

        Returns ``(new amendment ids, superseded rows)``. An urgent flush
        followed by the ordinary debounce flush of the same conversation would
        otherwise leave two live cards for one run, and a ✅ on the older one
        would re-apply a change the group has already moved past.
        """
        ids: list[str] = []
        retired: list[dict] = []
        for entry in entries:
            if entry.run is not None:
                retired.extend(supersede(self.bot.repo, run_id=entry.run["id"]))
            elif entry.amendment.bosses:
                retired.extend(
                    supersede(self.bot.repo, channel_id=channel_id, bosses=entry.amendment.bosses)
                )
            when = entry.resolved.at
            # An amendment that points past the reset belongs to next boss week.
            target_week = (
                week_start(
                    when, self.bot.tz, self.bot.settings.reset_weekday, self.bot.settings.reset_time
                )
                if when is not None
                else week
            )
            ids.append(
                self.bot.repo.create_amendment(
                    week_start=target_week,
                    kind=entry.kind,
                    bosses=entry.amendment.bosses,
                    run_id=entry.run["id"] if entry.run else None,
                    new_datetime=when,
                    participants=entry.amendment.participants,
                    confidence=entry.amendment.confidence,
                    evidence_msg_ids=entry.amendment.evidence_message_ids,
                    channel_id=channel_id,
                    is_question=entry.amendment.is_question,
                    rsvp=entry.amendment.rsvp,
                    day_ref=entry.amendment.day_ref,
                    time_ref=entry.amendment.time_ref,
                    summary=summary,
                    payload=entry.payload,
                )
            )
        # Rows inserted in this pass are never their own predecessors.
        fresh = set(ids)
        return ids, [a for a in retired if a["id"] not in fresh]

    async def _mark_superseded(self, retired: list[dict]) -> None:
        annotate = getattr(self.bot, "annotate_message", None)
        if annotate is None:  # pragma: no cover - only a stand-in bot lacks it
            return
        seen: set[str] = set()
        for amendment in retired:
            message_id = amendment.get("proposal_message_id")
            if not message_id or message_id in seen:
                continue
            seen.add(message_id)
            await annotate(amendment.get("channel_id"), message_id, formatting.SUPERSEDED_NOTICE)

    async def _post_card(self, channel_id: str, amendment_ids: list[str]) -> None:
        repo = self.bot.repo
        amendments = [repo.get_amendment(aid) for aid in amendment_ids]
        amendments = [a for a in amendments if a is not None]
        if not amendments:
            return
        runs = {}
        for amendment in amendments:
            if amendment["run_id"]:
                run = repo.get_run(amendment["run_id"])
                if run is not None:
                    runs[amendment["run_id"]] = run

        channel = await self.bot.post_channel(channel_id)
        if channel is None:
            log.error("no channel available for the proposal card; leaving it unposted")
            return
        card = formatting.proposal_card(
            amendments,
            runs,
            self.bot.tz,
            unanswered=self._unanswered(amendments, runs),
            confidence=min(a["confidence"] or 0.0 for a in amendments),
        )
        mentions = formatting.everyone_on(
            [{"participants": a["participants"] or []} for a in amendments]
            + [{"participants": r["participants"]} for r in runs.values()]
        )
        message = await self.bot._post(channel, card, mention_users=mentions)
        if message is None:
            return
        for amendment in amendments:
            repo.set_amendment_proposal_message(amendment["id"], message.id)

    @staticmethod
    def _unanswered(amendments: list[dict], runs: dict[str, dict]) -> list[str]:
        """Participants of an affected run who have not said anything yet.

        DESIGN.md §2b.1: a missing field is never filled in on someone's behalf,
        it is left TBD and the people who still need to answer are named.
        """
        if not any(a["is_question"] or a["new_datetime"] is None for a in amendments):
            return []
        spoke = {str(p) for a in amendments for p in a["participants"]}
        waiting: list[str] = []
        for run in runs.values():
            for uid in run["participants"]:
                if uid not in spoke and uid not in waiting:
                    waiting.append(uid)
        return waiting


__all__ = ["Burst", "Pipeline", "Plan", "Planned", "plan_burst", "urgent"]
