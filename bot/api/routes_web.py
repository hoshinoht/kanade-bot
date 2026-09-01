"""The portal: server-rendered pages plus HTMX partials.

Every action is a real ``<form>`` with a real ``action``, and htmx only upgrades
it to an in-place swap.  With the CDN blocked (or JavaScript off) the same POST
still lands, the handler still runs, and the browser gets a redirect back to the
page it came from -- so the portal degrades instead of breaking.

Handlers here are thin: they turn form fields into the arguments
:mod:`bot.api.service` already takes, and choose between "return the fragment"
and "redirect with a message" based on the ``HX-Request`` header.
"""

from __future__ import annotations

import logging
from typing import Any
from urllib.parse import urlencode

from fastapi import APIRouter, Form, Request
from fastapi.responses import FileResponse, HTMLResponse, RedirectResponse, Response

from ..weeks import WEEKDAY_NAMES
from . import service
from .auth import (
    SESSION_COOKIE,
    SESSION_MAX_AGE,
    Identity,
    authenticate,
    issue_session,
    token_matches,
)
from .deps import Bot, Caller, get_bot
from .errors import ApiError, NotConfigured, NotFound
from .models import Week

log = logging.getLogger(__name__)

router = APIRouter(include_in_schema=False)

# ---------------------------------------------------------------------------
# rendering
# ---------------------------------------------------------------------------


def _templates(request: Request):
    return request.app.state.templates


def render(request: Request, name: str, active: str, **context: Any) -> Response:
    """One page, with the chrome every page shares."""
    bot = get_bot(request)
    base = {
        "active": active,
        "caller": getattr(request.state, "caller", None),
        "pending_count": len(bot.repo.list_amendments(status="proposed")),
        "reset_label": (
            f"{WEEKDAY_NAMES[bot.settings.reset_weekday]} "
            f"{bot.settings.reset_time.strftime('%H:%M')}"
        ),
        "flash": request.query_params.get("msg"),
        "kind": request.query_params.get("kind", "ok"),
        "message": request.query_params.get("msg"),
    }
    base.update(context)
    return _templates(request).TemplateResponse(request, name, base)


def fragment(request: Request, name: str, **context: Any) -> HTMLResponse:
    return HTMLResponse(_templates(request).get_template(name).render(**context))


def back_to(request: Request, path: str, message: str | None = None, kind: str = "ok") -> Response:
    query = urlencode({"msg": message, "kind": kind}) if message else ""
    return RedirectResponse(f"{path}{'?' if query else ''}{query}", status_code=303)


def safe_next(candidate: str | None, fallback: str = "/") -> str:
    r"""A ``next=`` value that can only point back into this portal.

    ``/login?next=https://evil.example`` would otherwise turn the sign-in page
    into an open redirect: a link that looks like the portal and lands
    somewhere else, with the token already typed. Only a path is allowed --
    one leading slash, no scheme, no protocol-relative ``//host``, no backslash
    (browsers normalise ``/\evil`` to ``//evil``), no control characters.
    """
    value = (candidate or "").strip()
    if not value.startswith("/") or value.startswith(("//", "/\\")):
        return fallback
    if any(ch in value for ch in "\r\n\t\\") or ":" in value.split("/", 2)[1][:16]:
        return fallback
    return value


def _next(request: Request, fallback: str = "/") -> str:
    """Where a non-HTMX form post should land: its own hint, then the referer."""
    referer = request.headers.get("referer") or ""
    if referer.startswith("http"):
        from urllib.parse import urlparse

        parsed = urlparse(referer)
        if parsed.path:
            return parsed.path + (f"?{parsed.query}" if parsed.query else "")
    return fallback


def _job_or_none(bot, job_id: str) -> dict | None:
    """A rescan job for the Config page, or nothing if the id is stale."""
    if not job_id:
        return None
    try:
        return service.rescan_job(bot, job_id)
    except ApiError:
        return None


