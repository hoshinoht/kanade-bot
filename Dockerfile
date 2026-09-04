# syntax=docker/dockerfile:1

# Pin the package manager used by the lockfile.
FROM ghcr.io/astral-sh/uv:0.11.1 AS uv

FROM python:3.12-slim

# Required by zoneinfo.
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

# Cache dependencies before copying source.
COPY pyproject.toml uv.lock README.md ./
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --frozen --no-dev --no-install-project

COPY bot ./bot
# Build CSS before the runtime root becomes read-only.
RUN python -m bot.portal_styles
COPY config ./config
# Copy tracked fallbacks only; private prompts must not enter image layers.
COPY personas/identities/example.md ./personas/identities/example.md
COPY personas/behaviours/default.example.md ./personas/behaviours/default.example.md
COPY personas/behaviours/profiles/example.md ./personas/behaviours/profiles/example.md
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --frozen --no-dev

# Run as non-root and retain a standalone data path.
RUN useradd --create-home --uid 10001 bossbot \
    && mkdir -p /app/data \
    && chown -R bossbot:bossbot /app
USER bossbot

HEALTHCHECK --interval=60s --timeout=10s --start-period=60s --retries=3 \
    CMD ["python", "-m", "bot.health"]

CMD ["python", "-m", "bot"]
