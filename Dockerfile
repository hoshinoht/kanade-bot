# syntax=docker/dockerfile:1

# uv is pinned so the lockfile is always resolved by a known version.
FROM ghcr.io/astral-sh/uv:0.11.1 AS uv

FROM python:3.12-slim

# zoneinfo needs the system tz database (Asia/Kuala_Lumpur et al).
RUN apt-get update \
    && apt-get install -y --no-install-recommends tzdata \
    && rm -rf /var/lib/apt/lists/*

COPY --from=uv /uv /uvx /bin/

ENV UV_COMPILE_BYTECODE=1 \
    UV_LINK_MODE=copy \
    UV_PROJECT_ENVIRONMENT=/app/.venv \
    PYTHONUNBUFFERED=1 \
    PATH="/app/.venv/bin:$PATH"

WORKDIR /app

# Dependencies first: they change far less often than the source.
COPY pyproject.toml uv.lock README.md ./
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --frozen --no-dev --no-install-project

COPY bot ./bot
COPY config ./config
# The chatbot's fallback persona -- the one file from personas/ that belongs in
# the image. The *real* one is not in the image or in git: it lives in the
# host's personas/ directory, which compose bind-mounts over this path. Copied
# by name, never `COPY personas`, so a personal persona sitting beside the
# template can never be baked into a layer. Without this, a deployment that has
# not written one yet has no voice at all rather than the placeholder the
# loader promises.
COPY personas/persona.example.md ./personas/
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --frozen --no-dev

# Non-root. /app/data is created here so the container still starts when no
# volume is mounted; compose bind-mounts ./data over it in normal use.
RUN useradd --create-home --uid 10001 bossbot \
    && mkdir -p /app/data \
    && chown -R bossbot:bossbot /app
USER bossbot

HEALTHCHECK --interval=60s --timeout=10s --start-period=60s --retries=3 \
    CMD ["python", "-m", "bot.health"]

CMD ["python", "-m", "bot"]