def watched_channels(bot) -> list[dict]:
    """Channels a run may live in, named where the bot can see them.

    Built from the live guild when connected, and from the configured ids plus
    the channels already in use otherwise -- so the page still offers sensible
    options while the gateway is down.
    """
    found: dict[str, str] = {}
    guild = bot.get_guild(bot.settings.guild_id)
    for channel in getattr(guild, "text_channels", []) or []:
        if bot.is_watched(channel):
            found[str(channel.id)] = f"#{channel.name}"
    for cid in bot.settings.chat_channel_id_list:
        found.setdefault(str(cid), service.channel_name(bot, cid) or f"channel {cid}")
    for row in bot.repo.list_fixed_runs():
        if row["channel_id"]:
            found.setdefault(
                row["channel_id"],
                service.channel_name(bot, row["channel_id"]) or f"channel {row['channel_id']}",
            )
    return [{"id": cid, "name": name} for cid, name in sorted(found.items(), key=lambda p: p[1])]


# ---------------------------------------------------------------------------
# sign in
# ---------------------------------------------------------------------------


@router.get("/login")
async def login_form(request: Request, next: str = "/") -> Response:
    bot = get_bot(request)
    if not bot.settings.admin_token:
        raise NotConfigured()
    destination = safe_next(next)
    try:
        authenticate(request, bot.settings)
    except ApiError:
        return _templates(request).TemplateResponse(
            request, "login.html", {"next": destination, "error": None}
        )
    return RedirectResponse(destination, status_code=303)


@router.post("/login")
async def login(request: Request, token: str = Form(), next: str = Form(default="/")) -> Response:
    bot = get_bot(request)
    if not bot.settings.admin_token:
        raise NotConfigured()
    destination = safe_next(next)
    if not token_matches(token.strip(), bot.settings.admin_token):
        return _templates(request).TemplateResponse(
            request,
            "login.html",
            {"next": destination, "error": "That isn't ADMIN_TOKEN. Check .env on the host."},
            status_code=401,
        )
    response = RedirectResponse(destination, status_code=303)
    response.set_cookie(
        SESSION_COOKIE,
        issue_session(bot.settings.admin_token, Identity(who="admin token", via="cookie")),
        max_age=int(SESSION_MAX_AGE.total_seconds()),
        httponly=True,
        # Strict is what stops another site's page POSTing to 127.0.0.1:8080
        # with this cookie attached; there is no CSRF token beyond it.
        samesite="strict",
        path="/",
    )
    return response


@router.get("/logout")
async def logout() -> Response:
    response = RedirectResponse("/login", status_code=303)
    response.delete_cookie(SESSION_COOKIE, path="/")
    return response


# ---------------------------------------------------------------------------
# pages
# ---------------------------------------------------------------------------


@router.get("/")
async def week_page(
    request: Request,
    bot: Bot,
    caller: Caller,
    week: Week = "this",
    channel: str | None = None,
    user: str | None = None,
    boss: str | None = None,
    show_past: bool = False,
) -> Response:
    request.state.caller = caller
    schedule = service.schedule(
        bot, week=week, channel_id=channel, user_id=user, boss=boss, show_past=show_past
    )
    filters = {"channel": channel, "user": user, "boss": boss}

    def url_for_week(which: str) -> str:
        params = {"week": which, **filters, "show_past": "1" if show_past else ""}
        return "/?" + urlencode({k: v for k, v in params.items() if v})

    def url_toggle_past() -> str:
        params = {"week": week, **filters, "show_past": "" if show_past else "1"}
        return "/?" + urlencode({k: v for k, v in params.items() if v})

    return render(
        request,
        "week.html",
        "week",
        schedule=schedule,
        rail=service.week_rail(bot, week),
        week=week,
        channels=watched_channels(bot),
        members=service.members(bot),
        selected={"channel": channel, "user": user, "boss": boss},
        filtered=bool(channel or user or boss),
        show_past=show_past,
        url_for_week=url_for_week,
        url_toggle_past=url_toggle_past,
        status_choices=STATUS_CHOICES,
        rescan_targets=service.rescan_targets(bot),
        roster=service.members(bot),
        back=str(request.url.path) + (f"?{request.url.query}" if request.url.query else ""),
    )


@router.get("/fixed")
async def fixed_page(request: Request, bot: Bot, caller: Caller) -> Response:
    request.state.caller = caller
    return render(
        request,
        "fixed.html",
        "fixed",
        fixed=[service.fixed_view(bot, f, with_grid=True) for f in bot.repo.list_fixed_runs()],
        members=service.members(bot),
        channels=watched_channels(bot),
        grid=service.boss_grid(bot),
    )


@router.get("/inbox")
async def inbox_page(request: Request, bot: Bot, caller: Caller) -> Response:
    request.state.caller = caller
    return render(request, "inbox.html", "inbox", pending=service.pending(bot))


