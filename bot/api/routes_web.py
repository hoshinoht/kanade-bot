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
from fastapi.responses import HTMLResponse, RedirectResponse, Response

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
from .errors import ApiError, NotConfigured
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


def _next(request: Request, fallback: str = "/") -> str:
    """Where a non-HTMX form post should land: its own hint, then the referer."""
    referer = request.headers.get("referer") or ""
    if referer.startswith("http"):
        from urllib.parse import urlparse

        parsed = urlparse(referer)
        if parsed.path:
            return parsed.path + (f"?{parsed.query}" if parsed.query else "")
    return fallback


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
    try:
        authenticate(request, bot.settings)
    except ApiError:
        return _templates(request).TemplateResponse(
            request, "login.html", {"next": next, "error": None}
        )
    return RedirectResponse(next or "/", status_code=303)


@router.post("/login")
async def login(request: Request, token: str = Form(), next: str = Form(default="/")) -> Response:
    bot = get_bot(request)
    if not bot.settings.admin_token:
        raise NotConfigured()
    if not token_matches(token.strip(), bot.settings.admin_token):
        return _templates(request).TemplateResponse(
            request,
            "login.html",
            {"next": next, "error": "That isn't ADMIN_TOKEN. Check .env on the host."},
            status_code=401,
        )
    response = RedirectResponse(next or "/", status_code=303)
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
) -> Response:
    request.state.caller = caller
    schedule = service.schedule(bot, week=week, channel_id=channel, user_id=user, boss=boss)

    def url_for_week(which: str) -> str:
        params = {"week": which, "channel": channel, "user": user, "boss": boss}
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
        url_for_week=url_for_week,
        back=str(request.url.path) + (f"?{request.url.query}" if request.url.query else ""),
    )


@router.get("/fixed")
async def fixed_page(request: Request, bot: Bot, caller: Caller) -> Response:
    request.state.caller = caller
    return render(
        request,
        "fixed.html",
        "fixed",
        fixed=[service.fixed_view(bot, f) for f in bot.repo.list_fixed_runs()],
        members=service.members(bot),
        channels=watched_channels(bot),
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
    )


# ---------------------------------------------------------------------------
# run actions
# ---------------------------------------------------------------------------


def _run_fragment(request: Request, bot, run_id: str, back: str) -> HTMLResponse:
    run = service.run_view(bot, service.load_run(bot, run_id))
    return fragment(request, "partials/run.html", run=run, back=back, index=0)


def _after_run_action(request: Request, bot, run_id: str, next_path: str, message: str) -> Response:
    if request.headers.get("HX-Request"):
        return _run_fragment(request, bot, run_id, next_path)
    return back_to(request, next_path, message)


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
    run = service.otot_run(bot, run_id)
    return _after_run_action(
        request, bot, run["id"], next or _next(request), "Own time: no countdown pings."
    )


@router.post("/runs/{run_id}/rsvp")
async def web_rsvp(
    request: Request,
    bot: Bot,
    caller: Caller,
    run_id: str,
    user_id: str = Form(),
    answer: str = Form(),
    next: str = Form("/"),
) -> Response:
    run = await service.set_rsvp(bot, run_id, user_id, answer)
    return _after_run_action(request, bot, run["id"], next or _next(request), "Answer recorded.")


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


@router.post("/fixed/new")
async def web_fixed_new(request: Request, bot: Bot, caller: Caller) -> Response:
    form = await request.form()
    try:
        row = service.create_fixed(
            bot,
            bosses=str(form.get("bosses") or ""),
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
        "bosses": str(form.get("bosses") or "") or None,
        "day": str(form.get("day")) if form.get("day") not in (None, "") else None,
        "time": str(form.get("time") or "") or None,
        "note": form.get("note") if form.get("note") is not None else None,
        "participants": people or None,
    }
    try:
        row = service.update_fixed(bot, fixed_id, **changes)
    except ApiError as exc:
        return back_to(request, "/fixed", exc.message, "error")
    return back_to(request, "/fixed", f"Saved #{row['short_id']}.")


@router.post("/fixed/{fixed_id}/delete")
async def web_fixed_delete(request: Request, bot: Bot, caller: Caller, fixed_id: str) -> Response:
    try:
        result = service.delete_fixed(bot, fixed_id)
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


@router.post("/rescan")
async def web_rescan(
    request: Request,
    bot: Bot,
    caller: Caller,
    channel_id: str = Form(),
    hours: int = Form(24),
) -> Response:
    try:
        result = await service.rescan(bot, channel_id, hours=hours)
    except ApiError as exc:
        return back_to(request, "/config", exc.message, "error")
    if not result["asked"]:
        return back_to(
            request, "/config", f"Nothing in the last {result['hours']}h looked like scheduling."
        )
    if result.get("error"):
        return back_to(request, "/config", f"The model didn't answer: {result['error']}", "error")
    found = len(result["proposed"])
    return back_to(
        request,
        "/inbox" if found else "/config",
        f"Read {result['hours']}h in {result['latency_ms']} ms: {found} change(s) found.",
    )


__all__ = ["router", "watched_channels"]
