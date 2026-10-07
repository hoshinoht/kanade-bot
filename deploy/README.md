# Kanade v5 deployment

Container and Compose stack for the Rust runtime. The admin portal is served
on the tailnet through the shared edge (`~/projects/personal/homelab/edge`,
site `sites/kanade`); the public portal's tunnel is prepared but not started
("Public portal" below). The v4 rollback
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

Every image has OCI revision, version and source labels. Set them from the
commit being built; Compose passes the same optional variables through to its
build:

```sh
export KANADE_IMAGE_REVISION="$(git rev-parse HEAD)"
export KANADE_IMAGE_VERSION="$(git describe --always --dirty)"
export KANADE_IMAGE_SOURCE="https://github.com/hoshinoht/kanade-bot"
```

## Host prerequisites

Run everything from the repository root of the live checkout; relative mounts
resolve from it.

| Input | Path | Notes |
|---|---|---|
| Settings | `kanade.toml` (or `KANADE_CONFIG_FILE=/path`) | Private, git-ignored copy of the tracked `kanade.example.toml`, mounted read-only at `/config/kanade.toml` (`KANADE_CONFIG`); a missing file fails the start. Non-secret settings only (key → variable table: `docs/v5/runtime-bootstrap.md` "Config file"). Required: `runtime.timezone`, `discord.guild_id`, `discord.bossing_role_id`, and `models.base_url` (Compose always sets `KANADE_MODEL_KEY_FILE`, which is refused without it). Usually also `discord.admin_role_id`, `[models.*]` roles and `[[models.groups]]`, and `[settings]` (starting settings, applied only until the store holds a value). Compose's `environment:` fixes the admin bind and host, trusted proxy, cloudflared peer, healthcheck URL and container paths (store, files, secret files), so those keys in the file are overridden; `[public] bind`/`host` stay in the file (the public listener's opt-in). Keep the three `admin.discord_*` keys all set or all unset. |
| Legacy env | `.env.v5` (or `KANADE_ENV_FILE=/path`) | Optional (back-compat). Every non-empty `KANADE_*` variable in it overrides the matching `kanade.toml` key; move its settings into `kanade.toml` and delete it so there is one source. |
| Secrets | `${KANADE_SECRETS_DIR:-$HOME/.config/kanade/v5/secrets}/` | One line per file: `discord_token`, `admin_token` (≥ 32 bytes, e.g. `openssl rand -base64 48`), `discord_client_secret`, and `model_api_key` (a symlink to the Kanata key file v4 uses); `kanade-cloudflared` (the tunnel token) only for profile `public`. Docker Desktop lets uid 65532 read `0600` files; on a Linux host make them readable by uid 65532. |
| Personas | `config/personas/` | Mounted read-only at `/config/personas`. |
| Catalog | `boss/bosses.yaml` | Tracked; mounted read-only at `/app/boss/bosses.yaml` (the image carries only `boss/knowledge`). |
| Boss art | `boss/portraits/`, `boss/artwork/` | Private, mounted read-only over `/app/boss/*`. |
| Store | Docker volume `kanade_v5_data` | Created by Compose, mounted at `/data`. Not a bind mount (SQLite on macOS file sharing is unsafe). The database is `/data/db/kanade.sqlite` with its owner lock dir `/data/run`; serve creates both directories `0700` on first start. The bot's cached avatar and banner live in `/data/identity` (`KANADE_IDENTITY_DIR`), created `0700` by the gateway side and refreshed from Discord's CDN after each `READY`; deleting it only brings back the monogram and wash until the next refresh. |
| Backups | `${KANADE_BACKUP_HOST_DIR:-$HOME/.config/kanade/v5/backups}` | Must exist (Compose does not create it, and the bot then fails to start). Bind mounted **read-only** into `bot` at `/backups` (`KANADE_BACKUP_DIR`, listed by History → Checkpoints) and read-write only into the `backup` tool service. Docker Desktop's file sharing writes as the host user, so a `0700` directory you own works for uid 65532; on a Linux host it must be writable by uid 65532 (e.g. an ACL `setfacl -m u:65532:rwx`). The volume tarballs below live here too; the listing ignores them. |
| Edge network | `kanade_edge` (external, `192.168.97.0/24`) | Created by the v4 stack; it must exist. v5 takes `192.168.97.10` with the alias `kanade-bot`. If the network is ever recreated with another subnet, update the addresses in `compose.yaml`. |
| Public networks | `kanade_public` (internal, `172.25.0.0/24`), `kanade_tunnel_egress` (`172.24.0.0/24`) | Created by Compose. The bot joins `kanade_public` at `172.25.0.10` on every `up` (unused until `[public] bind` is set); only cloudflared (`172.25.0.3`, profile `public`) also joins it and alone uses `kanade_tunnel_egress`. Both subnets lie outside the engine's default address pools. |