@router.get("/extractions")
async def extractions_page(request: Request, bot: Bot, caller: Caller, limit: int = 50) -> Response:
    request.state.caller = caller
    return render(
        request,
        "extractions.html",
        "extractions",
        extractions=[service.extraction_view(bot, e) for e in bot.repo.recent_extractions(limit)],
        config=service.get_config(bot),
    )


@router.get("/extractions/{extraction_id}")
async def extraction_page(
    request: Request, bot: Bot, caller: Caller, extraction_id: str
) -> Response:
    request.state.caller = caller
    row = service.load_extraction(bot, extraction_id)
    return render(
        request,
        "extraction.html",
        "extractions",
        extraction=service.extraction_view(bot, row, detail=True),
    )


@router.get("/chat")
async def chat_page(request: Request, bot: Bot, caller: Caller, limit: int = 50) -> Response:
    request.state.caller = caller
    return render(
        request,
        "chat.html",
        "chat",
        interactions=service.chat_interactions(bot, limit),
        summary=service.chat_summary(bot),
        config=service.get_config(bot),
    )


@router.get("/chat/{interaction_id}")
async def chat_interaction_page(
    request: Request, bot: Bot, caller: Caller, interaction_id: str
) -> Response:
    request.state.caller = caller
    row = service.load_chat_interaction(bot, interaction_id)
    return render(
        request,
        "chat_interaction.html",
        "chat",
        interaction=service.chat_interaction_view(bot, row, detail=True),
    )


@router.get("/static/portraits/{short}")
async def portrait(request: Request, bot: Bot, short: str) -> Response:
    """A boss portrait, straight off the bind-mounted config directory.

    Deliberately unauthenticated, like the stylesheet: it is a picture of a
    game boss, and gating it would mean the browser could not cache it. The
    filename never comes from the URL -- ``short`` is looked up in the boss
    table -- so this cannot be walked out of ``config/portraits``.
    """
    path = bot.bosses.portrait_path(short)
    if path is None:
        raise NotFound(f"no portrait for {short}")
    return FileResponse(
        path,
        # Long-lived but revalidated: a replaced file should show up on a
        # reload, not in a week.
        headers={"Cache-Control": "public, max-age=86400, must-revalidate"},
    )


@router.get("/access")
async def access_fragment(request: Request, bot: Bot, caller: Caller) -> HTMLResponse:
    """The permission table on its own, for the Config page's "check again"."""
    return fragment(request, "partials/access.html", access=service.access_report(bot))


@router.post("/access")
async def access_recheck(request: Request, bot: Bot, caller: Caller) -> Response:
    """The no-JavaScript path: re-render Config, which rebuilds the table."""
    return back_to(request, "/config", "Checked.")


@router.get("/bosses")
async def bosses_page(request: Request, bot: Bot, caller: Caller) -> Response:
    """The in-game boss list, with what the guild actually runs ticked."""
    request.state.caller = caller
    in_use = service.bosses_in_use(bot)
    return render(
        request,
        "bosses.html",
        "bosses",
        rows=service.boss_grid(bot, in_use),
        in_use=in_use,
        total=sum(len(row["difficulties"]) for row in service.boss_grid(bot)),
    )


@router.get("/members")
async def members_page(request: Request, bot: Bot, caller: Caller) -> Response:
    request.state.caller = caller
    return render(request, "members.html", "members", members=service.members(bot))


@router.get("/reminders")
async def reminders_page(
    request: Request, bot: Bot, caller: Caller, run_id: str | None = None
) -> Response:
    request.state.caller = caller
    rows = service.reminders(bot, run_id=run_id, limit=400)
    return render(
        request,
        "reminders.html",
        "reminders",
        upcoming=sorted([r for r in rows if not r["sent_at"]], key=lambda r: r["fire_at"]),
        sent=[r for r in rows if r["sent_at"]],
    )


@router.get("/config")
async def config_page(request: Request, bot: Bot, caller: Caller) -> Response:
    request.state.caller = caller
    return render(
        request,
        "config.html",
        "config",
        config=service.get_config(bot),
        channels=watched_channels(bot),
        access=service.access_report(bot),
        rescan_targets=service.rescan_targets(bot),
        job=_job_or_none(bot, request.query_params.get("job", "")),
        recent_rescans=service.recent_rescans(bot),
    )


