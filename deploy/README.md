# Kanade v5 deployment

Container and Compose stack for the Rust runtime. Run every `docker compose`
command from this directory (`deploy/`) without `-f`: Compose then reads
`compose.yaml`, merges `compose.override.yaml` when it exists (your host's
add-ons; git-ignored, template `compose.override.example.yaml`) and reads
`.env` for host paths (git-ignored, template `.env.example`). Relative paths
in `compose.yaml` resolve from here, so `../kanade.toml` is the repository
root's file.

- [Quick start](#quick-start): one bot and its admin portal on
  `http://localhost:8080`.
- [Add-ons](#add-ons): a reverse proxy, admin Discord sign-in, the public
  portal, encrypted backups, character data, scouter estimates.
- [Operations](#operations): deploy an update, back up, restore, roll back,
  import v4 data.
- [Reference](#reference): the image, host inputs and hardening.

## Quick start

You need Docker with Compose, a Discord server you manage, and an
OpenAI-compatible model gateway (Kanata or another server) with its key.

### 1. Discord application

In the [Discord Developer Portal](https://discord.com/developers/applications):

1. **New Application**, then **Bot**: **Reset Token** and keep the token for
   step 3. Under **Privileged Gateway Intents** turn on **Server Members
   Intent** (roster sync) and **Message Content Intent** (extraction reads
   message text); without them Discord closes the gateway (4014) and the bot
   stays unhealthy. The other intents it uses (guilds, guild messages,
   reactions) need no switch.
2. **OAuth2 → URL Generator**: scopes `bot` and `applications.commands`; bot
   permissions **View Channels**, **Send Messages**, **Read Message
   History**, **Embed Links**, **Add Reactions** and **Manage Messages**.
   Open the generated URL and add the bot to your server.
3. In Discord, with **Developer Mode** on (Settings → Advanced), copy your
   server's ID and the ID of the role whose members play bossing runs.

The admin Config page reports these six permissions per watched and posting
channel.

### 2. Settings

From the repository root:

```sh
cp kanade.example.toml kanade.toml
$EDITOR kanade.toml
```

Set the uncommented keys: `runtime.timezone`, `discord.guild_id`,
`discord.bossing_role_id` and `models.base_url`. Keep the `*_file` paths as
they are: Compose mounts the secrets there. Everything else is optional
(key → variable table: `docs/v5/runtime-bootstrap.md` "Config file");
`[settings]` only seeds values the admin Config page then owns. To use a
model server other than Kanata, see `kanade.openai-compatible.example.toml`.

### 3. Secrets and the backups directory

One line per file. On a Linux host make the files readable, and the backups
directory writable, by uid 65532 (the container user); Docker Desktop maps it
to your user.

```sh
SECRETS="$HOME/.config/kanade/v5/secrets"
mkdir -p "$SECRETS" "$HOME/.config/kanade/v5/backups"
chmod 700 "$SECRETS"
(umask 077
 printf '%s\n' '<bot token>' > "$SECRETS/discord_token"
 openssl rand -base64 48 > "$SECRETS/admin_token"         # break-glass sign-in
 printf '%s\n' '<model gateway key>' > "$SECRETS/model_api_key")
```

Compose does not create the backups directory, and the bot does not start
without it. Other locations go in `deploy/.env` (`KANADE_SECRETS_DIR`,
`KANADE_BACKUP_HOST_DIR`, `KANADE_CONFIG_FILE`; see `.env.example`).

### 4. Build and start

```sh
cd deploy
docker compose build
docker compose up -d
docker compose ps                  # wait for "healthy"
docker compose logs -f bot
```

Open `http://localhost:8080` on the host and sign in with the contents of
`admin_token`. The portal is published on `127.0.0.1` only; its session
cookie is `Secure`, which browsers accept on plain `http://localhost`. Other
devices reach it through a [reverse proxy](#reverse-proxy).

Optional, once: upload the difficulty pill emojis the redesigned cards use
(`--dry-run` only lists; the upload never deletes or renames an emoji):

```sh
docker exec kanade-v5 /usr/local/bin/kanade ctl emojis
```

### What runs

The `bot` service runs live `serve`: it owns the store, serves the admin
portal and connects the Discord gateway for `discord.guild_id` only (events
from other guilds and DMs are ignored). On connect it overwrites the guild's
slash commands (never global ones), reconciles the member roster and starts
the delivery tick (reminders, digests, the notice outbox). Chat and
extraction stay off until switched on in Config. Admin alerts are
`admin_alert` lines in the container log. Never run two bots on one token:
Discord allows one gateway session per token.

The container healthcheck needs `/healthz` `status: ok`: storage answering,
the gateway `ready` and the tick `running`; a disconnect or a stalled tick
turns it unhealthy. `/healthz` answers only the container's own address, so
it is 404 through a proxy. A gateway close for a bad token (4004) or missing
privileged intents (4014) is logged once (`gateway_closed_for_good`) and the
container keeps serving the portal with `discord: closed` (unhealthy) but
never reconnects, so a restart loop cannot burn the token's IDENTIFY budget;
fix the cause, then restart it. `discord.gateway = false` runs the admin API
alone (no gateway, no tick); for the old shell-only mode set
`command: ["serve", "--offline"]`.

## Add-ons

Each add-on is off until you turn it on. Host-specific parts go in
`compose.override.yaml` (copy `compose.override.example.yaml`, keep the
parts you use, replace every placeholder), host paths in `.env`, and
settings in `kanade.toml`. Check the merged result with `docker compose
config`, then apply it with `docker compose up -d`.

### Reverse proxy

To serve the admin portal under its own https name, and to other devices,
put it behind a reverse proxy that runs in its own Compose project:

1. Give the proxy a Docker network with a fixed subnet, and the proxy a fixed
   address on it.
2. In `compose.override.yaml`, from the example: attach `bot` to that network
   (`networks: proxy:` with its `name:`) at a fixed `ipv4_address`, with an
   alias for the proxy to use; set `KANADE_ADMIN_BIND` and
   `KANADE_HEALTHCHECK_URL` to that address, `KANADE_ADMIN_HOST` to the name
   the proxy serves (the listener then answers only that `Host`), and
   `KANADE_TRUSTED_PROXY` to the proxy's address (the only peer whose
   forwarding headers count). `ports: !reset []` drops the loopback port,
   which no longer leads anywhere.
3. Proxy `https://<admin-host>/` to `<alias>:8080` with the `Host` header
   passed through unchanged, and set `X-Forwarded-For` to the client address
   (kanade reads its last entry).
4. `docker compose up -d`; `https://<admin-host>/` then returns the admin
   shell (200).

The bot stays on `kanade_admin` too. Client addresses are the proxy's unless
it forwards them, so per-IP rate limits would pool. The optional shared edge
secret (`KANADE_EDGE_SECRET_FILE`) and Tailscale identity sign-in are
specified in `docs/v5/runtime-bootstrap.md` "Edge contract (admin origin)".

### Admin Discord sign-in

Admins (the admin role, Administrator or the server owner, once the roster
is reconciled) sign in with Discord instead of the break-glass token.
Discord needs an https redirect, so this needs the
[reverse proxy](#reverse-proxy).

```sh
# 1. Developer Portal, the bot's application -> OAuth2 -> Redirects: exactly
#    https://<admin-host>/api/admin/auth/discord/callback
#    Reset Secret and save it (one line, 0600):
#    $HOME/.config/kanade/v5/secrets/discord_client_secret
# 2. compose.override.yaml, from the example: the kanade_discord_client_secret
#    secret and KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE.
# 3. kanade.toml (with admin.host, here or as KANADE_ADMIN_HOST):
#      [admin]
#      discord_client_id = "<application id>"
#      discord_redirect_uri = "https://<admin-host>/api/admin/auth/discord/callback"
docker compose up -d --force-recreate bot
```

### Public portal

The member portal is reached from the internet only through a remotely
managed Cloudflare tunnel: `cloudflared` (profile `public`, so plain `up`
never starts it) dials out over `kanade_tunnel_egress` and forwards to the
bot's public listener at `172.25.0.10:8081` on the internal `kanade_public`
network. No port is published, no reverse proxy is involved, and the public
listener mounts only public routes (`src/api/listeners.rs`): admin paths
answer 404 there. The bot trusts `CF-*`/forwarding headers only from
`172.25.0.3` (`KANADE_CLOUDFLARED_PEER`). The bot joins `kanade_public` on
every `up`; the listener stays off until `[public] bind` is set.

```sh
# 1. Cloudflare Zero Trust -> Networks -> Tunnels: create a cloudflared tunnel
#    (remotely managed). Public hostname <public-host> -> service HTTP
#    172.25.0.10:8081 (this also creates the DNS record). Keep cloudflared's
#    default Host forwarding: the bot accepts only Host <public-host>.
# 2. Token file, one line, 0600 (never an env var):
#    $HOME/.config/kanade/v5/secrets/kanade-cloudflared
#    (Compose secret kanade_cloudflared_token, read by cloudflared through
#    --token-file). Without it the cloudflared container cannot be created;
#    plain `up` and `config` do not need it.
# 3. Open the listener in kanade.toml, then recreate the bot (a bind-mounted
#    file may be replaced by an editor, so restart is not enough):
#      [public]
#      bind = "172.25.0.10:8081"
#      host = "<public-host>"
docker compose up -d --force-recreate bot
docker compose logs bot | grep server_started      # the admin address and 172.25.0.10:8081
# 4. Start the tunnel (no depends_on: it never touches the bot).
docker compose --profile public up -d cloudflared
# 5. Verify.
docker compose --profile public logs cloudflared | grep -i 'registered tunnel connection'
docker port kanade-v5-cloudflared                                     # empty
curl -sS https://<public-host>/api/public/status                     # {"portal":"closed"}
curl -s -o /dev/null -w '%{http_code}\n' https://<public-host>/api/admin/session   # 404
docker exec kanade-v5 /usr/local/bin/kanade healthcheck               # admin still ok
```

Routine deploys keep using plain `up -d`: it recreates only the bot, and the
running cloudflared reconnects to the same address (public requests fail with
502 while the bot is stopped). Pass `--profile public` to `stop`/`down` so
cloudflared is included.

Take it down: `docker compose --profile public rm -sf cloudflared`, remove
`[public] bind`/`host` from `kanade.toml` and recreate the bot, then delete
the public hostname (and its DNS record) or the whole tunnel in Cloudflare,
and the token file. The empty `kanade_public` network can stay.

#### Member sign-in (public Discord application)

Members sign in on the public origin with their own Discord application
(separate from admin sign-in), scope `identify` only. The portal is open only
while the admin Config switch `self_service.public_portal` is on **and** these
keys are set; otherwise `/api/public/status` answers `closed`.

```sh
# 1. Discord Developer Portal: a new application (no bot, no install link),
#    Public Client off. OAuth2 -> Redirects: exactly
#    https://<public-host>/api/public/auth/discord/callback
# 2. Secret file, one line, 0600 (never an env var:
#    KANADE_PUBLIC_DISCORD_CLIENT_SECRET is refused):
#    $HOME/.config/kanade/v5/secrets/public_discord_client_secret
#    plus, from the example, the public_discord_client_secret secret in
#    compose.override.yaml.
# 3. kanade.toml:
#      [public]
#      discord_client_id = "<application id>"
#      discord_client_secret_file = "/run/secrets/public_discord_client_secret"
#      discord_redirect_uri = "https://<public-host>/api/public/auth/discord/callback"
#    Lifetimes default to 30 min idle / 8 h absolute / 15 min fresh writes
#    (session_idle_minutes, session_absolute_hours, fresh_write_minutes).
docker compose up -d --force-recreate bot
# 4. Admin Config -> Self-service -> public portal on, then:
curl -sS https://<public-host>/api/public/status                     # {"portal":"open"}
```

Rotation: Reset Secret in the Developer Portal, replace the file, recreate the
bot. Turning the switch off closes sign-in at once; existing member sessions
are kept but answer `503 closed` until they expire.

### Encrypted backups

Backups are 0600 but otherwise readable by anything that can read the host
directory. With a recipients file, `kanade backup` encrypts the snapshot with
[age](https://age-encryption.org) (X25519) and writes `<name>.age`; the
plaintext copy exists only in a hidden 0700 staging directory beside it and is
deleted after encryption (or with the directory when anything fails). The
manifest stays plaintext (history head, revision, schema, `created_at`) and
is named after the encrypted file, so History → Checkpoints lists and
anchor-checks encrypted backups without decrypting them. The image has no
shell or `age` binary, so every step runs through `kanade` itself. The files
are standard age: `age -d -i <identity>` (rage, age) opens them too.

Generate the key pair once. Prefer a machine other than the backup host
(`cargo run --release -- backup keygen --out <path>` from a checkout of this
release); on the host, keep the identity in a temporary directory and move it
off straight away:

```sh
SECRETS="${KANADE_SECRETS_DIR:-$HOME/.config/kanade/v5/secrets}"
KEYS=$(mktemp -d)                                  # 0700, not the backups directory
# Writes the identity 0600 (never overwrites) and prints only the public key.
(umask 077; docker run --rm --network none --user "$(id -u):$(id -g)" \
  -v "$KEYS:/keys" kanade-v5:local \
  backup keygen --out /keys/kanade-backup.key > "$SECRETS/backup_recipients")
cat "$SECRETS/backup_recipients"                   # age1…
# Store $KEYS/kanade-backup.key (three lines) in the password manager or on
# another machine, check it is there, then:
rm -rf "$KEYS"
# Turn the feature on for every compose command (.env is git-ignored):
echo "KANADE_BACKUP_RECIPIENTS_HOST_FILE=$SECRETS/backup_recipients" >> .env
docker compose up -d --force-recreate bot          # validates the file
```

`age-keygen -o <identity>` then `age-keygen -y <identity> > backup_recipients`
works as well. More than one public key (a second, offline identity) may go in
the recipients file; any one identity decrypts. Keep the identity out of the
backups directory, the secrets directory, `deploy/`, every Compose service and
the repository: whoever holds it can read every backup. The keygen and
decrypt containers run as your own user (`--user`), so on a Linux host they
can write `$KEYS`/`$RESTORE` and read the 0600 identity, which uid 65532
could not; the bot and the `backup` service still run as uid 65532 and must
be able to read the recipients file (Docker Desktop maps it to your user).

Deploy with encryption on (steps 1–3 and 6 of
[Deploy an update](#deploy-an-update) unchanged); `.env` supplies the
recipients path to Compose:

```sh
set -o pipefail
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
BACKUPS="${KANADE_BACKUP_HOST_DIR:-$HOME/.config/kanade/v5/backups}"
RECIPIENTS="$HOME/.config/kanade/v5/secrets/backup_recipients"   # as in .env
# 4. Whole-volume tarball, encrypted on its way to disk (the image's
#    entrypoint is kanade, so the arguments start at `backup`).
(umask 077; docker run --rm -v kanade_v5_data:/data:ro alpine tar -C /data -cz . \
  | docker run --rm -i --network none \
      -v "$RECIPIENTS:/run/secrets/backup_recipients:ro" \
      kanade-v5:local backup encrypt --recipients /run/secrets/backup_recipients \
  > "$BACKUPS/kanade_v5_data-$STAMP-pre-$SHA.tar.gz.age") \
  || rm -f "$BACKUPS/kanade_v5_data-$STAMP-pre-$SHA.tar.gz.age"
# 5. SQLite snapshot: the same command; it prints `backup kanade-…sqlite.age: …`.
KANADE_BACKUP_IMAGE=kanade-v5:rollback-<old-sha> docker compose \
  --profile backup run --rm backup backup --name kanade-$STAMP-pre-$SHA.sqlite
ls -l "$BACKUPS" | tail -3                          # .tar.gz.age, .sqlite.age, .sqlite.age.manifest.json
```

An image older than encrypted backups ignores the recipients setting, so the
first time step 5 with the rollback image prints a plain `.sqlite` name.
Encrypt that pair with the new image, then delete the plaintext:

```sh
NAME=kanade-$STAMP-pre-$SHA.sqlite
(umask 077; docker run --rm -i --network none \
  -v "$RECIPIENTS:/run/secrets/backup_recipients:ro" \
  kanade-v5:local backup encrypt --recipients /run/secrets/backup_recipients \
  < "$BACKUPS/$NAME" > "$BACKUPS/$NAME.age") &&
  mv "$BACKUPS/$NAME.manifest.json" "$BACKUPS/$NAME.age.manifest.json" &&
  rm "$BACKUPS/$NAME"
```

Restoring an encrypted snapshot is under [Restore](#restore). Turn
encryption off by removing the line from `.env` (and
`backup.recipients_file`, if set, from `kanade.toml`) and recreating the bot.

### Character data and NEXON

Module `character_data` (`[modules] roster = true` and
`character_data = true` in `kanade.toml`) links roster characters to NEXON
Open API (MapleSEA) data. Its file is `/chardata/db/chardata.sqlite` on its
own volume, `kanade_v5_chardata` (0600, in a 0700 `db/` the bot creates,
since a fresh volume root may come up 0755; owner lock in `/data/run`). Its
own volume on purpose: the whole-volume tarball of `kanade_v5_data` and
`kanade backup` never hold NEXON data. Losing it loses links (members link
again). Never tar, copy or back it up.

The NEXON Open API key (one line, 0600) is optional: put its host path in
`.env` as `KANADE_NEXON_API_KEY_HOST_FILE` (for example
`$HOME/.config/kanade/v5/secrets/nexon_api_key`); Compose mounts it as the
secret `nexon_api_key`. Unset: `/dev/null` stands in and the module runs with
links refused (health `no_key`). Set but unreadable: the bot refuses to
start. The key is personal to the registering NEXON ID; never share it. If it
leaks, rotate it in the Open API portal and tell help_openapi@nexon.co.kr
within 24 hours.

```sh
docker compose up -d --force-recreate bot
```

### Scouter estimates

Clear-time estimates for admins, and for members on their own characters
(module `character_data`). The scouter sidecar (private repository, its own
Compose project, never a service here) has no authentication, so it must
stay on this host's private networks. The bot reaches it by a single-label
name on a shared network; kanade sends it roster IGNs only. Unset: estimates
are off (health `off`). No secret.

```sh
# 1. In the scouter checkout, a local compose.override.yaml (Compose loads it
#    beside its compose.yaml, so the scouter source stays untouched) joining
#    a network the bot is on (kanade_admin, or your proxy network) with the
#    alias `scouter`:
#      services:
#        scouter:
#          networks:
#            default: {}
#            kanade:
#              aliases: [scouter]
#      networks:
#        kanade:
#          name: kanade_admin
#          external: true
#    Its port stays published on 127.0.0.1 only; never proxy it. Then
#    `docker compose up -d --build --wait` there.
# 2. Set KANADE_SCOUTER_URL=http://scouter:8091 in deploy/.env (with
#    [modules] character_data = true) and recreate the bot.
docker compose up -d --force-recreate bot
# 3. Check: Config -> Character data shows Scouter "ok" (the bot probes
#    /healthz at most once a minute). "unreachable": the network or alias is
#    missing; "verification_required": run the scouter's own setup, then the
#    next successful lookup clears it.
```

Lookups wait in one line (at most 16) and run one at a time; each can take
a few minutes (the bot waits up to 190 s per sidecar call, as long as the
sidecar's own queue and lookup can take). This needs the scouter build with
its request queue and the critical-rate assumption. Results stay in the bot's
memory for 24 hours and are lost on restart; they are never written to the
store, backups or logs.

## Operations

All from `deploy/`. The bot has fixed addresses, so `docker compose run …
bot <cmd>` fails with "Address already in use" while `kanade-v5` runs: use
`docker exec kanade-v5 /usr/local/bin/kanade <cmd>` for read-only checks, and
stop the bot first for commands that own the store.

### Deploy an update

Read the release's upgrade notes in `CHANGELOG.md` first. Snapshots go into
the backups directory twice: a `kanade backup` SQLite snapshot with its
manifest (it anchors the history and shows up in History → Checkpoints) and
the whole-volume tarball (the rollback fallback). With encrypted backups on,
use the steps in [Encrypted backups](#encrypted-backups) instead of steps 4
and 5.

```sh
SHA=$(git rev-parse --short HEAD)                 # what is being deployed
# 1. Keep the running image for rollback (name it by the commit it was built from).
docker tag kanade-v5:local kanade-v5:rollback-<old-sha>
# 2. Build. With uncommitted work in the tree, build from the commit instead:
#    git -C .. archive HEAD | docker build -f deploy/Dockerfile -t kanade-v5:local -
docker compose build
# 3. Stop the bot (it owns the store).
docker compose stop bot
# 4. Whole-volume tarball (fallback), 0600: first, since it never changes the store.
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
BACKUPS="${KANADE_BACKUP_HOST_DIR:-$HOME/.config/kanade/v5/backups}"
docker run --rm -v kanade_v5_data:/data:ro -v "$BACKUPS:/backups" \
  alpine sh -c "tar -C /data -czf /backups/kanade_v5_data-$STAMP-pre-$SHA.tar.gz . && chmod 600 /backups/kanade_v5_data-$STAMP-pre-$SHA.tar.gz"
# 5. SQLite snapshot + manifest, taken with the image that last served the
#    store: opening the store applies pending migrations, so a newer image
#    would snapshot an already-migrated store.
KANADE_BACKUP_IMAGE=kanade-v5:rollback-<old-sha> docker compose \
  --profile backup run --rm backup backup --name kanade-$STAMP-pre-$SHA.sqlite
# 6. Start and check.
docker compose up -d
docker exec kanade-v5 /usr/local/bin/kanade healthcheck
```

`kanade backup` refuses (exit 69, "stop the bot first") while the bot owns
the store, refuses an existing name (exit 78) and never creates a store.
`docker compose run` (not `docker exec`) is right here: the `backup` service
has no network, so the bot's fixed addresses do not clash.

A snapshot is staged in a hidden 0700 directory in the backups directory
(`.<name>.partial-<uuid>/`, holding the plaintext copy even when encryption is
on) and published under its name only when complete. A backup killed outright
(SIGKILL, OOM, power loss) cannot clean that directory up; the next
`kanade backup` deletes every such directory once it holds the owner lock and
names them in a `backup_partials_removed` log line (`backup_partial_not_removed`
if one could not be deleted). To clear one sooner, stop the bot and run a
backup, or delete it by hand.

Upgrading to 1.0.0-beta.7 from an earlier release: Compose now runs from
`deploy/`, and the host-specific parts left `compose.yaml`. Before the first
`up`, write `compose.override.yaml` for what the old file did on your host
(a proxy network and addresses, admin host, trusted proxy, the Discord
sign-in secrets; compare `docker compose config` with the old file's), move
any keys from the retired `../.env.v5` into `kanade.toml`, and remove the
v4 stop guard from its `[discord]` table (the startup error names the key:
an unknown key refuses startup).

Images up to 2a92944 kept the character-data file in the volume root
(`/chardata/chardata.sqlite`). Before the first `up` of a newer image, with
the bot stopped, move it (and any hot journal) into `db/` inside the volume,
or the module starts with an empty file and every link is lost:

```sh
docker run --rm --user 65532:65532 -v kanade_v5_chardata:/c alpine \
  sh -c 'mkdir -m 0700 /c/db && mv /c/chardata.sqlite* /c/db/ && ls -ln /c/db'
```

A rollback image that predates `kanade backup` cannot run step 5, so step 5
can then only use the new image (`KANADE_BACKUP_IMAGE` unset). That is safe
only when the release adds no store migration. Migrations are the
`src/infrastructure/store/sqlite/migrations/*.sql` files registered in
`MIGRATIONS` in `src/infrastructure/store/sqlite/migrate.rs`; check with:

```sh
git diff --stat <old-sha> HEAD -- ../src/infrastructure/store/sqlite/migrations ../src/infrastructure/store/sqlite/migrate.rs
```

If it lists changes, do not run `kanade backup` with the new image before
the deploy: skip step 5 (the tarball from step 4 is the pre-migration copy),
or take the snapshot after the new bot has started (stop it again, run step
5 without `KANADE_BACKUP_IMAGE`, start it). Before the first real deploy, a
dry run of step 5 against the real bind mount (`--name dry-run.sqlite`, then
delete the pair) confirms the container can write there.

### Restore

Restore a SQLite snapshot (instead of the whole tarball) with `kanade
restore`, through the `backup` service (no network, the store volume and the
backups directory). Stop the bot, dry-run, then apply; use the image you will
start, i.e. with `KANADE_BACKUP_IMAGE` unset (the restore migrates the
snapshot to its schema, and refuses a snapshot or a current store newer than
itself):

```sh
docker compose stop bot
docker compose --profile backup run --rm backup \
  restore --from /backups/kanade-<stamp>-pre-<sha>.sqlite
docker compose --profile backup run --rm backup \
  restore --from /backups/kanade-<stamp>-pre-<sha>.sqlite --apply
docker compose up -d
docker exec kanade-v5 /usr/local/bin/kanade healthcheck
```

It refuses while the bot owns the store (exit 69, "stop the bot first") and
refuses a corrupt snapshot, a newer schema or a symlinked path (exit 78)
before changing anything. `--apply` keeps the previous database beside the
new one as `/data/db/kanade.sqlite.pre-restore-<stamp>` (with its `-wal`/
`-shm` files, if any) and swaps the snapshot in with one rename; delete the
side copy once the bot runs well on the restored store. A restore killed
midway can leave a hidden `/data/db/.kanade.sqlite.restore-<uuid>/` staging
directory: delete it with the bot stopped. The restored store's history ends
at the manifest's `history_head`, so later backups show `mismatch` in
Checkpoints until they are removed.

The whole-volume tarball replaces everything in `kanade_v5_data`, with the
bot stopped. It is the way back to an older image after a migration, since
an older image refuses the migrated store:

```sh
docker compose stop bot
BACKUPS="${KANADE_BACKUP_HOST_DIR:-$HOME/.config/kanade/v5/backups}"
docker run --rm -v kanade_v5_data:/data -v "$BACKUPS:/backups:ro" alpine sh -c '
  find /data -mindepth 1 -delete && tar -C /data -xzpf /backups/kanade_v5_data-<stamp>-pre-<sha>.tar.gz'
```

From an encrypted snapshot: fetch the identity into a fresh 0700 directory
outside the backups directory and run `kanade restore --identity` in a
throwaway container with no network (the identity is mounted read-only
there, never into a Compose service); it decrypts into a 0600 file in a
hidden staging directory beside the database and deletes it afterwards:

```sh
KEYS=$(mktemp -d)
# Put the identity at $KEYS/kanade-backup.key (0600), e.g. from the password manager.
BACKUPS="${KANADE_BACKUP_HOST_DIR:-$HOME/.config/kanade/v5/backups}"
docker compose stop bot
restore() {
  docker run --rm --network none \
    -e KANADE_DB_PATH=/data/db/kanade.sqlite -e KANADE_OWNER_LOCK_DIR=/data/run \
    -v kanade_v5_data:/data -v "$BACKUPS:/backups:ro" \
    -v "$KEYS/kanade-backup.key:/run/identity:ro" \
    kanade-v5:local restore --identity /run/identity \
      --from /backups/kanade-<stamp>-pre-<sha>.sqlite.age "$@"
}
restore            # dry run
restore --apply
rm -rf "$KEYS"
docker compose up -d
```

A wrong identity, a damaged or truncated file and a corrupt snapshot fail
before anything changes (exit 78); without `--identity` an `.age` file is
refused (exit 64). The container runs as uid 65532, which must be able to read
the identity: Docker Desktop maps it to your user; on a Linux host copy it
for that uid first (`docker run --rm -v "$KEYS:/keys" alpine install -m 600
-o 65532 -g 65532 /keys/kanade-backup.key /keys/restore.key` and mount
`restore.key`).

`backup decrypt --identity FILE --in FILE --out FILE` still decrypts any age
file to a new 0600 output (never overwriting `--out`; a wrong identity writes
nothing, a damaged file removes the partial output; exit 78 for each), for
the tarball fallback in particular. On a Linux host the `.age` files belong
to uid 65532 (0600), so your user cannot read them in place: copy one out
first (`docker run --rm -v "$BACKUPS:/backups:ro" -v "$RESTORE:/restore"
alpine install -m 600 -o "$(id -u)" -g "$(id -g)" /backups/<file>
/restore/`) and decrypt `--in /restore/<file>`. An encrypted tarball
(written by your shell, so readable in place) decrypts with
`RESTORE=$(mktemp -d)` and `docker run --rm --network none --user "$(id -u):$(id -g)"
-v "$KEYS/kanade-backup.key:/run/identity:ro" -v "$BACKUPS:/backups:ro" -v
"$RESTORE:/restore" kanade-v5:local backup decrypt --identity /run/identity
--in /backups/kanade_v5_data-<stamp>-pre-<sha>.tar.gz.age
--out /restore/kanade_v5_data.tar.gz`; then, with the bot stopped, replace
the whole volume from it (and `rm -rf "$RESTORE"` afterwards: plaintext
member data):

```sh
docker run --rm -v kanade_v5_data:/data -v "$RESTORE:/restore:ro" alpine sh -c '
  find /data -mindepth 1 -delete && tar -C /data -xzpf /restore/kanade_v5_data.tar.gz'
```

### Stop and roll back

```sh
docker compose stop       # or down (keeps kanade_v5_data)
```

Roll back to an earlier v5 image with its `kanade-v5:rollback-<sha>` tag and
the matching pre-deploy backup: with the bot stopped, restore the tarball
([Restore](#restore)) when the newer image migrated the store, then
`docker tag kanade-v5:rollback-<sha> kanade-v5:local` and
`docker compose up -d --no-build`.

An image older than 1.0.0-beta.7 also needs that release's `compose.yaml`,
which holds the whole host topology, and refuses to connect the gateway
without the v4 stop guard in `kanade.toml` (its 1.0.0-beta.1 upgrade notes
in `CHANGELOG.md`; add it back, newer images refuse it). Name that file with
`-f compose.yaml`: without `-f`, Compose would also merge
`compose.override.yaml`, written for the newer base file, and add its
networks, secrets and environment a second time.

```sh
docker compose stop bot
git checkout <old-sha> -- compose.yaml
docker tag kanade-v5:rollback-<old-sha> kanade-v5:local
docker compose -f compose.yaml up -d --no-build
```

Before upgrading again, `git checkout HEAD -- compose.yaml` and remove the
guard.

Never run `down -v` unless the store may be discarded.

### Import v4 data

`kanade import v4` copies v4's weekly fixed runs and recent chat/extraction
logs into the v5 store (details: `docs/v5/v4-import.md`). It reads an online
backup of the v4 database, never the live file, and takes the v5 store lock,
so the bot must be stopped.

```sh
# 1. A v4 database snapshot, e.g. from v4's data volume or a kept copy.
cp <v4-backup>.sqlite /tmp/v4-snapshot.sqlite
# 2. Stop v5, dry-run, then apply with the snapshot mounted read-only.
docker compose stop bot
docker compose run --rm --no-deps \
  -v /tmp/v4-snapshot.sqlite:/import/v4.sqlite:ro \
  bot import v4 --from /import/v4.sqlite
docker compose run --rm --no-deps \
  -v /tmp/v4-snapshot.sqlite:/import/v4.sqlite:ro \
  bot import v4 --from /import/v4.sqlite --apply
docker compose start bot
```

The dry run prints counts and skip reasons only; `--apply` is safe to repeat
(it adds nothing the second time). `--since YYYY-MM-DD` narrows the logs
below the 90-day retention. The owner lock directory (`/data/run`) must
already exist, as it must for `serve`. To rewrite logs imported by an older
mapping, run the same two commands with `--refresh-logs` added (dry run,
then `--refresh-logs --apply`); it replaces only `v4-` chat and extraction
logs.

### Operator commands

The public portal switch and the break-glass token's sessions are reachable
from the shell through the running bot's admin API (the bot keeps the only
store connection). Each run signs in with the token from
`/run/secrets/kanade_admin_token`, sends the admin host's `Host`/`Origin` to
the container's own listener and signs out at the end, also after an error;
nothing prints the token or the session cookie:

```sh
docker exec kanade-v5 /usr/local/bin/kanade portal status    # public portal switch: open|closed
docker exec kanade-v5 /usr/local/bin/kanade portal open       # or: portal close
docker exec kanade-v5 /usr/local/bin/kanade sessions list     # the token's sessions, by handle
docker exec kanade-v5 /usr/local/bin/kanade sessions revoke <handle>
docker exec kanade-v5 /usr/local/bin/kanade sessions sign-out-all
```

Exit 69 means the API was unreachable, slow or throttled (token sign-in
allows five per minute) and nothing was changed; 78 a refused token or host;
64 a refused request such as an unknown handle. Sessions signed in through
Discord or Tailscale are not listed: the API shows only the caller's own.

Rotate the break-glass token: write a new one into the secrets directory
(0600; `--force` replaces the old file through a temporary file and a
rename, never through a symlink), then recreate the bot so the bind-mounted
secret is read again. Sessions signed in with the old token end.

```sh
SECRETS="${KANADE_SECRETS_DIR:-$HOME/.config/kanade/v5/secrets}"
docker run --rm --network none --user "$(id -u):$(id -g)" -v "$SECRETS:/secrets" \
  kanade-v5:local token rotate --out /secrets/admin_token --force
docker compose up -d --force-recreate bot
```

Model logs (extraction, chat and rewrite logs, processed cached messages,
drained notices) older than 90 days are pruned by the bot itself at startup
and daily (`model_logs_pruned` log line with the counts).

## Reference

### Image

`deploy/Dockerfile` (context = repository root, allow-list in
`deploy/Dockerfile.dockerignore`):

- `oven/bun:1.4.0-slim` builds `web/apps/{admin,public}/dist`;
- `rust:1.98.1-slim-trixie` runs `cargo build --locked --release` (keep in
  step with `rust-toolchain.toml` and CI);
- runtime `gcr.io/distroless/cc-debian13:nonroot`: the binary at
  `/usr/local/bin/kanade`, the web dists under `/app/web`, the tracked boss
  catalog (`boss/bosses.yaml`) and `boss/knowledge` under `/app/boss`, the
  difficulty pills under `/app/assets/emojis`, and empty `/data` and
  `/chardata` owned by uid 65532. No shell, package manager or toolchain;
  runs as `65532:65532`.

Base images are pinned by digest; bump tag and digest together
(`docker buildx imagetools inspect <image:tag>`). Nothing private is baked in:
settings, personas, art, the store and secrets are mounted at runtime.

Every image has OCI revision, version and source labels. Set them from the
commit being built; Compose passes the same optional variables (from the
shell or `.env`) through to its build:

```sh
export KANADE_IMAGE_REVISION="$(git rev-parse HEAD)"
export KANADE_IMAGE_VERSION="$(git describe --always --dirty)"
docker compose build
# or, from the repository root: docker build -f deploy/Dockerfile \
#   --build-arg KANADE_IMAGE_REVISION="$(git rev-parse HEAD)" \
#   --build-arg KANADE_IMAGE_VERSION="$(git describe --always --dirty)" \
#   -t kanade-v5:local .
```

### Host inputs

| Input | Path | Notes |
|---|---|---|
| Settings | `../kanade.toml` (or `KANADE_CONFIG_FILE` in `.env`) | Private, git-ignored copy of the tracked `kanade.example.toml`, mounted read-only at `/config/kanade.toml` (`KANADE_CONFIG`); a missing file fails the start. Non-secret settings only. `compose.yaml`'s `environment:` fixes the listener addresses, cloudflared peer, healthcheck URL and container paths (store, files, secret files), so those keys in the file are overridden; `[public] bind`/`host` stay in the file (the public listener's opt-in). |
| Add-ons | `compose.override.yaml` | Optional, git-ignored; template `compose.override.example.yaml`. Compose merges it into every command run from `deploy/` without `-f`. |
| Compose variables | `.env` | Optional, git-ignored; template `.env.example`. Host paths and image labels only. |
| Secrets | `${KANADE_SECRETS_DIR:-$HOME/.config/kanade/v5/secrets}/` | One line per file: `discord_token`, `admin_token` (≥ 32 bytes, e.g. `openssl rand -base64 48`) and `model_api_key` (the bearer key for the model gateway). Add-ons: `discord_client_secret` (admin Discord sign-in), `public_discord_client_secret` (member sign-in), `kanade-cloudflared` (the tunnel token, profile `public` only). Docker Desktop lets uid 65532 read `0600` files; on a Linux host make them readable by uid 65532. |
| Personas | `../config/personas/` | Mounted read-only at `/config/personas`; the tracked Kanade bundle is the fallback (`config/personas/README.md`). |
| Catalog | `boss/bosses.yaml` | Tracked and built into the image at `/app/boss/bosses.yaml` with `boss/knowledge`, so a catalog change needs a rebuild. |
| Boss art | `../boss/portraits/`, `../boss/artwork/` | Private, mounted read-only over `/app/boss/*`. |
| Store | Docker volume `kanade_v5_data` | Created by Compose, mounted at `/data`. Not a bind mount (SQLite on macOS file sharing is unsafe). The database is `/data/db/kanade.sqlite` with its owner lock dir `/data/run`; serve creates both directories `0700` on first start. The bot's cached avatar and banner live in `/data/identity` (`KANADE_IDENTITY_DIR`), created `0700` by the gateway side and refreshed from Discord's CDN after each `READY`; deleting it only brings back the monogram and wash until the next refresh. |
| Backups | `${KANADE_BACKUP_HOST_DIR:-$HOME/.config/kanade/v5/backups}` | Must exist (Compose does not create it, and the bot then fails to start). Bind mounted **read-only** into `bot` at `/backups` (`KANADE_BACKUP_DIR`, listed by History → Checkpoints) and read-write only into the `backup` tool service. Docker Desktop's file sharing writes as the host user, so a `0700` directory you own works for uid 65532; on a Linux host it must be writable by uid 65532 (e.g. an ACL `setfacl -m u:65532:rwx`). The volume tarballs live here too; the listing ignores them. |
| Backup recipients | `KANADE_BACKUP_RECIPIENTS_HOST_FILE` in `.env` | Optional: [Encrypted backups](#encrypted-backups). age **public** keys only, mounted as the Compose secret `backup_recipients` into `bot` (checked at startup: a bad or empty file refuses it) and `backup`. Unset: `/dev/null` stands in and backups are plaintext. |
| Character data | Docker volume `kanade_v5_chardata`; `KANADE_NEXON_API_KEY_HOST_FILE` in `.env` | Optional: [Character data and NEXON](#character-data-and-nexon). |
| Scouter | `KANADE_SCOUTER_URL` in `.env` | Optional: [Scouter estimates](#scouter-estimates). |
| Networks | `kanade_admin` (`172.23.0.0/24`), `kanade_public` (internal, `172.25.0.0/24`), `kanade_tunnel_egress` (`172.24.0.0/24`) | Created by Compose with fixed subnets so the listeners bind fixed addresses: the bot at `172.23.0.10` (egress and the admin listener) and `172.25.0.10` (the public listener, unused until `[public] bind` is set); cloudflared (`172.25.0.3`, profile `public`) alone uses `kanade_tunnel_egress`. The subnets are not reserved: Docker can already have given an overlapping range to another network, and then Compose cannot create this one. If one overlaps a network on your host (`docker network inspect`), change it and its addresses in `compose.yaml`. |

### Hardening

Read-only root, `/tmp` tmpfs (16 MiB), all capabilities dropped,
`no-new-privileges`, 1 CPU, 512 MiB, 128 pids, JSON logs capped at 3 × 10 MB,
`restart: unless-stopped`, `stop_grace_period: 30s` (serve's fixed 25 s
shutdown budget plus a 5 s margin; `KANADE_SHUTDOWN_TIMEOUT_SECONDS`, default
10, caps only the HTTP drain inside that budget). One port is published, on
the host's loopback only: `127.0.0.1:8080` to the admin listener, which binds
only its `kanade_admin` address (a reverse proxy override drops the port).
The optional public listener binds only its `kanade_public` address, internal
(no egress). Discord and the model gateway use `kanade_admin`. Listeners
never bind a wildcard or public address. The container healthcheck calls
`kanade healthcheck` against its own address. cloudflared mirrors this
(read-only, no capabilities, uid 65532, 0.5 CPU, 256 MiB, 64 pids, metrics on
its own loopback, no auto-update).
