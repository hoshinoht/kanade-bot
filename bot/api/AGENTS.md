# API and portal guide

## Request flow

- `server.py` runs uvicorn as an asyncio task inside `BossBot`; do not create a separate process or event loop for the portal.
- `app.py` assembles middleware, shared state, routers, error handling, templates, and static files. Register dynamic artwork routes before the static mount.
- `routes_api.py` adapts `/api` JSON requests; `routes_web.py` renders pages, HTMX fragments, and form redirects. Keep route handlers async for same-loop SQLite access.
- Put reusable operations and mutations in `service.py`, without FastAPI dependencies, so portal and `bossctl` share behavior.
- `models.py` owns strict Pydantic wire shapes. Raise the hierarchy in `errors.py`; app-level handlers choose JSON, HTML/fragment, or login redirect responses.

## Auth and checks

- Normal handlers receive both `deps.Bot` and `deps.Caller`. Public health, login/logout, identity images, and catalog-resolved boss artwork are deliberate exceptions.
- Focused API check: `uv run pytest -q tests/test_api_portal.py tests/test_api_routes.py tests/test_api_auth.py tests/test_api_server.py`.
- Portal contracts live in `tests/test_portal_*.py`; run the relevant file for presentation changes.
