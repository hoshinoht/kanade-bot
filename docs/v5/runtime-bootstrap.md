# v5 runtime bootstrap

`kanade` is the single Rust executable. Cargo package version `1.0.0-beta.1`
corresponds to the planned release label `1.0.0b`; Cargo does not accept the
latter as a semver version.

## Available now

The offline development server and live `serve` without Discord:

```sh
KANADE_TIMEZONE=Asia/Kuala_Lumpur kanade serve --offline
kanade serve        # live: the "Serve environment" below is required
KANADE_HEALTHCHECK_URL=http://127.0.0.1:8080/healthz kanade healthcheck
```

`kanade models check [--probe]` reads the model variables below (`KANADE_MODEL_*`, the role aliases, `KANADE_ALLOW_EXTERNAL_UNMASKED`) and calls the live gateway: it prints the catalog (alias, trust zone, whether it leaves the homelab, reasoning efforts, context, admitted concurrency) and each role's route and effective effort; `--probe` sends one fixed, member-free completion per configured role (128 tokens, 30 s) and prints `ok <ms> ms finish=<reason>`, `refused: …` or `failed: …`. The key is never printed. It exits `69` when the listing or any probe fails, `78` on a configuration error.

```text
gateway: https://kanata.example/v1 (key: set, roots: webpki)
catalog: 2 models
  sumi-structured zone=private_network homelab=stays efforts=off,minimal,low,medium,high,xhigh,max context=32768 in_flight=2
  codex-like zone=external homelab=leaves efforts=low,medium,high in_flight=8
roles:
  extraction sumi-structured effort=off route=homelab
  chat codex-like effort=off route=external refused
  rewrite (not configured)
warning: chat model codex-like leaves the homelab; its calls are refused while pseudonymization is off
probe:
  extraction sumi-structured effort=off ok 412 ms finish=stop
  chat codex-like effort=off refused: role chat routes to external model "codex-like" but pseudonymization is off
```

