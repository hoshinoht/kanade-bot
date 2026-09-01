"""The JSON API under ``/api``.

Every route depends on :func:`bot.api.deps.require_identity`, so an
unauthenticated request never reaches a handler.  The work itself lives in
:mod:`bot.api.service`, which the portal's HTML routes call too -- there is one
implementation of "cancel a run", not two.
"""

from __future__ import annotations

import json
import logging

from fastapi import APIRouter, Body, Query
from fastapi.responses import StreamingResponse

from . import service
from .deps import Bot, Caller
from .errors import BadRequest
from .models import (
    AccessOut,
    AmendIn,
    AmendmentOut,
    ApproveIn,
    ApproveOut,
    ChatInteractionDetailOut,
    ChatInteractionOut,
    ChatSummaryOut,
    ConfigIn,
    ConfigOut,
    DeletedOut,
    DigestIn,
    DigestOut,
    ExtractionDetailOut,
    ExtractionOut,
    FixedCreate,
    FixedOut,
    FixedUpdate,
    MemberOut,
    MemberUpdate,
    NickIn,
    NickOut,
    PingIn,
    PingOut,
    RejectOut,
    ReminderOut,
    RescanIn,
    RescanJobDetailOut,
    RescanJobOut,
    RescanTargetOut,
    RsvpIn,
    RunOut,
    ScheduleOut,
    StatusIn,
    SwapIn,
    Week,
)

log = logging.getLogger(__name__)

router = APIRouter(prefix="/api", tags=["api"])


# ---------------------------------------------------------------------------
# schedule
# ---------------------------------------------------------------------------


@router.get("/schedule", response_model=ScheduleOut, summary="One boss week's runs")
async def get_schedule(
    bot: Bot,
    caller: Caller,
    week: Week = "this",
    channel: str | None = Query(default=None, description="only this home channel"),
    user: str | None = Query(default=None, description="only runs this member is on"),
    boss: str | None = Query(default=None, description="substring match on a boss token"),
    show_past: bool = Query(default=False, description="include done and cancelled runs"),
) -> dict:
    return service.schedule(
        bot, week=week, channel_id=channel, user_id=user, boss=boss, show_past=show_past
    )


# ---------------------------------------------------------------------------
# fixed runs
# ---------------------------------------------------------------------------


@router.get("/fixed", response_model=list[FixedOut], summary="The weekly baseline timings")
async def list_fixed(bot: Bot, caller: Caller, user: str | None = None) -> list[dict]:
    return [service.fixed_view(bot, f) for f in bot.repo.list_fixed_runs(participant=user)]


@router.get("/fixed/{fixed_id}", response_model=FixedOut)
async def get_fixed(bot: Bot, caller: Caller, fixed_id: str) -> dict:
    return service.fixed_view(bot, service.load_fixed(bot, fixed_id))


@router.post("/fixed", response_model=FixedOut, status_code=201, summary="Add a weekly timing")
async def post_fixed(bot: Bot, caller: Caller, body: FixedCreate) -> dict:
    return await service.create_fixed(
        bot,
        bosses=body.bosses,
        day=body.day,
        time_hhmm=body.time,
        participants=body.participants,
        channel_id=body.channel_id,
        owner_id=body.owner_id,
        note=body.note,
    )


@router.patch("/fixed/{fixed_id}", response_model=FixedOut)
async def patch_fixed(bot: Bot, caller: Caller, fixed_id: str, body: FixedUpdate) -> dict:
    return await service.update_fixed(bot, fixed_id, **body.model_dump(exclude_unset=True))


@router.delete("/fixed/{fixed_id}", response_model=DeletedOut)
async def remove_fixed(bot: Bot, caller: Caller, fixed_id: str) -> dict:
    return await service.delete_fixed(bot, fixed_id)


@router.post("/validate/bosses", summary="Parse boss tokens without saving anything")
async def validate_bosses(bot: Bot, caller: Caller, text: str = Body(embed=True)) -> dict:
    """What ``hstar, hfa`` resolves to -- or why it doesn't."""
    try:
        tokens = service.validate_bosses(bot, text)
    except BadRequest as exc:
        return {"ok": False, "error": exc.message, "bosses": []}
    return {"ok": True, "error": None, "bosses": [service.boss_view(bot, t) for t in tokens]}


# ---------------------------------------------------------------------------
# runs
# ---------------------------------------------------------------------------


@router.get("/runs/{run_id}", response_model=RunOut)
async def get_run(bot: Bot, caller: Caller, run_id: str) -> dict:
    return service.run_view(bot, service.load_run(bot, run_id))


@router.post("/runs/{run_id}/amend", response_model=RunOut, summary="Move a run")
async def post_amend(bot: Bot, caller: Caller, run_id: str, body: AmendIn) -> dict:
    return await service.amend_run(bot, run_id, body.to)