# ---------------------------------------------------------------------------
# run actions
# ---------------------------------------------------------------------------


#: The status control's buttons, in the order a night actually goes.
STATUS_CHOICES = [
    ("planned", "Planned"),
    ("confirmed", "Confirmed"),
    ("otot", "Own time"),
    ("done", "Done"),
    ("cancelled", "Cancelled"),
]


def _run_fragment(
    request: Request, bot, run_id: str, back: str, answers_open: bool = False
) -> HTMLResponse:
    run = service.run_view(bot, service.load_run(bot, run_id))
    return fragment(
        request,
        "partials/run.html",
        run=run,
        back=back,
        index=0,
        status_choices=STATUS_CHOICES,
        rescan_targets=service.rescan_targets(bot),
        roster=service.members(bot),
        # The row is replaced wholesale after every action, so a panel the
        # reader opened has to be re-opened by the server or it snaps shut
        # between two answers.
        answers_open=answers_open,
    )


def _after_run_action(
    request: Request,
    bot,
    run_id: str,
    next_path: str,
    message: str,
    answers_open: bool = False,
) -> Response:
    """An HTMX caller gets the updated row; a plain form post goes back to the page."""
    destination = safe_next(next_path, "/")
    if request.headers.get("HX-Request"):
        return _run_fragment(request, bot, run_id, destination, answers_open)
    return back_to(request, destination, message)


@router.post("/runs/{run_id}/amend")
async def web_amend(
    request: Request, bot: Bot, caller: Caller, run_id: str, to: str = Form(), next: str = Form("/")
) -> Response:
    run = await service.amend_run(bot, run_id, to)
    return _after_run_action(
        request,
        bot,
        run["id"],
        next or _next(request),
        f"Moved to {run['local_day']} {run['local_time']}.",
    )


@router.post("/runs/{run_id}/cancel")
async def web_cancel(
    request: Request, bot: Bot, caller: Caller, run_id: str, next: str = Form("/")
) -> Response:
    run = await service.cancel_run(bot, run_id)
    return _after_run_action(request, bot, run["id"], next or _next(request), "Run cancelled.")


@router.post("/runs/{run_id}/otot")
async def web_otot(
    request: Request, bot: Bot, caller: Caller, run_id: str, next: str = Form("/")
) -> Response:
    run = await service.otot_run(bot, run_id)
    return _after_run_action(
        request, bot, run["id"], next or _next(request), "Own time: no countdown pings."
    )


@router.post("/runs/{run_id}/status")
async def web_status(
    request: Request,
    bot: Bot,
    caller: Caller,
    run_id: str,
    status: str = Form(),
    next: str = Form("/"),
) -> Response:
    run = await service.set_status(bot, run_id, status)
    return _after_run_action(
        request,
        bot,
        run["id"],
        safe_next(next, _next(request)),
        f"Now {run['status_label']}.",
    )


@router.post("/runs/{run_id}/restore")
async def web_restore(
    request: Request, bot: Bot, caller: Caller, run_id: str, next: str = Form("/")
) -> Response:
    run = await service.restore_run(bot, run_id)
    return _after_run_action(
        request, bot, run["id"], safe_next(next, _next(request)), "Back on the schedule."
    )


@router.post("/runs/{run_id}/participants")
async def web_swap(request: Request, bot: Bot, caller: Caller, run_id: str) -> Response:
    """Swap someone in or out for this week; the fixed timing is untouched."""
    form = await request.form()
    remove = [v for v in form.getlist("remove") if v.strip()]
    add = [v for v in form.getlist("add") if v.strip()]
    next_path = str(form.get("next") or "/")
    if not remove and not add:
        return back_to(request, safe_next(next_path), "Nobody was picked.", "error")
    run = await service.swap_participants(bot, run_id, remove=remove, add=add)
    return _after_run_action(request, bot, run["id"], next_path, "Party updated for this week.")


@router.post("/runs/{run_id}/rsvp")
async def web_rsvp(
    request: Request,
    bot: Bot,
    caller: Caller,
    run_id: str,
    user_id: str = Form(),
    answer: str = Form(),
    next: str = Form("/"),
    answers_open: str = Form(""),
) -> Response:
    """Record (or clear) one person's answer, as if they had reacted.

    Deliberately the same `service.set_rsvp` `bossctl` and the API use: it is
    what fires the card-refresh hook, so the ✅ tally on the message already in
    Discord catches up with the portal within the second.
    """
    run = await service.set_rsvp(bot, run_id, user_id, answer)
    who = service.member_name(bot, user_id)
    said = "Cleared" if answer == "clear" else "Recorded"
    return _after_run_action(
        request,
        bot,
        run["id"],
        next or _next(request),
        f"{said} {who}'s answer.",
        answers_open=bool(answers_open),
    )


