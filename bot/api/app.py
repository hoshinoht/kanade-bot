"""Wiring: one FastAPI app around a live bot.

``create_app(bot)`` is deliberately a function of the bot rather than a module
global, so a test can build the same app over an in-memory repository and a
stand-in client.  Everything a handler needs is reachable from
``request.app.state``.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import TYPE_CHECKING

from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.openapi.docs import get_swagger_ui_html
from fastapi.responses import HTMLResponse, JSONResponse, RedirectResponse, Response
from fastapi.staticfiles import StaticFiles
from starlette.types import ASGIApp, Receive, Scope, Send

from .. import __version__, audit
from ..config import Settings
from ..portal_styles import build_stylesheet
from .auth import HEADER_LOGIN, is_local_peer
from .deps import Caller
from .errors import ApiError
from .templating import build_templates

if TYPE_CHECKING:  # pragma: no cover
    from ..client import BossBot

log = logging.getLogger(__name__)

HERE = Path(__file__).resolve().parent
TEMPLATE_DIR = HERE / "templates"
STATIC_DIR = HERE / "static"

#: The longest actor name kept. A tailnet login or a unix username; anything
#: longer is a header being used as a payload rather than as a name.
MAX_ACTOR = 64


def resolve_actor(request: Request, settings: Settings) -> audit.Actor:
    """Who to credit for this request's changes (DESIGN.md §5).

    One shared token gets everybody in, so the name has to come from somewhere
    else, and both places it can come from are headers -- which are worth
    anything at all only because of *where the socket is*:

    * ``Tailscale-User-Login``, which `tailscale serve` sets and a browser
      cannot. Read only when ``TRUST_TAILSCALE_HEADERS`` is on **and** the peer
      is this machine, exactly the pair :func:`bot.api.auth.tailscale_identity`
      requires -- a header this process would refuse to authenticate must not be
      good enough to sign somebody's name to a change either.
    * :data:`bot.audit.HEADER_BOSSCTL`, which ``bossctl`` sets from the
      operating-system user. It vouches for nothing, so it is read only over
      loopback -- where whoever sent it can already run code as that user
      anyway -- or when the tailnet header is trusted in front of it.

    Neither is a credential: the request has been authenticated by the time a
    handler runs. With nothing to go on the actor is ``token``, which is the
    honest answer -- somebody holding ADMIN_TOKEN did this.
    """
    peer = request.client.host if request.client else None
    local = is_local_peer(peer)

    login = ""
    if settings.trust_tailscale_headers and local:
        login = (request.headers.get(HEADER_LOGIN) or "").strip().lower()[:MAX_ACTOR]

    bossctl = ""
    if local or settings.trust_tailscale_headers:
        bossctl = (request.headers.get(audit.HEADER_BOSSCTL) or "").strip()[:MAX_ACTOR]

    # The header that says *how* is bossctl's; the better *name*, when a request
    # somehow carries both, is the one the tailnet vouched for.
    return audit.Actor("cli" if bossctl else "portal", login or bossctl or "token")


class ActorMiddleware:
    """Resolve the caller once per request, for :mod:`bot.audit`.

    Deliberately plain ASGI rather than a dependency: every mutating service
    function needs the actor, and threading it through thirty route handlers
    would mean a route added later could silently write anonymous rows. Set
    here, in the same task the handler then runs in, so the context variable it
    reads belongs to this request and to no other.
    """

    def __init__(self, app: ASGIApp) -> None:
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        bot = getattr(getattr(scope.get("app"), "state", None), "bot", None)
        if bot is None:  # pragma: no cover - defensive; no bot, nothing to audit
            await self.app(scope, receive, send)
            return
        token = audit.CURRENT_ACTOR.set(resolve_actor(Request(scope), bot.settings))
        try:
            await self.app(scope, receive, send)
        finally:
            audit.CURRENT_ACTOR.reset(token)


def _wants_json(request: Request) -> bool:
    """``/api`` is JSON; the portal is HTML.  HTMX fragments count as HTML."""
    return request.url.path.startswith("/api")


def create_app(bot: BossBot) -> FastAPI:
    from .routes_api import router as api_router
    from .routes_web import router as web_router

    app = FastAPI(
        title="Boss scheduler",
        description=(
            "Local control plane for the guild's boss schedule. Loopback only; "
            "reach it from the tailnet with `tailscale serve`."
        ),
        version=__version__,
        # FastAPI's own /docs and /openapi.json carry no dependencies, so they
        # are re-declared below behind the same auth as everything else.
        docs_url=None,
        redoc_url=None,
        openapi_url=None,
    )
    app.state.bot = bot
    app.state.templates = build_templates(TEMPLATE_DIR, bot)
    # Before the routers, so every handler runs with an actor resolved.
    app.add_middleware(ActorMiddleware)

    # Routes before the mount: Starlette matches in registration order, and a
    # Mount at /static would otherwise swallow /static/portraits/<boss> and
    # /static/entry/<boss>, which are served from the bind-mounted config
    # directory rather than from here.
    app.include_router(api_router)
    app.include_router(web_router)

    # Docker builds a static artifact into the image. A fresh checkout or wheel
    # deliberately has no generated file, so serve the same source bundle from
    # memory rather than requiring a frontend build before a local run.
    if not (STATIC_DIR / "portal.css").is_file():

        @app.get("/static/portal.css", include_in_schema=False)
        async def _portal_stylesheet() -> Response:
            return Response(build_stylesheet(), media_type="text/css")

    app.mount("/static", StaticFiles(directory=STATIC_DIR), name="static")

    @app.exception_handler(ApiError)
    async def _api_error(request: Request, exc: ApiError) -> Response:
        if _wants_json(request):
            return JSONResponse({"error": exc.message}, status_code=exc.status_code)
        if exc.status_code == 401:
            # A browser that has no session should be asked to sign in, not
            # shown a wall of JSON. `next` brings them back where they were.
            return RedirectResponse(f"/login?next={request.url.path}", status_code=303)
        if request.headers.get("HX-Request"):
            # An HTMX swap: give it something it can drop into the page.
            return HTMLResponse(
                app.state.templates.get_template("partials/flash.html").render(
                    kind="error", message=exc.message
                ),
                status_code=exc.status_code,
            )
        return app.state.templates.TemplateResponse(
            request,
            "error.html",
            {"status": exc.status_code, "message": exc.message},
            status_code=exc.status_code,
        )

    @app.exception_handler(RequestValidationError)
    async def _validation_error(request: Request, exc: RequestValidationError) -> Response:
        message = "; ".join(
            f"{'.'.join(str(p) for p in err['loc'][1:]) or 'body'}: {err['msg']}"
            for err in exc.errors()
        )
        return await _api_error(request, ApiError(422, message or "invalid request"))

    @app.get("/api/openapi.json", include_in_schema=False)
    async def openapi(caller: Caller) -> JSONResponse:
        return JSONResponse(app.openapi())

    @app.get("/api/docs", include_in_schema=False)
    async def docs(caller: Caller) -> HTMLResponse:
        """Swagger UI, reachable once you are signed in (the cookie goes with it)."""
        return get_swagger_ui_html(openapi_url="/api/openapi.json", title="Boss scheduler API")

    @app.get("/healthz", include_in_schema=False)
    async def healthz() -> Response:
        """Unauthenticated, and says nothing but "ok" (DESIGN.md §5)."""
        return Response("ok\n", media_type="text/plain")

    return app


__all__ = [
    "STATIC_DIR",
    "TEMPLATE_DIR",
    "ActorMiddleware",
    "create_app",
    "resolve_actor",
]