The admin listener binds `127.0.0.1:8080` by default. `GET /healthz` answers
`{status, mode, scheduler, storage, discord}`: offline mode reports `ok`,
`offline` and `unavailable` for the rest; live mode reports `mode: "live"`,
`storage: "ok"` when the store answers a read (else `status: "degraded"`,
`storage: "error"` and HTTP 503), and `scheduler`/`discord` `disabled` until
the gateway and delivery tick are wired (later fields may be added).
`healthcheck` accepts the exact offline document or a live one with `status`
and `storage` `ok`, ignoring extra fields.
Binds are loopback-only unless `KANADE_ALLOW_PRIVATE_BIND=1` also admits a
private address (RFC 1918, IPv4 link-local, IPv6 `fc00::/7` and `fe80::/10`)
on the internal edge network; wildcard (`0.0.0.0`, `::`) and public addresses
are always refused. Runtime configuration comes only from the process
environment; `.env` is not loaded automatically. `healthcheck` accepts only a
loopback `http://HOST:PORT/healthz` URL (or, with the opt-in, a private one:
the container's own listener address) and has a bounded timeout.

### HTTP environment

| Variable | Default | Meaning |
|---|---|---|
| `KANADE_ADMIN_BIND` | `127.0.0.1:8080` | Admin listener, loopback (or private with the opt-in). Replaces `KANADE_BIND`, which is now refused with a rename error. |
| `KANADE_PUBLIC_BIND` | unset | Public listener, same address rule; it exists only when set and must differ from the admin bind. |
| `KANADE_ALLOW_PRIVATE_BIND` | `0` | `1` lets both listeners bind a private address on an internal container network. Never wildcard or public. |
| `KANADE_EDGE_SECRET_FILE` | unset | Shared secret (≥ 32 bytes, one line) the edge sends in `X-Kanade-Edge-Auth`; requires `KANADE_TRUSTED_PROXY`. See "Edge contract". |
| `KANADE_ADMIN_HOST` | unset | Exact `host[:port]` the admin listener serves; unset accepts only `localhost`, `127.0.0.1`, `[::1]` (any port). |
| `KANADE_PUBLIC_HOST` | unset | Exact `host[:port]` of the public listener; required with `KANADE_PUBLIC_BIND`, must differ from the admin host. |
| `KANADE_TRUSTED_PROXY` | unset | IP of the edge peer; only it may supply `X-Forwarded-*`/`Forwarded` headers to admin, and `Tailscale-*` only with the edge secret. With `KANADE_EDGE_SECRET_FILE` set it is trusted only when it presents the secret. |
| `KANADE_CLOUDFLARED_PEER` | unset | IP of the cloudflared peer; only it may supply `X-Forwarded-*` and `CF-*` headers (client IP from `CF-Connecting-IP`) to public. |
| `KANADE_WEB_DIR` | unset | Web workspace root; serves `apps/admin/dist` and `apps/public/dist` (same layout as `tools/pwa-mock`). Unset serves no shell. |
| `KANADE_BOSS_DIR` | unset | Private boss art root (`portraits/`, `portraits/icon/`, `artwork/entry/`). Unset or missing art is 404. |
| `KANADE_IDENTITY_DIR` | unset | Cached `avatar.*`/`banner.*`; unset serves generated SVG stand-ins. |
| `KANADE_ADMIN_DISCORD_CLIENT_ID` | unset | Discord application id; the three Discord variables are all-or-none. |
| `KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE` | unset | File holding the client secret (one line, ≤ 4 KiB). |
| `KANADE_ADMIN_DISCORD_REDIRECT_URI` | unset | Exactly `https://KANADE_ADMIN_HOST/api/admin/auth/discord/callback` (`http:` only for loopback dev hosts); needs `KANADE_ADMIN_HOST`. |
| `KANADE_ADMIN_TOKEN_FILE` | unset | Break-glass token file (≥ 32 bytes). Changing the token ends sessions made with the old one. |
| `KANADE_ADMIN_TAILSCALE_LOGINS` | unset | Comma-separated Tailscale logins allowed to sign in via the edge; requires a non-loopback `KANADE_TRUSTED_PROXY` and `KANADE_EDGE_SECRET_FILE`. |
| `KANADE_ADMIN_SESSION_IDLE_MINUTES` | `60` | Idle timeout, 5–720, not above the absolute lifetime. |
| `KANADE_ADMIN_SESSION_ABSOLUTE_HOURS` | `12` | Absolute session lifetime, 1–168. |

Empty values count as unset. Values are never echoed in errors or logs.
Plain `KANADE_ADMIN_TOKEN` / `KANADE_ADMIN_DISCORD_CLIENT_SECRET` are refused:
secrets come only from files. `serve --offline` has no store, so it parses
these but serves sign-in routes as `503 auth_unavailable`; live `serve`
keeps sessions in its store. See `admin-api.md` "Sign-in and sessions".

### Serve environment

Parsed by `ServeConfig` (`src/runtime/config/`) for live `serve` only.
Snowflakes are canonical decimal (no sign, no leading zero, non-zero, `u64`);
lists are comma-separated.

| Variable | Default | Meaning |
|---|---|---|
| `KANADE_DISCORD_TOKEN_FILE` | required | Bot token file (one line, ≤ 4 KiB); read at startup even while the gateway is not wired, so a missing secret fails the deploy. Plain `KANADE_DISCORD_TOKEN`/`DISCORD_TOKEN` are refused by every command. |
| `KANADE_EXPECT_V4_STOPPED` | `0` | `0` or `1`. Required to be `1` only before the Discord gateway connects ("stop the v4 container first"); serve without the gateway does not check it. |
| `KANADE_GUILD_ID`, `KANADE_BOSSING_ROLE_ID` | required | Snowflakes. |
| `KANADE_ADMIN_ROLE_ID`, `KANADE_CHAT_PILOT_ROLE_ID` | unset | Snowflakes. |
| `KANADE_DEBUG_USER_IDS` | empty | Snowflake list. |
| `KANADE_DB_PATH`, `KANADE_OWNER_LOCK_DIR` | required | Absolute paths without `..`. The lock directory and the database's directory are created `0700` when absent (existing ones are never re-moded); ownership, symlink and mode checks run when the store opens. A second process on the same store is refused. |
| `KANADE_CATALOG_FILE` | `boss/bosses.yaml` | Boss catalog. |
| `KANADE_KNOWLEDGE_DIR` | unset | Boss knowledge root. |
| `KANADE_PERSONA_DIR` | `config/personas` | Persona layout root. |
| `KANADE_MODEL_BASE_URL` | unset | `https`, or `http` only to loopback, `localhost` or `host.docker.internal`; no userinfo or query. Unset disables models; the key, CA and alias variables then are refused. |
| `KANADE_MODEL_KEY_FILE`, `KANADE_MODEL_CA_FILE` | unset | Bearer key file (plain `KANADE_MODEL_KEY` is refused) and a CA file (PEM bundle or one DER certificate) that replaces the compiled webpki roots. |
| `KANADE_EXTRACT_MODEL`, `KANADE_CHAT_MODEL`, `KANADE_REWRITE_MODEL` | unset | Model aliases (printable ASCII, ≤ 200). |
| `KANADE_MODEL_PERMITS` | `2` | Concurrent model calls, 1–16. |
| `KANADE_ALLOW_EXTERNAL_UNMASKED` | `0` | `1` lets roles whose model leaves the homelab (Kanata trust zone `external` or unknown, or a `-cloud` alias) run without pseudonymization; for provider testing only. Startup warns `UNMASKED:` per such role and their model-log rows carry `guardrail.external_unmasked`. |
| `KANADE_TICK_SECONDS` | `30` | Scheduler tick, 5–300. |
| `KANADE_INSTANCE_ID` | `kanade-<random>` | ≤ 64 of `[A-Za-z0-9._-]`. |
| `KANADE_POST_CHANNEL_ID` | unset | Settings seed: snowflake. |
| `KANADE_WATCH_CHANNEL_IDS`, `KANADE_WATCH_CATEGORY_IDS` | empty | Settings seeds: snowflake lists the extractor reads. |
| `KANADE_CHAT_CATEGORY_IDS` | empty | Settings seed: Kanade chats in every channel of these categories (there is no per-channel chat list). |
| `KANADE_EXTRACTION_ENABLED`, `KANADE_CHAT_ENABLED` | unset | Settings seeds: `0` or `1`. |
| `KANADE_BOSS_WEEK_RESET_WEEKDAY` | unset | Settings seed: `mon` … `sun` (case-insensitive). |
| `KANADE_BOSS_WEEK_RESET_TIME`, `KANADE_DAY_OF_PING_TIME` | unset | Settings seeds: exactly `HH:MM`, 24-hour. |
| `KANADE_COUNTDOWN_MINUTES` | unset | Settings seed: positive whole minutes, comma-separated (stored largest first, duplicates dropped). |

Seeds apply per key only where the store has no row; an unset seed keeps
the code default (`docs/v5/api-schemas/config.json` sections). A stored row
that does not decode (or a `persona` that is not a persona id) fails startup
naming the key, never the value. `KANADE_PILOT_CHANNEL_IDS` is refused with a
pointer to `KANADE_CHAT_CATEGORY_IDS`. Other unknown `KANADE_*` variables are
ignored, as for the HTTP settings.

### Live serve (Discord not wired yet)

`serve` (`src/runtime/serve/`) reads the bot token file, opens and owns the
store, loads the catalog, knowledge and personas, resolves settings, then
serves both listeners: one shared `SchedulerWriter`, `ApiState` (schedule
policy from settings, one `GuildAccess` shared with the staff gate, guild id
for card links) and `AdminAuth` (Discord OAuth, Tailscale and break-glass per
the HTTP environment) with `GuildStaffGate` over `StoreGuildMembers`. On
`SIGINT`/`SIGTERM` it drains HTTP, then closes the store (logged
`store_closed`) so ownership is released only after SQLite closes; a startup
failure after the store opened closes it too.

Until the gateway is wired: the channel list is empty (pickers and rescan
targets show nothing); no member rows arrive, so Discord sign-in refuses
everyone as not staff (fail-closed) and the break-glass token is the way in;
`rescans` is `None` (`503`); admin writes persist but send no Discord
effects; nothing is delivered (no tick).

### Listeners

Two routers, authorized by mounting: the public router is built without any
admin route, so every `/api/admin/*`, `/__*` and `/healthz` path is a generic
`404` there for every method. Both apply, outermost first: security headers,
Host allow-list (`421 misdirected` for unknown, missing, duplicated or
mismatched absolute-form hosts), proxy trust (forwarding, Cloudflare and
Tailscale headers from any other peer are stripped before routing), then a
64 KiB declared-body limit (`413`) and a 30 s handler timeout (`503
timeout`). Errors are `ApiError` bodies `{error, message}` with generic text.

Headers match `tools/pwa-mock` (CSP, report-only Trusted Types, `nosniff`,
`no-referrer`, COOP/CORP `same-origin`, Permissions-Policy, per-path
Cache-Control) without the mock's dev-only `report-uri`; non-2xx answers are
`no-store`; `Strict-Transport-Security` is sent on the public origin only; no
CORS headers are ever sent.

| Route | Admin | Public |
|---|---|---|
| `GET /healthz` | Clients on this host only: a loopback peer or the listener's own address, whatever the trusted-proxy setting (any loopback Host name); requests the authenticated edge relays get 404 | 404 |
| `GET /api/identity`, `/identity/{avatar,banner}` | yes | yes |
| `GET /api/public/status` | 404 | `{portal: "closed"}` |
| `/api/public/*`, `/art/*` | 404 / art | `503 closed` |
| `GET /art/{portraits,icons,entry}/{key}` | file or 404 | `503 closed` |
| other `GET`/`HEAD` | the app's static files; extensionless paths get `index.html`, missing files 404 | same, public app |

Admin API handlers take the `AdminSession` extractor (`src/api/auth/`);
unmounted `/api/admin/*` paths are `404`. Static paths reach the filesystem only as plain segments (no `..`,
dotfiles, percent-encoding or backslashes) and only inside the canonical app
root, so symlinks cannot escape it.

Known gap: `axum::serve` sets no header-read timeout, so slow-header clients
are bounded by the edge/cloudflared in front of the loopback listeners until
harden-and-package revisits connection limits.

### Admin composition: what remains

`serve --offline` builds neither `AdminAuth` nor `ApiState`, so admin reads
answer `503 auth_unavailable`. Live `serve` composes both (above); still
missing: the guild's `ChannelList` (pass the gateway's `GuildCache` to
`runtime::serve::api::compose` instead of the empty list). Gateway wiring: `BotEvent::Roster` →
`api::auth::roster::on_roster_update`, `BotEvent::GuildAvailable` →
`on_guild_available(auth, access, members, owner_id, &admin_roles)` (async;
`GuildAvailable` also fires when the owner or the set of Administrator roles
changes). `RosterUpdate::Seen` carries role ids and the computed
Administrator flag from the gateway `Router`'s role-permission cache.
Preconditions before Discord admin sign-in is enabled in `serve`: both roster
handlers run on one sequential task (a concurrent `on_guild_available` could
write back a stale row over a newer `Seen`); startup roster reconciliation
refreshes members changed while the bot was offline; and deleting the
configured admin role revokes its holders (verify Discord sends member
updates, or filter stored roles against the known role set).
Rescans (A7): `ApiState.rescans` is `None` (rescan routes answer `503`)
until serve builds the extractor's `Rescans` queue (Discord `History`
backfill, `Extractor` over the governed model client, `Proposer`, `Outbox`),
spawns `Rescans::run` and passes `RescanDesk::new(RescanService::new(..))`;
serve shutdown must call `Rescans::close`.
Admin writes' notices (A4 run and timing notices, rollbacks, inbox merge and
requester notices) are written to the store's notice outbox with the change
and posted by the delivery tick's outbox drain once serve runs the tick
(`maintenance-contract.md`, *Notice outbox*). `DeliveryConfig.max_notice_age`
(default `DEFAULT_MAX_NOTICE_AGE`, 6 h; parent decision 2026-09-25) retires
older notices unsent at drain time, so the backlog written before serve
first ticks (admin edits, an import) never floods the channels; serve builds
it with that default. Still dropped until serve: the inbox's Discord card
refresh/close (and its superseded siblings' cards).
Bot token (user decision 2026-09-25): `KANADE_DISCORD_TOKEN_FILE` (e.g.
`/run/secrets/kanade_discord_token`, a Compose secret from a host file outside
the repo); plain `KANADE_DISCORD_TOKEN`/`DISCORD_TOKEN` are refused at startup.
v5 reuses the production bot application, so it must never run while v4 is
connected (one gateway session per token), and it must act only in its
configured guild: ignore every event from other guilds and register commands
per guild, never globally (the production guild keeps v4's commands).

### Edge contract (admin origin)

For the shared edge (`sites/kanade`); owned and applied by the edge owner.

- Topology: the admin listener binds a private address on the internal
  `kanade_edge` network (`KANADE_ALLOW_PRIVATE_BIND=1`,
  `KANADE_ADMIN_BIND=<kanade's address>:8080`), and `KANADE_TRUSTED_PROXY` is
  the edge container's fixed address on that network. Kanade refuses a
  loopback `KANADE_TRUSTED_PROXY` whenever Tailscale sign-in is enabled,
  because every local process shares the loopback address.
- Shared secret: at least 32 random bytes (for example `openssl rand -base64
  48`), one line, stored as a secret file on both sides; kanade reads it from
  `KANADE_EDGE_SECRET_FILE`. Rotate by replacing both files and restarting
  both services.
- On every request it forwards to kanade, the edge must:
  1. remove any client-supplied `X-Kanade-Edge-Auth`, `Tailscale-User-*`,
     `X-Forwarded-*`, `Forwarded`, `X-Real-IP` and `CF-*` headers;
  2. set `X-Kanade-Edge-Auth: <secret>`;
  3. set `Tailscale-User-Login` (and optionally `Tailscale-User-Name`) only
     from its own Tailscale `whois` of the connecting peer, and omit them when
     the peer is not a tailnet user;
  4. set `X-Forwarded-For` to the client address it saw (kanade reads the last
     entry) and pass `Host` through unchanged (`KANADE_ADMIN_HOST`).
- Kanade honours `Tailscale-User-*` only when the TCP peer is
  `KANADE_TRUSTED_PROXY` **and** `X-Kanade-Edge-Auth` matches (constant-time);
  otherwise it strips them. With the secret configured, a peer that omits it
  gets no forwarding trust either. `X-Kanade-Edge-Auth` is always stripped
  before handlers.
- A request that presents the secret must carry a parseable `X-Forwarded-For`;
  otherwise kanade answers `400 bad_forwarding` rather than falling back to the
  edge's own address, which would pool every client into one rate-limit
  bucket (a misconfigured edge fails loudly).
- Without `KANADE_EDGE_SECRET_FILE`, `KANADE_TRUSTED_PROXY` is trusted by
  address alone: any process that can connect from that address (on
  loopback, every local process) can set `X-Forwarded-For` and so choose the
  per-IP rate-limit bucket and the client IP in audit records. Configure the
  secret whenever the proxy address is shared.
- The edge must not forward `/healthz`; the container healthcheck calls kanade
  directly (loopback or its own address) without the secret. A relayed
  `/healthz` carrying the secret answers 404.
- The public origin (cloudflared) is unchanged: `CF-Connecting-IP` from
  `KANADE_CLOUDFLARED_PEER`; no identity headers are ever read there.

`SIGINT` and `SIGTERM` stop accepting work and drain HTTP requests up to
`KANADE_SHUTDOWN_TIMEOUT_SECONDS` (default 10, range 1–120). Logs are JSON and
emit only safe configuration-error descriptions, not environment values.

## Deliberate boundaries

Live `serve` runs without Discord (see "Live serve"). `ctl` and `export`
are reserved commands that return a nonzero not-implemented result.
`import v4` is the one-off testing import from a v4 snapshot
(`v4-import.md`). `serve --offline` wires no scheduler, persistence,
Discord, import/export, admin API or mutation route.

The runtime installs Rustls' `ring` provider before command processing. SQLx
and Twilight are intentionally absent until storage and Discord work needs
them, so the production graph has no SQLx MySQL/RSA path from the spike.

## Source layout

The bootstrap is organized by responsibility rather than a flat source
directory:

```text
src/
├── lib.rs
├── main.rs
├── api/
│   ├── mod.rs
│   ├── server.rs        # binding, both listeners, graceful drain
│   ├── listeners.rs     # per-origin Site policy and router assembly
│   ├── admin/           # admin-only routes (auth.rs: sign-in, session, logout)
│   ├── auth/            # sessions, CSRF, Discord OAuth, Tailscale, break-glass, staff gate
│   ├── public/mod.rs    # public-only routes (closed portal)
│   ├── guard/           # host, proxy, headers, limits
│   ├── assets.rs        # shell, art, identity
│   └── error.rs         # ApiError
├── cli/
│   ├── mod.rs
│   └── healthcheck.rs
└── runtime/
    ├── mod.rs
    ├── application.rs   # command dispatch, `/healthz` document
    ├── serve/           # live serve: store, settings, API composition, health
    ├── config.rs
    ├── error.rs
    ├── logging.rs
    └── tls.rs

tests/
├── api/                 # listeners, guards, headers, static serving
└── runtime_bootstrap/
    └── main.rs
```

`main.rs` remains the entrypoint: it installs the crypto provider, captures
process inputs, delegates to the runtime application, and maps failures to
exit codes. `lib.rs` only declares the top-level module groups. The nested
module paths are canonical; the former flat paths are not re-exported.

### Feature-boundary roadmap

Future implementation directories will be added with their first real module;
the bootstrap does not create empty placeholders:

- `bot/` — Discord client, commands, cards, and workers.
- `chat/` — conversation tools, prompts, and safety.
- `extract/` — extraction pipeline.
- `domain/` — pure scheduling rules.
- `infrastructure/` — persistence and external integrations.
- `api/` — routes, authentication, and wire types.
- `runtime/` — process start, configuration, and lifecycle.
- `cli/` — operational commands and outbound healthcheck client.