@router.post("/runs/{run_id}/cancel", response_model=RunOut)
async def post_cancel(bot: Bot, caller: Caller, run_id: str) -> dict:
    return await service.cancel_run(bot, run_id)


@router.post("/runs/{run_id}/otot", response_model=RunOut, summary="Own time: no countdowns")
async def post_otot(bot: Bot, caller: Caller, run_id: str) -> dict:
    return await service.otot_run(bot, run_id)


@router.post(
    "/runs/{run_id}/restore",
    response_model=RunOut,
    summary="Put a cancelled or own-time run back on the schedule",
)
async def post_restore(bot: Bot, caller: Caller, run_id: str) -> dict:
    return await service.restore_run(bot, run_id)


@router.patch("/runs/{run_id}/status", response_model=RunOut, summary="Set a run's status")
async def patch_status(bot: Bot, caller: Caller, run_id: str, body: StatusIn) -> dict:
    """The one place a status is chosen; `/otot`, `/cancel` and `/restore` are shortcuts."""
    return await service.set_status(bot, run_id, body.status)


@router.patch(
    "/runs/{run_id}/participants",
    response_model=RunOut,
    summary="Swap someone in or out for this week only",
)
async def patch_participants(bot: Bot, caller: Caller, run_id: str, body: SwapIn) -> dict:
    """Changes the run, never the fixed timing behind it."""
    return await service.swap_participants(bot, run_id, remove=body.remove, add=body.add)


@router.post("/runs/{run_id}/rsvp", response_model=RunOut)
async def post_rsvp(bot: Bot, caller: Caller, run_id: str, body: RsvpIn) -> dict:
    return await service.set_rsvp(bot, run_id, body.user_id, body.answer)


@router.get("/reminders", response_model=list[ReminderOut])
async def get_reminders(
    bot: Bot, caller: Caller, run_id: str | None = None, limit: int = 200
) -> list:
    return service.reminders(bot, run_id=run_id, limit=limit)


# ---------------------------------------------------------------------------
# the inbox
# ---------------------------------------------------------------------------


@router.get("/pending", response_model=list[AmendmentOut], summary="Proposals awaiting a decision")
async def get_pending(bot: Bot, caller: Caller, channel: str | None = None) -> list[dict]:
    return service.pending(bot, channel_id=channel)


@router.get("/amendments/{amendment_id}", response_model=AmendmentOut)
async def get_amendment(bot: Bot, caller: Caller, amendment_id: str) -> dict:
    return service.amendment_view(bot, service.load_amendment(bot, amendment_id))


@router.post("/amendments/{amendment_id}/approve", response_model=ApproveOut)
async def post_approve(
    bot: Bot, caller: Caller, amendment_id: str, body: ApproveIn | None = None
) -> dict:
    return await service.approve(bot, amendment_id, actor_id=(body.actor_id if body else None))


@router.post("/amendments/{amendment_id}/reject", response_model=RejectOut)
async def post_reject(bot: Bot, caller: Caller, amendment_id: str) -> dict:
    return await service.reject_amendment(bot, amendment_id)


# ---------------------------------------------------------------------------
# the extraction log -- the prompt-tuning tool (DESIGN.md §5)
# ---------------------------------------------------------------------------


@router.get("/extractions", response_model=list[ExtractionOut])
async def get_extractions(
    bot: Bot, caller: Caller, limit: int = Query(default=25, ge=1, le=200)
) -> list:
    return [service.extraction_view(bot, e) for e in bot.repo.recent_extractions(limit)]


@router.get("/extractions/{extraction_id}", response_model=ExtractionDetailOut)
async def get_extraction(bot: Bot, caller: Caller, extraction_id: str) -> dict:
    return service.extraction_view(bot, service.load_extraction(bot, extraction_id), detail=True)


# ---------------------------------------------------------------------------
# the chat log -- what the speech pilot was asked, and what it cost
# ---------------------------------------------------------------------------


@router.get("/chat", response_model=list[ChatInteractionOut])
async def get_chat_interactions(
    bot: Bot, caller: Caller, limit: int = Query(default=50, ge=1, le=500)
) -> list:
    return service.chat_interactions(bot, limit)


@router.get("/chat/summary", response_model=ChatSummaryOut, summary="Per-model totals")
async def get_chat_summary(bot: Bot, caller: Caller) -> dict:
    return service.chat_summary(bot)


@router.get("/chat/{interaction_id}", response_model=ChatInteractionDetailOut)
async def get_chat_interaction(bot: Bot, caller: Caller, interaction_id: str) -> dict:
    return service.chat_interaction_view(
        bot, service.load_chat_interaction(bot, interaction_id), detail=True
    )


# ---------------------------------------------------------------------------
# members
# ---------------------------------------------------------------------------


