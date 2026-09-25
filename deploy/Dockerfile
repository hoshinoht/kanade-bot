# syntax=docker/dockerfile:1
# Build from the repository root: docker build -f deploy/Dockerfile .
# The context is an allow-list (deploy/Dockerfile.dockerignore).

# Must match web/ CI (bun-version 1.4.0).
FROM oven/bun:1.4.0-slim@sha256:e0ee68d16ccb9927bf02aa7dd8fd4bf3369ee6d46da04faa72b05ce8bfd135f6 AS web
WORKDIR /web
COPY web/package.json web/bun.lock web/tsconfig.base.json ./
COPY web/packages ./packages
COPY web/apps ./apps
RUN --mount=type=cache,target=/root/.bun/install/cache \
    bun install --frozen-lockfile
RUN bun run build

# Must match rust-toolchain.toml; that file is not copied so rustup does not
# fetch its dev-only components (clippy, rustfmt) here.
FROM rust:1.98.1-slim-trixie@sha256:f47a8de237dcbb0b0ce1099901e60a89728e3d51f24e664b40e947171538ade7 AS rust
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# The cache-mounted target dir is not in the image, so copy the binary out in the same step.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release \
    && install -D -m 0555 target/release/kanade /out/kanade \
    && install -d -m 0750 /out/data

# glibc runtime (Debian 13, matching the builder) with CA roots for the
# platform TLS verifier; no shell or package manager.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:54df941ed0d06a1bd95ef5e0ce391fd8d9f94b64782dc9a60062727849ee3f97
COPY --from=rust /out/kanade /usr/local/bin/kanade
COPY --from=web /web/apps/admin/dist /app/web/apps/admin/dist
COPY --from=web /web/apps/public/dist /app/web/apps/public/dist
# Tracked catalog knowledge only; private portraits/artwork are mounted at runtime.
COPY boss/bosses.yaml /app/boss/bosses.yaml
COPY boss/knowledge /app/boss/knowledge
# Owned by the runtime user so a fresh named volume at /data inherits it.
COPY --from=rust --chown=65532:65532 /out/data /data

ENV KANADE_WEB_DIR=/app/web \
    KANADE_BOSS_DIR=/app/boss \
    KANADE_HEALTHCHECK_URL=http://127.0.0.1:8080/healthz
USER 65532:65532
WORKDIR /data

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["/usr/local/bin/kanade", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/kanade"]
CMD ["serve", "--offline"]