@router.post("/runs/{run_id}/ping")
async def web_ping(
    request: Request,
    bot: Bot,
    caller: Caller,
    run_id: str,
    kind: str = Form("day_of"),
    next: str = Form("/"),
) -> Response:
    result = await service.debug_ping(bot, run_id, kind)
    return _after_run_action(
        request,
        bot,
        result["run_id"],
        next or _next(request),
        "Posted a 🧪 TEST ping in the run's home channel.",
    )


# ---------------------------------------------------------------------------
# fixed timings
# ---------------------------------------------------------------------------


@router.post("/validate/bosses")
async def web_validate_bosses(request: Request, bot: Bot, caller: Caller) -> HTMLResponse:
    """Live feedback while someone types boss tokens; saves nothing."""
    form = await request.form()
    text = str(form.get("bosses") or "")
    if not text.strip():
        return HTMLResponse("")
    try:
        tokens = service.validate_bosses(bot, text)
    except ApiError as exc:
        return fragment(request, "partials/bosscheck.html", error=exc.message, detail=[])
    return fragment(
        request,
        "partials/bosscheck.html",
        error=None,
        detail=[service.boss_view(bot, t) for t in tokens],
    )


def boss_field(form) -> str:
    """The bosses a fixed-run form chose, from the grid and the text fallback.

    Both are accepted and merged: the pills are the ordinary way in, the text
    box is there for someone who would rather type `hstar, hfa`.
    ``BossTable.parse`` de-duplicates, so choosing a boss both ways is fine.
    """
    picked = list(form.getlist("boss_tokens"))
    typed = str(form.get("bosses") or "").strip()
    return ", ".join([*picked, typed]) if typed else ", ".join(picked)


@router.post("/fixed/new")
async def web_fixed_new(request: Request, bot: Bot, caller: Caller) -> Response:
    form = await request.form()
    try:
        row = await service.create_fixed(
            bot,
            bosses=boss_field(form),
            day=str(form.get("day") or "mon"),
            time_hhmm=str(form.get("time") or ""),
            participants=form.getlist("participants"),
            channel_id=str(form.get("channel_id") or ""),
            note=str(form.get("note") or "") or None,
        )
    except ApiError as exc:
        return back_to(request, "/fixed", exc.message, "error")
    return back_to(
        request,
        "/fixed",
        f"Added {' + '.join(row['bosses'])} on {row['weekday_name']} {row['time']}.",
    )


@router.post("/fixed/{fixed_id}/edit")
async def web_fixed_edit(request: Request, bot: Bot, caller: Caller, fixed_id: str) -> Response:
    form = await request.form()
    people = form.getlist("participants")
    changes: dict[str, Any] = {
        "bosses": boss_field(form) or None,
        "day": str(form.get("day")) if form.get("day") not in (None, "") else None,
        "time": str(form.get("time") or "") or None,
        "note": form.get("note") if form.get("note") is not None else None,
        "participants": people or None,
    }
    try:
        row = await service.update_fixed(bot, fixed_id, **changes)
    except ApiError as exc:
        return back_to(request, "/fixed", exc.message, "error")
    return back_to(request, "/fixed", f"Saved #{row['short_id']}.")


@router.post("/fixed/{fixed_id}/delete")
async def web_fixed_delete(request: Request, bot: Bot, caller: Caller, fixed_id: str) -> Response:
    try:
        result = await service.delete_fixed(bot, fixed_id)
    except ApiError as exc:
        return back_to(request, "/fixed", exc.message, "error")
    return back_to(
        request,
        "/fixed",
        f"Removed #{result['short_id']} ({result['cancelled_runs']} upcoming run(s) cancelled).",
    )


# ---------------------------------------------------------------------------
# inbox
# ---------------------------------------------------------------------------