@router.get("/members", response_model=list[MemberOut])
async def get_members(bot: Bot, caller: Caller, with_role: bool = True) -> list[dict]:
    return service.members(bot, with_role=with_role)


@router.patch("/members/{user_id}", response_model=MemberOut, summary="Set a member's ping level")
async def patch_member(bot: Bot, caller: Caller, user_id: str, body: MemberUpdate) -> dict:
    return service.update_member(bot, user_id, ping_level=body.ping_level)


@router.post("/members/{user_id}/nick", response_model=NickOut)
async def post_nick(bot: Bot, caller: Caller, user_id: str, body: NickIn) -> dict:
    return service.set_nick(bot, user_id, body.alias)


# ---------------------------------------------------------------------------
# config and actions
# ---------------------------------------------------------------------------


@router.get("/config", response_model=ConfigOut)
async def get_config(bot: Bot, caller: Caller) -> dict:
    return service.get_config(bot)


@router.put("/config", response_model=ConfigOut)
async def put_config(bot: Bot, caller: Caller, body: ConfigIn) -> dict:
    changes = body.model_dump(exclude_unset=True)
    if not changes:
        raise BadRequest(f"nothing to change - one of {', '.join(service.CONFIG_KEYS)}")
    result = service.get_config(bot)
    for key, value in changes.items():
        result = service.set_config(bot, key, value)
    return result


@router.get(
    "/access",
    response_model=list[AccessOut],
    summary="What the bot may do in each watched channel",
)
async def get_access(bot: Bot, caller: Caller) -> list[dict]:
    return service.access_report(bot)


@router.post("/digest", response_model=DigestOut, summary="Post the weekly digest now")
async def post_digest(bot: Bot, caller: Caller, body: DigestIn | None = None) -> dict:
    body = body or DigestIn()
    return await service.post_digest(bot, body.channel_id, week=body.week)


@router.get(
    "/rescan/targets",
    response_model=list[RescanTargetOut],
    summary="The channels a rescan can cover",
)
async def get_rescan_targets(bot: Bot, caller: Caller) -> list[dict]:
    return service.rescan_targets(bot)


@router.post(
    "/rescan",
    response_model=RescanJobOut,
    status_code=202,
    summary="Queue a re-read of the party channels",
)
async def post_rescan(bot: Bot, caller: Caller, body: RescanIn | None = None) -> dict:
    """Returns at once with a job to poll; no channels means all of them.

    Re-reading a boss week is minutes of model time, so this never blocks: the
    worker drains the queue one channel at a time while the bot carries on.
    """
    body = body or RescanIn()
    return service.queue_rescan(bot, body.channels, window=body.window)


@router.get("/rescan/{job_id}", response_model=RescanJobDetailOut, summary="A rescan's progress")
async def get_rescan_job(bot: Bot, caller: Caller, job_id: str) -> dict:
    return service.rescan_job(bot, job_id)


@router.delete("/rescan/{job_id}", response_model=RescanJobDetailOut, summary="Stop a rescan")
async def delete_rescan_job(bot: Bot, caller: Caller, job_id: str) -> dict:
    """Cancellation is cooperative: it lands between bursts, not mid-call."""
    return service.cancel_rescan(bot, job_id)


@router.get("/rescan", response_model=list[RescanJobOut], summary="The last few rescans")
async def list_rescans(bot: Bot, caller: Caller, limit: int = 5) -> list[dict]:
    return service.recent_rescans(bot, limit)


@router.post("/debug/ping", response_model=PingOut, summary="Post one test reminder now")
async def post_debug_ping(bot: Bot, caller: Caller, body: PingIn) -> dict:
    return await service.debug_ping(bot, body.run_id, body.kind)


# ---------------------------------------------------------------------------
# message export (DESIGN.md §5, "Message export")
# ---------------------------------------------------------------------------


@router.get("/messages", summary="Stream a watched channel's messages as JSONL")
async def get_messages(
    bot: Bot,
    caller: Caller,
    channel: str = Query(description="channel id; must be watched"),
    since: str = Query(description="YYYY-MM-DD or an ISO timestamp"),
    until: str | None = None,
) -> StreamingResponse:
    start = service.parse_since(bot, since, "since")
    end = service.parse_since(bot, until, "until") if until else None
    if end is not None and end <= start:
        raise BadRequest("until must be after since")
    # Raised now rather than mid-stream, where the status line has already gone.
    if not service.channel_is_watched(bot, channel):
        raise BadRequest(f"channel {channel} isn't watched - only watched channels can be exported")

    async def lines():
        async for record in service.export_messages(bot, channel, start, end):
            yield json.dumps(record, ensure_ascii=False) + "\n"

    return StreamingResponse(
        lines(),
        media_type="application/x-ndjson",
        headers={"Content-Disposition": f'attachment; filename="channel-{channel}.jsonl"'},
    )


__all__ = ["router"]