## Build

```sh
docker compose -f deploy/compose.yaml build
# or: docker build -f deploy/Dockerfile \
#   --build-arg KANADE_IMAGE_REVISION="$(git rev-parse HEAD)" \
#   --build-arg KANADE_IMAGE_VERSION="$(git describe --always --dirty)" \
#   --build-arg KANADE_IMAGE_SOURCE="https://github.com/hoshinoht/kanade-bot" \
#   -t kanade-v5:local .
```

## Start (cut over from v4)

v4 and v5 share the production bot token (one gateway session per token) and
the edge alias `kanade-bot`, so they never run together. v5 refuses to start
its gateway unless `discord.expect_v4_stopped = true` is in `kanade.toml` (or
`KANADE_EXPECT_V4_STOPPED=1` in the environment); set it only after v4 is
stopped.

```sh
docker stop kanade-bot                           # v4
# then set discord.expect_v4_stopped = true in kanade.toml
docker compose -f deploy/compose.yaml up -d
docker compose -f deploy/compose.yaml ps         # wait for "healthy"
docker compose -f deploy/compose.yaml logs -f bot
```

Smoke test from a tailnet peer: `https://kanade.hoshinoht.dev/` returns the
admin shell (200). `/healthz` is **not** a smoke test through the edge: v5
answers it only for the container's own address, so it is 404 via the edge
(v4's handover used it). Sign in with the break-glass admin token; the admin
app then works against the real (initially empty) store.

The stack runs live `serve`: it owns the store, serves the admin API and
connects the Discord gateway for `KANADE_GUILD_ID` only (events from other
guilds and DMs are ignored). On connect it overwrites the guild's slash
commands (never global ones), reconciles the member roster, and starts the
delivery tick (reminders, digests, the notice outbox). Discord sign-in works
for members with the admin role, Administrator or ownership once the roster
is reconciled. Chat and extraction stay off. Admin alerts are `admin_alert`
lines in the container log. The container healthcheck needs `/healthz`
`status: ok`: storage answering, the gateway `ready` and the tick
`running`; a disconnect or a stalled tick turns it unhealthy. A gateway
close for a bad token (4004) or missing privileged intents (4014: enable
Server Members and Message Content on the Developer Portal's Bot page) is
logged once (`gateway_closed_for_good`) and the container keeps running the
portal with `discord: closed` (unhealthy) but never reconnects, so a restart
loop cannot burn the shared token's IDENTIFY budget; fix the cause, then
restart it. `discord.gateway = false` runs the admin
API alone (no gateway, no tick); for the old shell-only mode set
`command: ["serve", "--offline"]`.

## Deploy an update

Snapshots go into the backups directory twice: a `kanade backup` SQLite
snapshot with its manifest (it anchors the history and shows up in History →
Checkpoints) and the whole-volume tarball (the rollback fallback).

```sh
SHA=$(git rev-parse --short HEAD)                 # what is being deployed
# 1. Keep the running image for rollback (name it by the commit it was built from).
docker tag kanade-v5:local kanade-v5:rollback-<old-sha>
# 2. Build. With uncommitted work in the tree, build from the commit instead:
#    git archive HEAD | docker build -f deploy/Dockerfile -t kanade-v5:local -
docker compose -f deploy/compose.yaml build
# 3. Stop the bot (it owns the store).
docker compose -f deploy/compose.yaml stop bot
# 4. Whole-volume tarball (fallback), 0600: first, since it never changes the store.
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
BACKUPS="${KANADE_BACKUP_HOST_DIR:-$HOME/.config/kanade/v5/backups}"
docker run --rm -v kanade_v5_data:/data:ro -v "$BACKUPS:/backups" \
  alpine sh -c "tar -C /data -czf /backups/kanade_v5_data-$STAMP-pre-$SHA.tar.gz . && chmod 600 /backups/kanade_v5_data-$STAMP-pre-$SHA.tar.gz"
# 5. SQLite snapshot + manifest, taken with the image that last served the
#    store: opening the store applies pending migrations, so a newer image
#    would snapshot an already-migrated store.
KANADE_BACKUP_IMAGE=kanade-v5:rollback-<old-sha> docker compose -f deploy/compose.yaml \
  --profile backup run --rm backup backup --name kanade-$STAMP-pre-$SHA.sqlite
# 6. Start and check.
docker compose -f deploy/compose.yaml up -d
docker exec kanade-v5 /usr/local/bin/kanade healthcheck
```

`kanade backup` refuses (exit 69, "stop the bot first") while the bot owns
the store, refuses an existing name (exit 78) and never creates a store.
`docker compose run` (not `docker exec`) is right here: the `backup` service
has no network, so the bot's fixed address does not clash.

The first time, the rollback image predates `kanade backup`, so step 5 can
only use the new image (`KANADE_BACKUP_IMAGE` unset). That is safe only when
the release adds no store migration. Migrations are the
`src/infrastructure/store/sqlite/migrations/*.sql` files registered in
`MIGRATIONS` in `src/infrastructure/store/sqlite/migrate.rs`; check with:

```sh
git diff --stat <old-sha> HEAD -- src/infrastructure/store/sqlite/migrations src/infrastructure/store/sqlite/migrate.rs
```

If it lists changes, do not run `kanade backup` with the new image before
the deploy: skip step 5 (the tarball from step 4 is the pre-migration copy),
or take the snapshot after the new bot has started (stop it again, run step
5 without `KANADE_BACKUP_IMAGE`, start it). Before the first real deploy, a
dry run of step 5 against the real bind mount (`--name dry-run.sqlite`, then
delete the pair) confirms the container can write there.

Restore a SQLite snapshot (instead of the whole tarball): with the bot
stopped, move the live database and any `-wal`/`-shm` files aside, copy the
snapshot in as `/data/db/kanade.sqlite` owned by `65532:65532` with mode
`0600`, then start the image whose schema matches the manifest's
`schema_version` (a newer image migrates it forward; an older one refuses a
newer schema):

```sh
docker run --rm -v kanade_v5_data:/data -v "${KANADE_BACKUP_HOST_DIR:-$HOME/.config/kanade/v5/backups}:/backups:ro" alpine sh -c '
  mkdir /data/replaced-<stamp> && mv /data/db/kanade.sqlite* /data/replaced-<stamp>/ &&
  cp /backups/kanade-<stamp>-pre-<sha>.sqlite /data/db/kanade.sqlite &&
  chown 65532:65532 /data/db/kanade.sqlite && chmod 600 /data/db/kanade.sqlite'
```

The restored store's history ends at the manifest's `history_head`, so later
backups show `mismatch` in Checkpoints until they are removed.

## Stop and roll back

```sh
docker compose -f deploy/compose.yaml stop       # or down (keeps kanade_v5_data)
# set discord.expect_v4_stopped = false in kanade.toml so v5 cannot reconnect by accident
docker start kanade-bot                          # v4 back; the edge needs no change
```

v4 re-registers its own guild slash commands when it starts, replacing v5's.

Never run `down -v` unless the v5 store may be discarded. v4 data lives in
`kanade_botdata`, which v5 never mounts.

## Import v4 data (testing)

`kanade import v4` copies v4's weekly fixed runs and recent chat/extraction
logs into the v5 store (details: `docs/v5/v4-import.md`). It reads an online
backup of the v4 database, never the live file, and takes the v5 store lock,
so the v5 container must be stopped.

```sh
# 1. Snapshot v4 with SQLite's online backup API (v4 may keep running).
#    A recent v4 backup under data/backups/ works too.
docker exec kanade-bot python -c "import sqlite3; s = sqlite3.connect('/app/data/bot.sqlite'); d = sqlite3.connect('/app/data/backups/v4-import.sqlite'); s.backup(d); d.close()"
cp data/backups/v4-import.sqlite /tmp/v4-snapshot.sqlite
# 2. Stop v5, dry-run, then apply with the snapshot mounted read-only.
docker compose -f deploy/compose.yaml stop bot
docker compose -f deploy/compose.yaml run --rm --no-deps \
  -v /tmp/v4-snapshot.sqlite:/import/v4.sqlite:ro \
  bot import v4 --from /import/v4.sqlite
docker compose -f deploy/compose.yaml run --rm --no-deps \
  -v /tmp/v4-snapshot.sqlite:/import/v4.sqlite:ro \
  bot import v4 --from /import/v4.sqlite --apply
docker compose -f deploy/compose.yaml start bot
```

v4's `DB_PATH` is `/app/data/bot.sqlite` and `/app/data/backups` is bind
mounted to the checkout's `data/backups` (`legacy/python/deploy/compose.yaml`).
The dry run prints counts and skip reasons
only; `--apply` is safe to repeat (it adds nothing the second time).
`--since YYYY-MM-DD` narrows the logs below the 90-day retention. The owner
lock directory (`/data/run`) must already exist, as it must for `serve`.
To rewrite logs imported by an older mapping, run the same two commands
with `--refresh-logs` added (dry run, then `--refresh-logs --apply`); it
replaces only `v4-` chat and extraction logs.

## Hardening

Read-only root, `/tmp` tmpfs (16 MiB), all capabilities dropped,
`no-new-privileges`, 1 CPU, 512 MiB, 128 pids, JSON logs capped at 3 × 10 MB,
`restart: unless-stopped`, `stop_grace_period: 30s` (above
`KANADE_SHUTDOWN_TIMEOUT_SECONDS`, default 10; raise both together). No host
port is published: the admin listener binds only its `kanade_edge` address
and the optional public listener only its `kanade_public` one, both internal
(no egress); Discord and Kanata egress uses the project's `default` network.
The container healthcheck calls `kanade healthcheck` against its own
address. cloudflared mirrors this (read-only, no capabilities, uid 65532,
0.5 CPU, 256 MiB, 64 pids, metrics on its own loopback, no auto-update).

## Public portal (prepared, not started)

The public origin reaches the internet only through a remotely managed
Cloudflare tunnel: `cloudflared` (profile `public`, so plain `up` never starts
it) dials out over `kanade_tunnel_egress` and forwards to the bot's public
listener at `172.25.0.10:8081` on the internal `kanade_public` network. No
port is published, the edge is not involved, and the public listener mounts
only public routes (`src/api/listeners.rs`): admin paths answer 404 there.
The bot trusts `CF-*`/forwarding headers only from `172.25.0.3`
(`KANADE_CLOUDFLARED_PEER`). Creating the tunnel, its DNS and starting the
profile each need the owner's go-ahead.

```sh
# 1. Cloudflare Zero Trust -> Networks -> Tunnels: create a cloudflared tunnel
#    (remotely managed). Public hostname <public-host> -> service HTTP
#    172.25.0.10:8081 (this also creates the DNS record). Keep cloudflared's
#    default Host forwarding: the bot accepts only Host <public-host>.
# 2. Token file, one line, 0600 (never an env var):
#    ${KANADE_SECRETS_DIR:-$HOME/.config/kanade/v5/secrets}/kanade-cloudflared
#    (Compose secret kanade_cloudflared_token, read by cloudflared through
#    --token-file /run/secrets/kanade_cloudflared_token). Without it the cloudflared
#    container cannot be created; plain `up` and `config` do not need it.
# 3. Open the listener in kanade.toml, then recreate the bot (a bind-mounted
#    file may be replaced by an editor, so restart is not enough):
#      [public]
#      bind = "172.25.0.10:8081"
#      host = "<public-host>"
docker compose -f deploy/compose.yaml up -d --force-recreate bot
docker compose -f deploy/compose.yaml logs bot | grep server_started   # 192.168.97.10:8080 and 172.25.0.10:8081
# 4. Start the tunnel (no depends_on: it never touches the bot).
docker compose -f deploy/compose.yaml --profile public up -d cloudflared
# 5. Verify.
docker compose -f deploy/compose.yaml --profile public logs cloudflared | grep -i 'registered tunnel connection'
docker port kanade-v5-cloudflared                                     # empty
curl -sS https://<public-host>/api/public/status                     # {"portal":"closed"}
curl -s -o /dev/null -w '%{http_code}\n' https://<public-host>/api/admin/session   # 404
docker exec kanade-v5 /usr/local/bin/kanade healthcheck               # admin still ok
```

Routine deploys keep using plain `up -d`: it recreates only the bot, and the
running cloudflared reconnects to the same address (public requests fail with
502 while the bot is stopped). Pass `--profile public` to `stop`/`down` so
cloudflared is included.

Roll back: `docker compose -f deploy/compose.yaml --profile public rm -sf
cloudflared`, remove `[public] bind`/`host` from `kanade.toml` and recreate
the bot, then delete the public hostname (and its DNS record) or the whole
tunnel in Cloudflare and the token file. The empty `kanade_public` network
can stay.

## Edge and sign-in

The edge needs **no change**: the site already proxies `kanade.hoshinoht.dev`
to `kanade-bot:8080` over `kanade_edge`. Admins sign in with Discord or the
break-glass token. Tailscale identity sign-in is not used (user decision
2026-09-25): the edge only sees Docker's gateway address, never the tailnet
peer, so it cannot vouch for a tailnet login. For the same reason client IPs
seen by kanade are the edge's, so per-IP rate limits pool all admins.

Discord sign-in needs the redirect
`https://kanade.hoshinoht.dev/api/admin/auth/discord/callback` registered on
the Discord application (OAuth2 → Redirects), `admin.discord_client_id` and
`admin.discord_redirect_uri` in `kanade.toml`, and the `discord_client_secret`
file.
