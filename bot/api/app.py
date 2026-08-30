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

from .deps import Caller
from .errors import ApiError
from .jobs import JobRegistry
from .templating import build_templates

if TYPE_CHECKING:  # pragma: no cover
    from ..client import BossBot

log = logging.getLogger(__name__)

HERE = Path(__file__).resolve().parent
TEMPLATE_DIR = HERE / "templates"
STATIC_DIR = HERE / "static"


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
        version="0.3.0",
        # FastAPI's own /docs and /openapi.json carry no dependencies, so they
        # are re-declared below behind the same auth as everything else.
        docs_url=None,
        redoc_url=None,
        openapi_url=None,
    )
    app.state.bot = bot
    app.state.templates = build_templates(TEMPLATE_DIR, bot)
    # Long portal actions (a rescan of every party channel) run here and are
    # polled by the page; see bot/api/jobs.py.
    app.state.jobs = JobRegistry()

    # Routes before the mount: Starlette matches in registration order, and a
    # Mount at /static would otherwise swallow /static/portraits/<boss>, which
    # is served from the bind-mounted config directory rather than from here.
    app.include_router(api_router)
    app.include_router(web_router)
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


__all__ = ["STATIC_DIR", "TEMPLATE_DIR", "create_app"]
