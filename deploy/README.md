# Kanade v5 deployment

Container and Compose stack for the Rust runtime. The admin portal is served
on the tailnet through the shared edge (`~/projects/personal/homelab/edge`,
site `sites/kanade`); the public portal is not deployed yet. The v4 rollback
stack stays in `legacy/python/deploy/`.

## Image

`deploy/Dockerfile` (context = repository root, allow-list in
`deploy/Dockerfile.dockerignore`):

- `oven/bun:1.4.0-slim` builds `web/apps/{admin,public}/dist`;
- `rust:1.98.1-slim-trixie` runs `cargo build --locked --release` (keep in
  step with `rust-toolchain.toml` and CI);
- runtime `gcr.io/distroless/cc-debian13:nonroot`: the binary at
  `/usr/local/bin/kanade`, the web dists under `/app/web`, the tracked
  `boss/knowledge` under `/app/boss`, and an empty `/data` owned by uid 65532.
  No shell, package manager or toolchain; runs as `65532:65532`.

Base images are pinned by digest; bump tag and digest together
(`docker buildx imagetools inspect <image:tag>`). Nothing private is baked in:
personas, art, the store and secrets are mounted at runtime.

## Host prerequisites

Run everything from the repository root of the live checkout; relative mounts
resolve from it.

| Input | Path | Notes |
|---|---|---|
| Settings | `.env.v5` (or `KANADE_ENV_FILE=/path`) | Non-secret env only (see `docs/v5/runtime-bootstrap.md` "Serve environment"). Required: `KANADE_TIMEZONE`, `KANADE_GUILD_ID`, `KANADE_BOSSING_ROLE_ID`, and `KANADE_MODEL_BASE_URL` (Compose always sets `KANADE_MODEL_KEY_FILE`, which is refused without it). Usually also `KANADE_ADMIN_ROLE_ID` and the settings seeds (`KANADE_POST_CHANNEL_ID`, `KANADE_WATCH_CHANNEL_IDS`, `KANADE_WATCH_CATEGORY_IDS`, `KANADE_CHAT_CATEGORY_IDS`, `KANADE_EXTRACTION_ENABLED`, `KANADE_CHAT_ENABLED`, `KANADE_BOSS_WEEK_RESET_WEEKDAY`, `KANADE_BOSS_WEEK_RESET_TIME`, `KANADE_DAY_OF_PING_TIME`, `KANADE_COUNTDOWN_MINUTES`), which apply only until the store holds a value. Compose overrides bind, host, trusted proxy, healthcheck URL and container paths, so values for those in the file are ignored. Keep the three `KANADE_ADMIN_DISCORD_*` settings all set or all unset. |
| Secrets | `${KANADE_SECRETS_DIR:-$HOME/.config/kanade/v5/secrets}/` | One line per file: `discord_token`, `admin_token` (≥ 32 bytes, e.g. `openssl rand -base64 48`), `discord_client_secret`, and `model_api_key` (a symlink to the Kanata key file v4 uses). Docker Desktop lets uid 65532 read `0600` files; on a Linux host make them readable by uid 65532. |
| Personas | `config/personas/` | Mounted read-only at `/config/personas`. |
| Catalog | `boss/bosses.yaml` | Tracked; mounted read-only at `/app/boss/bosses.yaml` (the image carries only `boss/knowledge`). |
| Boss art | `boss/portraits/`, `boss/artwork/` | Private, mounted read-only over `/app/boss/*`. |
| Store | Docker volume `kanade_v5_data` | Created by Compose, mounted at `/data`. Not a bind mount (SQLite on macOS file sharing is unsafe). The database is `/data/db/kanade.sqlite` with its owner lock dir `/data/run`; serve creates both directories `0700` on first start. |
| Edge network | `kanade_edge` (external, `192.168.97.0/24`) | Created by the v4 stack; it must exist. v5 takes `192.168.97.10` with the alias `kanade-bot`. If the network is ever recreated with another subnet, update the addresses in `compose.yaml`. |

## Build

```sh
docker compose -f deploy/compose.yaml build
# or: docker build -f deploy/Dockerfile -t kanade-v5:local .
```

## Start (cut over from v4)

v4 and v5 share the production bot token (one gateway session per token) and
the edge alias `kanade-bot`, so they never run together.

```sh
docker stop kanade-bot                           # v4
docker compose -f deploy/compose.yaml up -d
docker compose -f deploy/compose.yaml ps         # wait for "healthy"
docker compose -f deploy/compose.yaml logs -f bot
```

Smoke test from a tailnet peer: `https://kanade.hoshinoht.dev/` returns the
admin shell (200). `/healthz` is **not** a smoke test through the edge: v5
answers it only for the container's own address, so it is 404 via the edge
(v4's handover used it). Sign in with the break-glass admin token; the admin
app then works against the real (initially empty) store.

The stack runs live `serve`: it owns the store and serves the admin API, but
the Discord gateway, roster sync and delivery tick are not wired yet, so the
bot stays offline in Discord, channel pickers are empty, nothing is posted,
and Discord sign-in refuses everyone as not staff (no member data reaches
the store yet) — use the break-glass token until the gateway lands. The
container healthcheck accepts the live `/healthz` (`mode: live`, `storage:
ok`); a store that stops answering turns it unhealthy. `KANADE_EXPECT_V4_STOPPED`
is not needed until the gateway is wired. For the old shell-only mode set
`command: ["serve", "--offline"]`.

## Stop and roll back

```sh
docker compose -f deploy/compose.yaml stop       # or down (keeps kanade_v5_data)
docker start kanade-bot                          # v4 back; the edge needs no change
```

Never run `down -v` unless the v5 store may be discarded. v4 data lives in
`kanade_botdata`, which v5 never mounts.

## Hardening

Read-only root, `/tmp` tmpfs (16 MiB), all capabilities dropped,
`no-new-privileges`, 1 CPU, 512 MiB, 128 pids, JSON logs capped at 3 × 10 MB,
`restart: unless-stopped`, `stop_grace_period: 30s` (above
`KANADE_SHUTDOWN_TIMEOUT_SECONDS`, default 10; raise both together). No host
port is published: the listener binds only its `kanade_edge` address, which is
internal (no egress); Discord and Kanata egress uses the project's `default`
network. The container healthcheck calls `kanade healthcheck` against its own
address.

## Edge and sign-in

The edge needs **no change**: the site already proxies `kanade.hoshinoht.dev`
to `kanade-bot:8080` over `kanade_edge`. Admins sign in with Discord or the
break-glass token. Tailscale identity sign-in is not used (user decision
2026-09-25): the edge only sees Docker's gateway address, never the tailnet
peer, so it cannot vouch for a tailnet login. For the same reason client IPs
seen by kanade are the edge's, so per-IP rate limits pool all admins.

Discord sign-in needs the redirect
`https://kanade.hoshinoht.dev/api/admin/auth/discord/callback` registered on
the Discord application (OAuth2 → Redirects), the client id and redirect in
`.env.v5`, and the `discord_client_secret` file.