@router.post("/inbox/{amendment_id}/approve")
async def web_approve(request: Request, bot: Bot, caller: Caller, amendment_id: str) -> Response:
    form = await request.form()
    edited = str(form.get("to") or "").strip()
    try:
        if edited:
            # "Edit, then approve": set the time the reader actually wants, then
            # apply the change, so the card's own value never quietly wins.
            amendment = service.load_amendment(bot, amendment_id)
            bot.repo.set_amendment_datetime(amendment["id"], service.parse_when(bot, edited))
        result = await service.approve(bot, amendment_id)
    except ApiError as exc:
        return back_to(request, "/inbox", exc.message, "error")
    return back_to(request, "/inbox", f"Applied the {result['kind']}.")


@router.post("/inbox/{amendment_id}/reject")
async def web_reject(request: Request, bot: Bot, caller: Caller, amendment_id: str) -> Response:
    try:
        await service.reject_amendment(bot, amendment_id)
    except ApiError as exc:
        return back_to(request, "/inbox", exc.message, "error")
    return back_to(request, "/inbox", "Rejected.")


# ---------------------------------------------------------------------------
# members, config, actions
# ---------------------------------------------------------------------------


@router.post("/members/{user_id}/nick")
async def web_nick(
    request: Request, bot: Bot, caller: Caller, user_id: str, alias: str = Form()
) -> Response:
    try:
        result = service.set_nick(bot, user_id, alias)
    except ApiError as exc:
        return back_to(request, "/members", exc.message, "error")
    return back_to(request, "/members", f"{result['name']} is also known as {alias}.")


@router.post("/config")
async def web_config(request: Request, bot: Bot, caller: Caller) -> Response:
    form = await request.form()
    changes = {k: v for k, v in form.items() if k in service.CONFIG_KEYS}
    if not changes:
        return back_to(request, "/config", "Nothing to change.", "error")
    try:
        for key, value in changes.items():
            service.set_config(bot, key, value)
    except ApiError as exc:
        return back_to(request, "/config", exc.message, "error")
    return back_to(request, "/config", "Saved.")


@router.post("/digest")
async def web_digest(
    request: Request,
    bot: Bot,
    caller: Caller,
    week: str = Form("this"),
    channel_id: str = Form(""),
) -> Response:
    try:
        result = await service.post_digest(bot, channel_id or None, week=week)
    except ApiError as exc:
        return back_to(request, "/config", exc.message, "error")
    return back_to(request, "/config", f"Posted the {result['week']}-week digest.")


def rescan_channels_from(form) -> list[str]:
    """The channels a rescan form picked; none ticked means all of them."""
    return [c for c in form.getlist("channels") if c.strip()]


@router.post("/rescan")
async def web_rescan(request: Request, bot: Bot, caller: Caller) -> Response:
    """Queue a rescan and hand back something to watch.

    Re-reading eight party channels is minutes of model time, far longer than a
    browser will hold a form post open, so this returns immediately with a job
    to poll. Without htmx the redirect lands on Config with the same fragment
    rendered, plus a refresh link.
    """
    form = await request.form()
    window = str(form.get("window") or "week")
    try:
        job = service.queue_rescan(
            bot,
            rescan_channels_from(form),
            window=window,
            source="portal",
            requested_by=bot.portal_actor_id,
        )
    except ApiError as exc:
        return back_to(request, "/config", exc.message, "error")
    if request.headers.get("HX-Request"):
        return _job_fragment(request, bot, job["job_id"])
    return RedirectResponse(f"/config?job={job['job_id']}", status_code=303)


@router.get("/rescan/{job_id}")
async def web_rescan_progress(
    request: Request, bot: Bot, caller: Caller, job_id: str
) -> HTMLResponse:
    """The progress (or result) of a rescan, as a fragment that polls itself."""
    return _job_fragment(request, bot, job_id)


@router.post("/rescan/{job_id}/cancel")
async def web_rescan_cancel(request: Request, bot: Bot, caller: Caller, job_id: str) -> Response:
    try:
        service.cancel_rescan(bot, job_id)
    except ApiError as exc:
        return back_to(request, "/config", exc.message, "error")
    if request.headers.get("HX-Request"):
        return _job_fragment(request, bot, job_id)
    return RedirectResponse(f"/config?job={job_id}", status_code=303)


def _job_fragment(request: Request, bot, job_id: str) -> HTMLResponse:
    job = service.rescan_job(bot, job_id)
    return fragment(request, "partials/rescan_job.html", job=job, totals=job["totals"])


__all__ = ["router", "safe_next", "watched_channels"]
