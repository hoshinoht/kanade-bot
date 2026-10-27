# v5 runtime bootstrap

`kanade` is the single Rust executable, version `1.0.0` (the release
label everywhere: Cargo, CHANGELOG and the `v1.0.0` git tag). The crate version
is the one source: `kanade --version` prints `kanade 1.0.0` without
reading any settings, and the `server_started` log line, the admin Account
page and the image's OCI version label show the same number; the commit is
only in the image's revision label.

## Available now

The offline development server and live `serve` (Discord gateway, roster
sync, delivery tick, the chat pilot and extraction (behind its settings
switch) for the configured guild):

```sh
kanade --version
KANADE_TIMEZONE=Asia/Kuala_Lumpur kanade serve --offline
kanade serve        # live: the "Serve environment" below is required
kanade healthcheck  # the admin listener's /healthz (KANADE_ADMIN_BIND), or --url
```

`kanade models check [--probe]` reads the model variables below (`KANADE_MODEL_*`, the role aliases and reasoning seeds); it cannot read the store while serve owns it, so its roles are what serve runs for a role with no saved alias or level (the `roles` header says so) and calls the live gateway: it prints the catalog (alias, trust zone, whether it leaves the homelab, reasoning efforts with `(off not allowed)` when the list lacks `none`, context, admitted concurrency; `<base>:<level>` variants are grouped under their base as `variants: high, low`) and each role's route and effective effort (`effort=low (configured off)` when the configured level is replaced, `effort=high (fixed)` on a variant); `--probe` sends one fixed, member-free `ping` through each configured role's governed route (128 tokens, 30 s) and prints `ok <ms> ms finish=<reason>`, `refused: …` or `failed: …`. A probe sends no member data; it is not a privacy test. The key is never printed. It exits `69` when the listing or any probe fails, `78` on a configuration error.

```text
gateway: https://kanata.example/v1 (key: set, roots: webpki)
catalog: 5 models
  sumi-structured zone=private_network homelab=stays efforts=off,minimal,low,medium,high,xhigh,max context=32768 in_flight=2
  codex-like zone=external homelab=leaves efforts=low,medium,high (off not allowed) in_flight=8
  gpt-6-luna zone=external homelab=leaves efforts=low,medium,high (off not allowed) variants: high, low, medium
roles (env seeds; saved settings are not read):
  extraction sumi-structured effort=off route=homelab
  chat codex-like effort=low (configured off) route=external_unmasked
  rewrite (not configured)
warning: chat reasoning off is not allowed: codex-like requires reasoning; sending low
warning: UNMASKED: chat model codex-like leaves the homelab; raw member names, IDs, messages, and complete URLs are sent to the provider. A provider's retention terms (such as zero data retention) do not prevent transmission.
probe:
  extraction sumi-structured effort=off ok 412 ms finish=stop
  chat codex-like effort=low ok 412 ms finish=stop
```

The admin listener binds `127.0.0.1:8080` by default. `GET /healthz` answers
`{status, mode, scheduler, storage, discord}`: offline mode reports `ok`,
`offline` and `unavailable` for the rest; live mode reports `mode: "live"`,
`storage: "ok"` when the store answers a read (else `storage: "error"`),
`discord` (`connecting`, `ready`, `disconnected`, `closed`) with `dropped_events:
{other_guild, no_guild}`, and `scheduler` (`starting` until the guild is
available and restart recovery ran, `running`, `stalled` after three tick
periods plus five minutes without a completed tick, `stopped`) with
`last_tick_age_seconds`, and `chat` (`disabled` when `chatbot.enabled` is off
or chat never started, `idle`, `busy` while any channel is answering,
`degraded` when enabled but without a chat model route, an active persona,
the pilot role or a chat category, or while the clean-retry storm guard is
suspended; chat never decides `status`). `status` is `ok` (HTTP 200) only when storage is ok,
Discord is `ready` and the scheduler `running`; otherwise `degraded` (503).
With `web` alone (`KANADE_COMPONENTS=web`), `scheduler`/`discord` are
`disabled` and only storage decides; with `bot` alone the admin listener
serves this document and nothing else. The live gateway also reports `extraction`
(informational, never degrades `status`): `disabled` (switched off or
paused), `degraded` (on, but no model gateway or extraction model, or the
latest extraction call `failed` or was `turned_away`), `running` (a rescan
job is queued or running) or `idle`.
`healthcheck` accepts the exact offline document or a live one with `status`
and `storage` `ok`, ignoring extra fields.
Binds are loopback-only unless `KANADE_ALLOW_PRIVATE_BIND=1` also admits a
private address (RFC 1918, IPv4 link-local, IPv6 `fc00::/7` and `fe80::/10`)
on a container network; wildcard (`0.0.0.0`, `::`) and public addresses
are always refused. Runtime configuration comes from the process
environment and, when `KANADE_CONFIG` names one, the `kanade.toml` file
("Config file" below); `.env` is not loaded automatically. `healthcheck`
targets `http://<KANADE_ADMIN_BIND>/healthz` (the same address rule), or a
loopback `--url http://HOST:PORT/healthz` (with the opt-in, a private one),
and has a bounded timeout.

### HTTP environment

| Variable | Default | Meaning |
|---|---|---|
| `KANADE_ADMIN_BIND` | `127.0.0.1:8080` | Admin listener, loopback (or private with the opt-in). Replaces `KANADE_BIND`, which is now refused with a rename error. |
| `KANADE_PUBLIC_BIND` | unset | Public listener, same address rule; used only while `KANADE_PUBLIC_HOST` is set (the host turns the listener on; Compose always sets `172.25.0.10:8081`), then required and must differ from the admin bind. Requires `KANADE_CLOUDFLARED_PEER` then. |
| `KANADE_ALLOW_PRIVATE_BIND` | `0` | `1` lets both listeners bind a private address on an internal container network. Never wildcard or public. |
| `KANADE_SECRETS_DIR` | `/run/secrets` | The secrets directory ("Secrets directory" below). |
| `KANADE_SAVED_SECRETS_DIR` | unset (image: `/data/secrets`) | Where admin Setup saves secrets; unset, nothing can be saved there ("Secrets directory" below). |
| `KANADE_SUPERVISED` | `0` (Compose: `1`) | Something restarts the process after it exits, so Setup offers "Restart now". |
| `KANADE_ADMIN_HOST` | unset | Exact `host[:port]` the admin listener serves; unset accepts only `localhost`, `127.0.0.1`, `[::1]` (any port). |
| `KANADE_PUBLIC_HOST` | unset | Exact `host[:port]` of the public listener; set, it turns the listener on (needs `KANADE_PUBLIC_BIND`); must differ from the admin host. |
| `KANADE_TRUSTED_PROXY` | unset | IP of the edge peer; only it may supply `X-Forwarded-*`/`Forwarded` headers to admin, and `Tailscale-*` only with the edge secret. With the secrets file `edge_secret` present (≥ 32 bytes) it is trusted only when it presents that secret in `X-Kanade-Edge-Auth`. See "Edge contract". |
| `KANADE_CLOUDFLARED_PEER` | unset | IP of the cloudflared peer; only it may supply `X-Forwarded-*` and `CF-*` headers (client IP from `CF-Connecting-IP`) to public. Required with `KANADE_PUBLIC_BIND`: without it every member would be the proxy's address (pooled sign-in limits, no client-change rotation), so startup refuses. A request from the peer without `CF-Connecting-IP` counts as the peer itself, so local runs and tests with no proxy name their own loopback (`127.0.0.1`). Compose fixes `172.25.0.3`, which also covers the tailnet test stage. |
| `KANADE_WEB_DIR` | unset | Web workspace root (`web/`); serves `apps/admin/dist` and `apps/public/dist`. Unset serves no shell. |
| `KANADE_BOSS_DIR` | unset | Private boss art root (`portraits/`, `portraits/icon/`, `artwork/entry/`). Unset or missing art is 404. |
| `KANADE_IDENTITY_DIR` | unset | Cached `avatar.*`/`banner.*`; unset serves generated SVG stand-ins. With the gateway on, serve creates it (`0700`) and, after each `READY` and on the bot's own nickname/avatar/user changes, fetches the guild avatar (else user avatar) and banner from Discord's CDN (png/webp/gif/jpeg, ≤ 8 MiB, 20 s timeout, no redirects), replacing files by temp + rename; no avatar/banner removes the file (monogram/wash). Logs `identity_cached {avatar, banner}`; failures log WARN `identity_refresh_failed {kind, reason}` and keep the last files. Compose sets `/data/identity`. |
| `KANADE_ADMIN_DISCORD_CLIENT_ID` | unset | Discord application id; set, it turns on admin Discord sign-in, needs `KANADE_ADMIN_HOST` and the secrets file `discord_client_secret`. The redirect URI is `https://KANADE_ADMIN_HOST/api/admin/auth/discord/callback` (`http:` for loopback dev hosts); register it with Discord. |
| `KANADE_ADMIN_TAILSCALE_LOGINS` | unset | Comma-separated Tailscale logins allowed to sign in via the edge; requires a non-loopback `KANADE_TRUSTED_PROXY` and the secrets file `edge_secret`. |
| `KANADE_ADMIN_SESSION_IDLE_MINUTES` | `60` | Idle timeout, 5–720, not above the absolute lifetime. |
| `KANADE_ADMIN_SESSION_ABSOLUTE_HOURS` | `12` | Absolute session lifetime, 1–168. |
| `KANADE_PUBLIC_DISCORD_CLIENT_ID` | unset | The public origin's own Discord application id (member sign-in); needs `KANADE_PUBLIC_HOST` and the secrets file `public_discord_client_secret`. The redirect URI is `https://KANADE_PUBLIC_HOST/api/public/auth/discord/callback` (no `http:` exception). Without it the portal stays `closed`. |
| `KANADE_PUBLIC_SESSION_IDLE_MINUTES` | `30` | Member session idle timeout, 5–60. |
| `KANADE_PUBLIC_SESSION_ABSOLUTE_HOURS` | `8` | Member session absolute lifetime, 1–24. |
| `KANADE_PUBLIC_FRESH_WRITE_MINUTES` | `15` | How long after a Discord sign-in writes count as fresh, 5–30; fresh ≤ idle ≤ absolute. |

Empty values count as unset. Values are never echoed in errors or logs.
Plain `KANADE_ADMIN_TOKEN` / `KANADE_ADMIN_DISCORD_CLIENT_SECRET` /
`KANADE_PUBLIC_DISCORD_CLIENT_SECRET` are refused: secrets come only from
the secrets directory. The break-glass token is the secrets file
`admin_token` (≥ 32 bytes) when present; changing it ends sessions made with
the old one. `serve --offline` has no store, so it parses these but serves
admin sign-in routes as `503 auth_unavailable` and keeps the public portal
`closed`; live `serve` keeps sessions in its store. See `admin-api.md`
"Sign-in and sessions". The public portal has no open key: live `serve`
opens it while the admin Config switch `self_service.public_portal` is on
(read per request) and member sign-in is configured.
Idle timeouts count ordinary requests only: an open change-hint stream
(`GET /api/admin/events`, `GET /api/public/events`), its heartbeats and its
reconnects (the browser's, and the forced one after each stream lifetime)
never extend a session, so a tab left open with nothing else to do still
signs out at the idle limit. Opening or reconnecting a member stream also
never rotates the session id on a new client address; the next ordinary
request does.

### Serve environment

Parsed by `ServeConfig` (`src/runtime/config/`) for live `serve` only.
Snowflakes are canonical decimal (no sign, no leading zero, non-zero, `u64`);
lists are comma-separated.

| Variable | Default | Meaning |
|---|---|---|
| `KANADE_COMPONENTS` | `bot,web` (every component the build has) | What `serve` runs: `bot` (the Discord gateway, roster sync, delivery tick, chat and extraction) and `web` (the admin app and the member portal), each at most once. `web` alone serves the admin app and portal without the bot: no gateway or tick; with the bot token present it reads the guild over Discord's REST API every 5 minutes (see "Web only"). `bot` alone serves no admin or public routes: the admin listener answers only `/healthz` (else `404`), the public listener is not bound and cards carry no portal button. A name the build lacks, a duplicate or an empty entry refuses startup; `serve --offline` needs `web` (usage error otherwise). Replaces `KANADE_DISCORD_GATEWAY` / `discord.gateway`, which now refuse startup. |
| (secrets file `discord_token`) | required with `bot` | The bot token, read at startup while `bot` runs (a missing secret fails the deploy); optional with `web` alone. Plain `KANADE_DISCORD_TOKEN`/`DISCORD_TOKEN` are refused by every command. |
| `KANADE_GUILD_ID`, `KANADE_BOSSING_ROLE_ID` | required | Snowflakes. |
| `KANADE_ADMIN_ROLE_ID`, `KANADE_CHAT_PILOT_ROLE_ID` | unset | Snowflakes. |
| `KANADE_DEBUG_USER_IDS` | empty | Snowflake list. |
| `KANADE_TEST_CHANNEL_ID` | unset | Snowflake. Where `/debug ping` and `/debug header` post without `channel:`. Unset: a run's home channel (else the post channel); sample runs and `/debug header` use the invoking channel, the week's `digest` the post channel. A run's test card posted anywhere but its home channel is display only (reactions do nothing, never refreshed). |
| `KANADE_DB_PATH`, `KANADE_OWNER_LOCK_DIR` | required | Absolute paths without `..` (the image sets `/data/db/kanade.sqlite` and `/data/run`). The lock directory and the database's directory are created `0700` when absent (existing ones are never re-moded); ownership, symlink and mode checks run when the store opens. A second process on the same store is refused. |
| `KANADE_BACKUP_DIR` | unset | Absolute path without `..` (the image sets `/backups`). Serve only reads it: History checkpoints (`GET /api/admin/history/checkpoints`) list the `kanade backup` manifests there on every request; unset reports `backup_dir_configured: false`. Compose mounts it read-only for serve. Its optional `recipients` file holds age X25519 public keys (`age1…`, one per line, blank lines and `#` comments allowed): while it is present `kanade backup` writes `<name>.age` (encrypted) with a plaintext manifest, and checkpoints list it from that manifest without decrypting. Serve and `kanade backup` refuse an unreadable file, one with no keys, a line that is not a recipient, or a pasted private identity (exit `78`). Absent: plaintext snapshots. |
| `KANADE_CATALOG_FILE` | `boss/bosses.yaml` | Boss catalog. Read like a knowledge document (below). |
| `KANADE_KNOWLEDGE_DIR` | unset | Boss knowledge root, validated once at startup (no reload). The directory and its `.yaml` documents must not be symlinks; each file (`schema.json`, `_meta.yaml`, documents) is opened without following a symlink or waiting on a FIFO and must then be a regular file with one hard link and at most 256 KiB. More than 256 documents (`*.yaml` not starting with `_`) fail startup. |
| `KANADE_PERSONA_DIR` | `config/personas` | Persona layout root; file rules in `config/personas/README.md`. |
| `KANADE_MODEL_BASE_URL` | unset | `https`, or `http` only to loopback, `localhost` or `host.docker.internal`; no userinfo or query. Unset disables models; the alias, group, capability and context variables then are refused. With it, the secrets files `model_api_key` (the bearer key; plain `KANADE_MODEL_KEY` is refused) and `model_ca.pem` (a PEM bundle or one DER certificate replacing the compiled webpki roots) are used when present. |
| `KANADE_EXTRACT_MODEL`, `KANADE_CHAT_MODEL`, `KANADE_REWRITE_MODEL` | unset | Model aliases (printable ASCII, ≤ 200); seeds for a role with no saved alias. |
| `KANADE_EXTRACT_REASONING`, `KANADE_CHAT_REASONING`, `KANADE_REWRITE_REASONING` | unset | Reasoning seeds (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`; chat and rewrite also `inherit` = extraction's level): used only where no `extract_reasoning` / `chat_pilot_think` / `v5.rewrite_reasoning` row is saved (row → env → default: extraction `off`, chat/rewrite `inherit`). Need `KANADE_MODEL_BASE_URL`. |
| `KANADE_MODEL_PERMITS` | `2` | Concurrent model calls in the single `gateway` group, 1–16. |
| `KANADE_MODEL_GROUPS` | unset | Capacity groups as a JSON list of `{name, permits, aliases}` (normally `[[models.groups]]` in `kanade.toml`): names unique, ≤ 64 of `[A-Za-z0-9._-]`; permits 1–16; each alias in one group. Non-empty replaces the `gateway` group; exclusive with `KANADE_MODEL_PERMITS`; needs `KANADE_MODEL_BASE_URL`. A role whose alias is in no group starts with an `ungrouped` warning and its calls are refused; switching a role to such an alias in the config API is refused. Without groups, an alias a role is switched to live joins the `gateway` group. The admin config view reports the groups the governor runs (`models.groups`, read-only). |
| `KANADE_MODEL_CONTEXT` | unset | JSON context seed (tests and harnesses; no kanade.toml key): cloud/local defaults, per-role `{reserve, cap?}` and alias overrides, all required except `cap` and `overrides`. Override keys match route aliases exactly, so an override on a base alias does not apply to its `<base>:<level>` variants. It applies only when `v5.model_context` is unsaved; Config › Models is the live writer. A window (default, cap or override) above 131072 is clamped to 131072 and logged once when the seed applies (`model_context_clamped`, WARN, `fields`, `max`). A zero window or reserve, a reserve above 131072, an unknown field, a reserve not smaller than its role's seed window (its cap, else the smaller zone default), or a reserve of 15360 or more refuses startup naming the field: each model call reserves its prompt estimate (request bytes / 4) plus `max_tokens` out of a 16384-token call budget and must leave at least 1024 for the prompt, else every call fails before sending (`budget_exceeded`). A Config save of such a reserve answers 422 naming the role, reserve and budget. Needs `KANADE_MODEL_BASE_URL`. |
| `KANADE_MODEL_CAPABILITIES` | unset | JSON object of model alias → capability fields (normally `[models.capabilities."<alias>"]` in `kanade.toml`), used only where the listing publishes no Kanata metadata for that alias; Kanata's published metadata always wins. Fields: `structured_output`, `function_tools`, `sampling_controls`, `reasoning_control` (booleans), `reasoning_efforts` (non-empty list of distinct `off` … `max` levels; needs `reasoning_control = true`), `context_tokens` (1–4294967295) and `max_output_tokens` (1–1048576, at most `context_tokens`); omitted fields keep the minimal profile's value (off/unset). Aliases follow the model alias rule (printable ASCII, ≤ 200). An unknown field (including `trust_zone`, `admission`, `streaming`), a wrong type or an out-of-range value refuses startup naming the alias and field. Needs `KANADE_MODEL_BASE_URL`; read once at startup and not editable in the Config API. |
| `KANADE_RUN_LENGTHS` | unset | JSON run lengths seed (tests and harnesses; no kanade.toml key): `{default_minutes, overrides}`. It applies only when `v5.run_lengths` is unsaved; Config › Run lengths is the live writer. The default is 5-240 minutes; each `{boss, difficulty, minutes}` override is 5-480 and must name an exact catalog key and valid lowercase difficulty when saved through Config. Every admin run, including own-time runs, reports the sum across its bosses. |
| `KANADE_TICK_SECONDS` | `30` | Scheduler tick, 5–300. |
| `KANADE_INSTANCE_ID` | `kanade-<random>` | ≤ 64 of `[A-Za-z0-9._-]`. |
| `KANADE_POST_CHANNEL_ID` | unset | Settings seed: snowflake. |
| `KANADE_WATCH_CHANNEL_IDS`, `KANADE_WATCH_CATEGORY_IDS` | empty | Settings seeds: snowflake lists the extractor reads. |
| `KANADE_CHAT_CATEGORY_IDS` | empty | Settings seed: Kanade chats in every channel of these categories (there is no per-channel chat list). |
| `KANADE_EXTRACTION_ENABLED`, `KANADE_CHAT_ENABLED` | unset | Settings seeds: `0` or `1`. |
| `KANADE_BOSS_WEEK_RESET_WEEKDAY` | unset | Settings seed: `mon` … `sun` (case-insensitive). |
| `KANADE_BOSS_WEEK_RESET_TIME`, `KANADE_DAY_OF_PING_TIME` | unset | Settings seeds: exactly `HH:MM`, 24-hour. |
| `KANADE_COUNTDOWN_MINUTES` | unset | Settings seed: positive whole minutes, comma-separated (stored largest first, duplicates dropped). |
| `KANADE_MODULE_ROSTER` | `0` | `0` or `1`. The manual character roster module (per-member characters; never sent to models): `1` serves its admin routes (`/api/admin/members/{id}/characters`, `/api/admin/characters/{id}`); `0` answers them `404`. Read at startup, so a change applies on restart; the `roster_characters` table and its rows stay either way. |
| `KANADE_MODULE_CHARACTER_DATA` | `0` | `0` or `1`. NEXON Open API data for linked roster characters (never sent to models, never in backups); startup refuses `1` without `KANADE_MODULE_ROSTER=1`. `1` serves `/api/admin/characters/{id}/link` and `/refresh` and the Add character lookup `/api/admin/characters/lookup` (shown only, never stored), makes a MapleSEA character create also ask its member to link it, runs the refresh/sweep task and adds Config's health block; `0` answers the routes `404` and opens no file. Read at startup. |
| `KANADE_CHARACTER_DATA_DB_PATH` | unset | Required while character data is on: absolute path of its own SQLite file (directory created `0700`, file `0600`, owner lock in `KANADE_OWNER_LOCK_DIR`), on a volume of its own so `kanade backup` and the data-volume tarball never hold it. Ignored while off. |
| (secrets file `nexon_api_key`) | absent | The Open API key, read while the module is on. Absent: the module runs with links refused (health `no_key`); present but unreadable or bad: startup is refused. |
| `KANADE_SCOUTER_URL` | unset | Optional: the private scouter sidecar (a separate Compose project, no authentication, no secret), e.g. `http://scouter:8091`. Plain `http` to a loopback or private address or a single-label host only (no credentials, path or query); anything else refuses startup. Set: admins (and members, for their own characters) can run estimates (`/api/admin/estimates`, `/api/public/me/estimates`, one lookup at a time); results are kept up to 30 days in the character-data file, and serve refreshes the party members of runs in the next 7 days whose estimate is missing or older than the saved `character_data.estimate_days` (default 7); the scouter is sent roster IGNs only. Unset: every estimate feature is off (routes `404`, no background refresh, health `off`) and the rest runs unchanged. Ignored while character data is off. |
| `KANADE_MODULE_GUIDES` | `0` | `0` or `1`. The guide publisher: `1` serves the admin Guides routes (`/api/admin/guides…`) and runs the publish worker, which posts each guide to its channel as an ordered set of messages and edits them in place on later publishes; `0` answers the routes `404` and runs no worker. Read at startup; the guide tables and their rows stay either way. |

Seeds apply per key only where the store has no row; an unset seed keeps
the code default (`docs/v5/api-schemas/config.json` sections). A stored row
that does not decode (or a `persona` that is not a persona id) fails startup
naming the key, never the value. `KANADE_PILOT_CHANNEL_IDS` is refused with a
pointer to `KANADE_CHAT_CATEGORY_IDS`. Other unknown `KANADE_*` variables are
ignored, as for the HTTP settings. **Exception:** the retired
`KANADE_PSEUDONYMIZE` and `KANADE_ALLOW_EXTERNAL_UNMASKED` names, and their
TOML counterparts `models.pseudonymize` and `models.allow_external_unmasked`,
are rejected on presence, even when empty or false. Remove both from private
deployment settings before a separately approved rollout; an old config must
not silently switch formerly masked calls to raw traffic. Configured external
roles send raw names, IDs, messages and URLs when present, with a warning that
data leaves the homelab. Unknown or not-yet-listed aliases are treated as
external until a successful listing establishes their zone. A provider's
retention terms (such as Kanata's or another provider's zero data retention)
concern retention, not transmission.

### Web only

With `KANADE_COMPONENTS=web` there is no gateway, roster task or delivery
tick. When the secrets file `discord_token` is present, serve reads the
guild over Discord's REST API at startup and every 5 minutes: the bot's
user (`GET /users/@me`), the guild's owner and roles (`GET /guilds/{id}`),
its channels, and every member page. The channel pickers, role names and
Setup's access report read that view; the roster is reconciled as the bot
does on `GUILD_CREATE` (rows pruned of deleted roles, members who left or
lost the bossing role marked so, their admin and portal sessions ended), and
a page that fails concludes no departures (logged WARN
`rest_guild_read_failed {step, outcome}`; a complete pass logs
`rest_guild_read`). Setup's application read and the preview's difficulty
marks use the same token. It never posts or reads messages. Guides stay
editable but publish, unpublish and "Search again" answer `503
discord_offline`. Without the token nothing contacts Discord: pickers are
empty and permissions unknown. `/healthz` reports `discord` and `scheduler`
`disabled`.

### Other OpenAI-compatible providers

`KANADE_MODEL_BASE_URL` also accepts another OpenAI-compatible server
(vLLM, llama.cpp, LiteLLM, OpenRouter, OpenAI, Groq, Together, Azure OpenAI
v1 and similar): use HTTPS, or plain HTTP only for loopback, `localhost` or
`host.docker.internal`, as above. The adapter trims one trailing `/v1` from
the base path and calls `<base>/v1/models` and `<base>/v1/chat/completions`
(OpenRouter's `/api/v1` and Azure's `/openai/v1/` work). Not supported: a
base on another version path (such as Gemini's `/v1beta/openai`, which would
become `/v1beta/openai/v1/...`) and a base with a query string (classic Azure
OpenAI's `?api-version=`; use its `/openai/v1/` endpoint instead).
The secrets file `model_api_key` holds the optional bearer key; when
present, Kanade sends `Authorization: Bearer` on both requests (there is no
`api-key` header variant). Keep the key itself out of TOML and environment
variables.

Kanata can publish per-model capability metadata. Resolution per alias is
published metadata, then the operator's declaration, then the minimal
profile; a declaration never overrides what Kanata publishes for an alias.
With a plain model list, every undeclared alias uses the minimal profile,
which sends no `response_format`, tools, `max_tokens`, sampling or
`reasoning_effort` fields (so chat, which needs tools, is refused). Declare
what the server accepts per alias in `kanade.toml` (or the
`KANADE_MODEL_CAPABILITIES` JSON variable):

```toml
[models.capabilities."example-chat"]
function_tools = true        # chat needs tools
structured_output = true     # send response_format (json_schema)
sampling_controls = true     # send max_tokens, temperature, seed, top_p
reasoning_control = true     # send reasoning_effort
reasoning_efforts = ["off", "low", "medium", "high"]  # optional; needs reasoning_control
context_tokens = 32768       # the model's context window
max_output_tokens = 4096     # caps each role's reserve; at most context_tokens
output_limit = "max_completion_tokens"  # optional: the size field's name
```

Omitted fields stay off (or unset). A declaration is used for request
shaping, effort resolution (a role level the declared list does not accept
falls back to `off` when listed, else the lowest listed level), the Config
catalog view and model picker (a declared alias shows trust zone `unknown`
and counts as external), `kanade models check`, the context window (source
`catalog`) and the reserve clamp. This holds whether or not the server's
model list names the alias: a declared model the list omits, or every
declared model when the server has no list, is added to the catalog marked
declared (`listed: false`; `declared` in `kanade models check`) and can be
picked for a role. Only a listed base makes `<base>:<level>` variants.

`output_limit` names the field that carries the role's reserve:
`"max_tokens"` or `"max_completion_tokens"`. Without it, `max_tokens` is sent
only with `sampling_controls` (Kanata's rule). With it, the reserve is always
sent under that name, whether or not sampling fields are; OpenAI reasoning
models, for example, need `output_limit = "max_completion_tokens"` and
`sampling_controls = false`.

A 400 whose `error.param` names `response_format`, a sampling field, the
declared `output_limit` field or `reasoning_effort` drops that capability for
the alias until restart (one resend without it), as for Kanata; rejecting the
declared size field drops only the size field. A 400 that names no `param`
(or names a field Kanade did not send) fails the call without a resend. With
no `reasoning_efforts` list `off` is sent as `reasoning_effort: "none"`;
declare the list if the server refuses `none`. A `429` whose `error.code` is
`insufficient_quota` (an exhausted account) fails at once as
`quota_exhausted`, a misconfiguration, and is never retried.

A server whose `GET /v1/models` answers 404 or 405 has no model list: the
listing is reported as `no-listing` (startup logs `models_degraded` with
`reason: "no-listing"`; it is retried like any failed listing), the declared
models are used, admin Config saves roles to them, Setup shows the model
gateway as ready with a note, and `kanade models check` prints
`catalog: no model list (declared models only)` without failing. Any other
listing failure (unreachable, a 5xx, a timeout) still refuses model saves in
Config until the server answers.

Trust zones and admission limits cannot be declared: plain routes stay
external, and capacity comes from `models.permits` or `[[models.groups]]`.
Declarations are operator-only (read once at startup, not editable in the
Config API). Config › Models sets a per-alias window (capped at a declared
`context_tokens`), each role's `reserve` and optional `cap`, and the role
models and reasoning. See
[`kanade.openai-compatible.example.toml`](../../kanade.openai-compatible.example.toml).
Omitting `[[models.groups]]` puts all model aliases in one shared `gateway`
group.

An external server still receives raw member names, IDs, messages and URLs
when present; the external/unmasked warning applies. Kanata-only published
metadata (trust zones, admission limits) and Kanata gateway
`client_request_id` access-log tracing are unavailable when calling another
server directly.

### Config file

`KANADE_CONFIG` names a TOML file (in Compose: the private, git-ignored
`kanade.toml` at the checkout root, mounted read-only at
`/config/kanade.toml`; the tracked `kanade.example.toml` documents every key)
for deployment settings. Unset or empty keeps the environment-only behaviour.
Each key sets one variable, whose rules apply unchanged
(`src/runtime/config/file/`); all commands (`serve`, `healthcheck`, `import`,
`models check`, `backup`, `restore`, `portal`, `sessions`, `token`) read it.

Each kind of value has one owner:

| Kind | Owner | Examples |
|---|---|---|
| Paths inside the image | `deploy/Dockerfile` `ENV` | `KANADE_DB_PATH`, `KANADE_OWNER_LOCK_DIR`, `KANADE_IDENTITY_DIR`, `KANADE_WEB_DIR`, `KANADE_BOSS_DIR`, `KANADE_CATALOG_FILE`, `KANADE_KNOWLEDGE_DIR`, `KANADE_PERSONA_DIR`, `KANADE_EMOJI_DIR`, `KANADE_CHARACTER_DATA_DB_PATH`, `KANADE_BACKUP_DIR`, `KANADE_SECRETS_DIR`, `KANADE_SAVED_SECRETS_DIR` |
| Network addresses | `deploy/compose.yaml` (and the override) `environment:` | `KANADE_ADMIN_BIND`, `KANADE_PUBLIC_BIND`, `KANADE_ALLOW_PRIVATE_BIND`, `KANADE_CLOUDFLARED_PEER`, `KANADE_TRUSTED_PROXY` |
| Secrets | files with fixed names in the secrets directory ("Secrets directory" below) | `discord_token`, `admin_token`, `model_api_key` |
| Deployment settings | `kanade.toml` (the table below) | timezone, Discord ids and roles, hosts, OAuth client ids, model gateway, modules |
| Live settings | the store, written by Config and Setup | channels, switches, pings, run lengths, model roles and context, persona |

- Precedence: for a key in the table, `kanade.toml` is the deployment's
  value. A non-empty `KANADE_*` variable still overrides its key (the
  interface of tests, harnesses and migration rehearsals); `serve` then logs
  one WARN `config_shadowed` naming the shadowed keys (never the values). An
  empty variable does not count, so a blank Compose variable cannot clear a
  file value. The settings seed variables (`KANADE_POST_CHANNEL_ID`, the
  channel and category lists, the switches, `KANADE_DAY_OF_PING_TIME`,
  `KANADE_COUNTDOWN_MINUTES`, `KANADE_RUN_LENGTHS`, the role aliases and
  reasoning levels, `KANADE_MODEL_CONTEXT`) have no key: Config owns those
  values, and a seed applies only where no row is saved.
- Secrets never go in the file: a key containing `token`, `secret`, `key`,
  `password`, `passwd` or `credential` stops startup.
- Removed keys and variables stop startup naming where the value lives now,
  never the value (``kanade.toml `discord.token_file` was removed: put the
  token in the secrets directory as `discord_token` ``); the full list is
  `RETIRED` in `src/runtime/config/file/keys.rs`. Blank removed variables do
  not count.
- Unknown keys or tables, wrong TOML types (e.g. `tick_seconds = "30"`) and
  syntax errors stop startup with exit `78`, naming the key (or the line for
  syntax), never the value. Errors name the key of every variable they
  mention, with the variable in brackets, e.g. ``kanade.toml
  `runtime.timezone` [KANADE_TIMEZONE] must be a valid IANA timezone``. The
  file is at most 1 MiB.
- Booleans are `true`/`false` (written as `1`/`0`); lists are TOML arrays
  (joined with commas, so string items may not contain one); snowflakes may be
  strings or integers (strings keep ids above `i64` exact).
- `[[models.groups]]` takes exactly `name`, `permits` and `aliases`.
- `[models.capabilities."<alias>"]` takes only the capability fields listed
  under `KANADE_MODEL_CAPABILITIES`; other names refuse startup.

| Key | Variable | TOML type |
|---|---|---|
| `runtime.components` | `KANADE_COMPONENTS` | non-empty string list |
| `runtime.timezone` | `KANADE_TIMEZONE` | string |
| `runtime.instance_id` | `KANADE_INSTANCE_ID` | string |
| `runtime.tick_seconds` | `KANADE_TICK_SECONDS` | integer |
| `runtime.shutdown_timeout_seconds` | `KANADE_SHUTDOWN_TIMEOUT_SECONDS` | integer |
| `runtime.healthcheck_timeout_seconds` | `KANADE_HEALTHCHECK_TIMEOUT_SECONDS` | integer |
| `runtime.boss_week_reset_weekday` | `KANADE_BOSS_WEEK_RESET_WEEKDAY` | string |
| `runtime.boss_week_reset_time` | `KANADE_BOSS_WEEK_RESET_TIME` | string |
| `discord.guild_id` | `KANADE_GUILD_ID` | snowflake (string or integer) |
| `discord.bossing_role_id` | `KANADE_BOSSING_ROLE_ID` | snowflake (string or integer) |
| `discord.admin_role_id` | `KANADE_ADMIN_ROLE_ID` | snowflake (string or integer) |
| `discord.chat_pilot_role_id` | `KANADE_CHAT_PILOT_ROLE_ID` | snowflake (string or integer) |
| `discord.debug_user_ids` | `KANADE_DEBUG_USER_IDS` | snowflake list |
| `discord.test_channel` | `KANADE_TEST_CHANNEL_ID` | snowflake (string or integer) |
| `admin.host` | `KANADE_ADMIN_HOST` | string |
| `admin.discord_client_id` | `KANADE_ADMIN_DISCORD_CLIENT_ID` | snowflake (string or integer) |
| `admin.tailscale_logins` | `KANADE_ADMIN_TAILSCALE_LOGINS` | string list |
| `admin.session_idle_minutes` | `KANADE_ADMIN_SESSION_IDLE_MINUTES` | integer |
| `admin.session_absolute_hours` | `KANADE_ADMIN_SESSION_ABSOLUTE_HOURS` | integer |
| `public.host` | `KANADE_PUBLIC_HOST` | string |
| `public.discord_client_id` | `KANADE_PUBLIC_DISCORD_CLIENT_ID` | snowflake |
| `public.session_idle_minutes` | `KANADE_PUBLIC_SESSION_IDLE_MINUTES` | integer |
| `public.session_absolute_hours` | `KANADE_PUBLIC_SESSION_ABSOLUTE_HOURS` | integer |
| `public.fresh_write_minutes` | `KANADE_PUBLIC_FRESH_WRITE_MINUTES` | integer |
| `models.base_url` | `KANADE_MODEL_BASE_URL` | string |
| `models.permits` | `KANADE_MODEL_PERMITS` | integer |
| `models.groups` | `KANADE_MODEL_GROUPS` | array of tables |
| `models.capabilities` | `KANADE_MODEL_CAPABILITIES` | table of alias tables |
| `modules.roster` | `KANADE_MODULE_ROSTER` | bool |
| `modules.character_data` | `KANADE_MODULE_CHARACTER_DATA` | bool |
| `modules.guides` | `KANADE_MODULE_GUIDES` | bool |
| `character_data.scouter_url` | `KANADE_SCOUTER_URL` | string |

Variables without a key: `KANADE_CONFIG` itself; the image paths and
network addresses above; the settings seeds; the refused plain secrets
(`KANADE_DISCORD_TOKEN`, `DISCORD_TOKEN`, `KANADE_MODEL_KEY`,
`KANADE_ADMIN_TOKEN`, `KANADE_ADMIN_DISCORD_CLIENT_SECRET`,
`KANADE_PUBLIC_DISCORD_CLIENT_SECRET`) and the renamed `KANADE_BIND` /
`KANADE_PILOT_CHANNEL_IDS`.

### Secrets directory

Secrets are one-line files (at most 4 KiB) with fixed names in
`KANADE_SECRETS_DIR` (default and image value `/run/secrets`; Compose binds
the host's `~/.config/kanade/v5/secrets`, or `KANADE_SECRETS_HOST_DIR`, there
read-only). Configuration only names them; startup reads them:

| File | Used when | Absent |
|---|---|---|
| `discord_token` | always (`serve`, `kanade ctl emojis`) | refused |
| `admin_token` | present: break-glass sign-in (at least 32 bytes); `kanade portal`/`sessions` sign in with it | no break-glass sign-in; the commands refuse |
| `model_api_key` | `models.base_url` is set | no bearer key (a keyless gateway) |
| `model_ca.pem` | `models.base_url` is set: a PEM bundle or one DER certificate replacing the compiled roots (any size) | the compiled webpki roots |
| `discord_client_secret` | `admin.discord_client_id` is set | refused |
| `public_discord_client_secret` | `public.discord_client_id` is set | refused |
| `edge_secret` | `KANADE_TRUSTED_PROXY` is set (at least 32 bytes) | the proxy is trusted without it; `admin.tailscale_logins` refuses |
| `nexon_api_key` | module `character_data` is on | links are refused (health `no_key`) |

A refusal names the file, the directory and what needs it, never the
contents. A present file that is empty, has more than one line, is over 4 KiB
or cannot be read refuses as well, and so does a directory that cannot be
searched (a missing file is the only "absent"). A file no feature uses is
ignored. The backup encryption keys are not secrets: they are the
`recipients` file in `KANADE_BACKUP_DIR` (see `KANADE_BACKUP_DIR`).

Saved from Setup: `discord_token`, `model_api_key`, `discord_client_secret`,
`public_discord_client_secret` and `nexon_api_key` may also be entered in
the admin app's More › Setup, which writes them (0600, through a temporary
file and a rename) into `KANADE_SAVED_SECRETS_DIR` (the image's
`/data/secrets`, created 0700 on the first save and refused if it is a
symlink or open to group or others). They are read after the secrets
directory, so a host file always wins and Setup will not save over one;
`admin_token`, `edge_secret` and `model_ca.pem` are host files only. Saving
needs a sign-in from the last 15 minutes, is recorded in History (section
`secrets`, "saved" or "replaced", never the value) and applies at the next
start: with `KANADE_SUPERVISED` set, Setup's "Restart now" first checks the
configuration and every secret the next start reads, then shuts down as a
SIGTERM would. The saved files live on the data volume, so they are in the
whole-volume tarball (encrypted with recipients) but not in `kanade backup`
snapshots.

### Live serve

After composing, serve logs the model setup once in the background
(`serve/models.rs`; HTTP readiness never waits and a degraded listing is
never fatal), as JSON lines without the key or the gateway URL:

```text
{"level":"INFO","event":"models_listed","models":3}
{"level":"INFO","event":"model_role","role":"extraction","alias":"gpt-6-luna:high","effort":"high","source":"fixed","route":"homelab"}
{"level":"INFO","event":"model_role","role":"chat","alias":"gpt-6-luna","effort":"low","source":"floor","route":"homelab"}
{"level":"INFO","event":"model_role","role":"rewrite","alias":"ext","effort":"low","source":"stored","route":"external_unmasked"}
{"level":"INFO","event":"model_warning","kind":"unpublished_effort","message":"chat reasoning off is not allowed: gpt-6-luna requires reasoning; sending low"}
{"level":"WARN","event":"model_warning","kind":"external_unmasked","message":"UNMASKED: rewrite model ext leaves the homelab; raw member names, IDs, messages, and complete URLs are sent to the provider. A provider's retention terms (such as zero data retention) do not prevent transmission."}
```

`models_degraded` (WARN, `reason`) replaces `models_listed` when the
listing fails (every route then stays external until one succeeds;
`reason: "no-listing"` is a server without a model list, see above);
`models_disabled` is logged without `KANADE_MODEL_BASE_URL`. `source` is
`fixed` (a `<base>:<level>` variant), `floor` (the configured level is not
accepted; the lowest accepted one is used), `inherit` (extraction's level),
else where the role's own level came from: `stored`, `env` or `default`.
`route` is currently `homelab` or `external_unmasked`; `external_masked` and
`external_refused` are historical values only. Every external route, including
an unknown alias until a listing classifies it, gets an `external_unmasked`
WARN with the raw-data and retention notice. `capacity` and `ungrouped`
are WARN, `unpublished_effort` INFO. Each role whose route is local
(`leaves_homelab` false) and whose effective context window is above 16,384
adds one WARN line
`{"event":"model_warning","kind":"local_context","role":…,"alias":…,"window":…,"message":"Context past 16k may result in degraded performance on local models."}`;
cloud routes never warn. Each routed role whose reserve is not smaller than
its effective window (defaults, a seed, or a catalog that later lowers its
published window can all cause this) adds a WARN
`{"event":"model_warning","kind":"context_reserve","role":…,"alias":…,"window":…,"reserve":…,"message":"<role> context reserve <n> is not smaller than <alias>'s effective window <n>; its prompts cannot fit"}`;
`kanade models check` prints the same message as a `warning:` line under the
role, as it does the local-context warning. The local-context line (with `"surface":"admin_portal"`) is
logged when a Config save changes `models.context` and leaves a role past the
threshold. The catalog refresh task (every 300 s,
30 s until a listing succeeds) is aborted at shutdown.

Model roles saved in the config API switch the running stack at once
(`ModelStack::apply_roles`): each role's next question, extraction call or
rewrite opens with the new alias and resolved effort, and calls already
running keep theirs. After `settings_changed` (which carries the stored
before/after values) each role whose running alias or effort changed logs

```json
{"level":"INFO","event":"model_role_switched","role":"chat","from":{"alias":"gpt-6-luna","effort":"low"},"to":{"alias":"gpt-6-luna","effort":"high"}}
```

(`null` for an unrouted side), and `model_roles_not_applied` (WARN) if the
stack refused them. The save stands; until a switch succeeds every config
answer (`GET` and each `PATCH`) first retries it against the saved roles
and, while it still fails, carries a notice that the saved models are not
running (they apply at restart at the latest). Chat reads its route once
when a question is prepared; the system prompt's runtime line, permit,
requests, chat row and
`chat_answered`/`chat_failed` (`model`, `reasoning`) all use that pinned route,
so a save landing mid-question applies from the next question. Per-call chat
and extraction `guardrail.external_unmasked` is set only after a provider
request is admitted. Each extraction log row carries its `model` and
`reasoning`.
Context windows apply live the same way: chat resolves per question,
extraction once per pass (burst or rescan batch) and the heading rewriter per
request, each from the settings saved at that moment and the cached catalog;
a call in flight keeps what it started with. The window is
`min(override | published | zone default, published, 131072, role cap)` and
each role's `max_tokens` is its reserve clamped to the route's published
`max_output_tokens` (rewrite defaults to 96). A reserve must stay below 15360
(the 16384-token call budget less a 1024-token prompt floor); the runner
refuses a call whose prompt estimate plus `max_tokens` exceeds the budget
before sending it, and the Rewrites log shows it as "reserved N > budget B"
(a reply whose reported usage exceeds the reservation shows as "used N >
reserved M").
Extraction and the heading rewriter are still composed at startup: a role
with no alias then gets its route live, but extraction and heading rewrites
for it start only after a restart — the save says so in `notices` ("The
extraction model had none when the bot started: restart to start extraction
with …") and the view shows no `running` for it. `model_role` lines are
startup-only.

`serve` (`src/runtime/serve/`) reads the bot token file, checks
the backups directory's `recipients` file when present, opens and owns the store, loads
the catalog, knowledge and personas, resolves settings, then serves both
listeners: one shared `SchedulerWriter`, `ApiState` (schedule policy from
settings, one `GuildAccess` shared with the staff gate, guild id for card
links, the
gateway's `GuildCache` as the channel list) and `AdminAuth` (Discord OAuth,
Tailscale and break-glass per the HTTP environment) with `GuildStaffGate`
over `StoreGuildMembers`.

The Discord side (`serve/discord/`) runs one gateway session for
`KANADE_GUILD_ID` with the production bot token:

- Events from other guilds and DMs are dropped and counted (health
  `dropped_events`); interactions are handled only when both the interaction
  and its command are registered to this guild.
- On every `READY` the transport learns the application id; on the guild's
  first `GUILD_CREATE` after it, the command registry
  (`serve/commands.rs`: the retained v4 commands, `register_retained`)
  is bulk-overwritten for the
  guild (never globally; v4 re-syncs its own guild commands when it starts
  again, the rollback), then startup roster reconciliation pages
  `list_members` (1000 per page until a short page) and applies `Seen` for
  changed members and `Left` for stored members no longer in the guild.
- One sequential roster task applies `GuildAvailable`
  (`on_guild_available`), member events (`on_roster_update`) and the
  reconciliation in gateway order. `GuildAvailable` also fires on every role
  deletion: stored role lists are filtered against the guild's current roles
  (Discord sends no member updates then), so deleting the configured admin
  role revokes its holders. Discord OAuth sign-in works for members holding
  the admin role or Administrator (and the owner) once reconciled.
- Card ✅/❌ go to `CardDesk` (the approver is the member, `via_portal:
  false`); other ✅/❌ on reminder/digest cards are RSVPs. One sequential
  reaction worker keeps a member's add/remove in order.
- `recover_on_start` runs once before the gateway starts (an attempt a
  previous process left in flight becomes indeterminate and is never
  resent), so nothing (reactions, commands, the tick) can send first; a
  failed recovery fails startup. The delivery tick waits for the guild and
  the initial roster reconciliation attempt before its first tick; the roster
  task marks that attempt complete even when fetching members fails, so a
  failed page neither sends early nor deadlocks startup. It then ticks every
  `KANADE_TICK_SECONDS` under one lease per tick in v4's order (materialise,
  mark done, expiry, notice outbox drain with `DEFAULT_MAX_NOTICE_AGE`,
  digest, reminders). The post channel, quiet
  mode, watch list and members are re-read each tick; the schedule policy
  is fixed at startup, as in the API. Reachability is the shared
  `GuildCache`.
- Admin alerts go to the structured log (`admin_alert`, throttled per key
  per hour) for the beta.
- Chat (`runtime::serve::chat`, driver `chat::driver`, input
  `bot::chat_feed`): a created message mentioning the bot (its user, or its
  managed integration role, which `@Kanade` often resolves to; v4) or
  replying to it in
  any channel or thread of `chatbot.category_ids`, from a holder of
  `KANADE_CHAT_PILOT_ROLE_ID` (staff are exempt), is answered as a reply
  that pings nobody. `chatbot.enabled`, the categories and the allowances
  are read live from the config desk's `SettingsChanged`; allowance
  overrides reload on each settings change. The chat route follows the
  model stack: external and not-yet-classified aliases send raw prompts
  without an opt-in when the governor admits the request. Startup/model
  checks warn for external routes; live role changes do not reinstate an
  opt-in, and a call's `guardrail.external_unmasked` is set only after
  admission. The persona is the live snapshot with the
  member's saved reply style. Replies are outside the delivery journal (an
  ambiguous reply is logged, never retried); proposal cards go through the
  shared card desk (journalled, ✅/❌ via the reaction worker). Withheld ids are
  reloaded from the chat log before the first admission (a failed reload
  fails startup); the question timeout (60 s) is validated below the
  clean-retry window. A queued question gets its position as a keycap
  reaction; shed, expired and deleted waiters are refunded; deleting a
  running question lets it finish (a staged proposal is never cut) but
  posts nothing more for it.
- Extraction (`serve/extract/`, S9; contract
  `extraction-orchestration.md`): the handler hands created, edited and
  deleted guild messages to `bot::extract_feed` without awaiting; one feed
  task forwards them in gateway order to the `Pipeline` (debounce, backlog,
  governed calls on the `ModelStack`'s client under raw Passthrough; external
  and not-yet-classified routes send names, IDs, message text and URLs when
  present without an opt-in). Chat sees each created message
  first: one it handles (answered, queued, shed or rate-limited; v4
  `Handling(True)`) is `handled_by_chat`, cached as processed and never
  extracted, and so are its later edits (the live guild's chat and watch
  categories are the same). Only regular messages and replies are read.
  Watched = the settings' channel
  ids plus every channel and thread under the watched categories (thread
  messages are filed under their parent). A message or edit the handler
  received more than 60 s after it happened is `Replay`: cached for
  rescans, never offered
  to the pipeline, so stale history (RESUME replays included) makes no card
  until a rescan (parent decision). Cards only (the public portal is
  closed), posted and refreshed through the one `CardDesk` chat and the
  reaction worker share, so card ✅/❌ reach them; merge notices go through
  the notice outbox. `extract_enabled` and `paused` apply live through
  `ConfigDesk::subscribe` (the watch list too); while off, messages are
  cached, nothing calls the model and rescan submits are refused (`/rescan`
  "not available", API `503`; parent decision). Switching off cuts calls in
  flight and ends queued and running rescans (`cancelled` / `switched off`),
  so no call and no card follow it. When on at startup, once the guild is
  available, the model listing has named the routes' trust zones and the
  roster task has reconciled (each waited for at most 60 s), one automated
  `24h` rescan (`source: startup`) of every watched channel is queued; it
  reads only messages no pass has read (`processed_at` unset), so a restart
  reposts no card and supersedes nothing. Rescan jobs a crash left open are
  ended `cancelled` / `interrupted` when the runner starts.
  Without a model gateway or extraction alias nothing is composed
  (`extraction_unavailable` logged; rescans `503`); messages are then only
  counted. `ApiState.rescans` and `/rescan` share one runner
  (`RescanService` over `Rescans`, backfill via `channel_messages` `After`
  pages of 100). The extraction effort sent is the configured level at
  startup (the runner floors `off`). A manual or API rescan re-reads cached
  messages, processed or not, whatever chat did with them (as v4).
- A fatal close (4004: the token; 4014: enable the Server Members and
  Message Content privileged intents in the Developer Portal) is logged once
  (`gateway_closed_for_good`), stops the Discord side (workers, tick) and
  leaves HTTP serving with health `discord: closed` (`degraded`) until
  shutdown; serve neither exits nor reconnects (parent decision: a restart
  loop would re-IDENTIFY with the shared production token). Other
  disconnects reconnect with Twilight's backoff (health `discord:
  disconnected` meanwhile).

Shutdown (`SIGINT`/`SIGTERM`): the gateway closes; its spawned
interaction/registration tasks share a 2 s drain grace, then unfinished tasks
are cancelled and joined (`gateway_tasks_aborted`). Chat stops (waiting
questions are refunded, running ones get 3 s to finish and are then cut, refunded and
concluded with a `cancelled` log row; tidy-up gets 2 s more plus 1 s for
the log writes of anything aborted, so chat adds at most 6 s; logged
`chat_stopped`), extraction stops (`Rescans::close`: queued jobs end
`cancelled` / `shut down`, the running one `cancelled` before its next
burst; feed, pipeline and worker get 1 s, then calls and permit waits in
flight are cut and logged `failed` with `cancelled: serve shut down`
(`extraction_calls_cancelled`), and anything left after 2 s more is
aborted (`extraction_aborted`), so extraction adds about 3 s), the roster
and reaction workers drain, the running tick finishes (a tick is never cut
midway, and it is polled from the start of shutdown, the gateway close
included, so no step waits on a store write it holds), then HTTP
drains, then the store closes (logged `store_closed`) so ownership is
released only after SQLite closes. A startup failure after the store opened
closes it too.

All of this shares one 25 s budget from the signal (Compose's 30 s
`stop_grace_period` less a 5 s margin): each step gets its own grace or what
remains, whichever is less, and every wait before the store close ends by
22 s, keeping 3 s for the close. One exception: after chat aborts its
questions at 22 s, the wait for their log writes may run up to 1 s more, so
the close always starts with at least 2 s left. Steps after chat (the HTTP
drain included) then get no time and are cut at once; an HTTP request still
in flight is not dropped but keeps its connection, and the store close waits
for it within the time left. At 22 s the store also stops handing out read
connections (logged `store_reads_refused`): a read waiting for one, and every
read after it, fails at once (an admin read answers 503 `unavailable`), while
reads already running and all writes finish. A step the budget cuts logs
`shutdown_deadline_cut` (`phase`, `elapsed_ms`); serve still exits cleanly.
Workers still running are aborted; a worker caught inside a store write (say a
header pre-generation save at the cutoff) has that transaction rolled back
whole, and the header is generated again after the restart. The tick is not cut. Every Discord call
serve makes (the tick, a manual digest post, card refresh and card posts,
decline retraction, commands, chat, extraction, roster and identity) that is
still in flight at 21 s ends as the transport's own deadline would: a post or
edit counts as possibly delivered (journalled indeterminate, never re-sent),
and a read or any call started later as not sent. That second before 22 s
lets the caller record the outcome through its usual journal path before
workers are aborted and reads are refused; a manual digest's HTTP request
then answers, and the store close waits for it. The tick never calls the
model: card headings are rewritten ahead of time by the header
pre-generation worker: a daily batch at `notifications.header_generation_time`
(Config → Notifications, guild time, default 00:00, read live; a change
applies from its next occurrence) writes the lines for every card firing in
the next 24 h and the coming week's digest when its reset falls in that
window, a run of the process whose batch time has passed today without a
batch runs one at once, and a 60 s catch-up tick writes lines only for cards
the last batch had not seen (runs added or moved since) and retries the
batch's failures (a busy rewrite permit without spending one of the three
attempts). A rewrite of its in flight at 22 s (or started later)
fails as unavailable and stores nothing, so the send uses the seed text; the
worker's own stop abandons a rewrite earlier but always lets a store write
it began finish. The tick's waits are store work. Reads fail at once
after 22 s (above); before that, waiting for one of the 4 read connections
is capped at 5 s (then the read fails and the tick step is retried next
tick; an admin read answers 503 `unavailable`). Writes are atomic
transactions and are never cut: they take turns on the one write connection,
and each holds it only for its own SQL, so a tick's writes after 22 s finish
in milliseconds. SQLite's 5 s `busy_timeout` caps a lock wait, which only a
process outside kanade holding the database lock could cause.

So serve normally returns within 25 s of the signal. Known limit (accepted
2026-10-07): if a process outside kanade holds the SQLite write lock, a tick
write that starts just before 22 s can wait the full 5 s `busy_timeout`, and
serve can take about 27 s, still inside Compose's 30 s.

Runbook: `deploy/README.md`. Never run two bots on one token: Discord allows
one gateway session per token.

Still not wired: the inbox's Discord card refresh/close.

### Listeners

Two routers, authorized by mounting: the public router is built without any
admin route, so every `/api/admin/*`, `/__*` and `/healthz` path is a generic
`404` there for every method. Both apply, outermost first: security headers,
Host allow-list (`421 misdirected` for unknown, missing, duplicated or
mismatched absolute-form hosts), proxy trust (forwarding, Cloudflare and
Tailscale headers from any other peer are stripped before routing), then a
64 KiB declared-body limit (`413`) and a 30 s handler timeout (`503
timeout`). Errors are `ApiError` bodies `{error, message}` with generic text.

Headers: CSP with enforced Trusted Types
(`require-trusted-types-for 'script'; trusted-types kanade-sw`, the one policy
being the service-worker registration; no `report-uri`, which only the
browser e2e server adds), `nosniff`, `no-referrer`, COOP/CORP `same-origin`,
Permissions-Policy and per-path Cache-Control; non-2xx answers are `no-store`;
`Strict-Transport-Security` is sent on the public origin only; no CORS headers
are ever sent.

| Route | Admin | Public |
|---|---|---|
| `GET /healthz` | Clients on this host only: a loopback peer or the listener's own address, whatever the trusted-proxy setting (any loopback Host name); requests the authenticated edge relays get 404 | 404 |
| `GET /api/identity`, `/identity/{avatar,banner}` | yes | yes |
| `GET /api/public/status` | 404 | `{portal}`: `open` only while the switch is on and member sign-in is configured |
| `/api/public/auth/discord/{start,callback}`, `POST /api/public/auth/logout` | 404 | member sign-in (`303 /?login_error=closed` while closed; sign-out always) |
| `/api/public/session{,/avatar}`, `/api/public/sessions{,/{handle},/end-all}` | 404 | member session required; `503 closed` while closed |
| other `/api/public/*`, `/art/*` | 404 / art | `503 closed`, `404` while open |
| `GET /art/{portraits,icons,entry,animated}/{key}` (`animated` honours a single byte `Range`) | file or 404 | `503 closed` (`404` while open) |
| other `GET`/`HEAD` | the app's static files; extensionless paths get `index.html`, missing files 404 | same, public app |

Admin API handlers take the `AdminSession` extractor (`src/api/auth/`); the
public session routes sit behind `auth::member::require_session` (member
realm, `__Host-kanade_pub`),
which never reads an admin cookie, bearer or edge identity;
unmounted `/api/admin/*` paths are `404`. Static paths reach the filesystem only as plain segments (no `..`,
dotfiles, percent-encoding or backslashes) and only inside the canonical app
root, so symlinks cannot escape it.

Known gap: `axum::serve` sets no header-read timeout, so slow-header clients
are bounded by the edge/cloudflared in front of the loopback listeners until
harden-and-package revisits connection limits.

### Admin composition: what remains

`serve --offline` builds neither `AdminAuth` nor `ApiState`, so admin reads
answer `503 auth_unavailable`. Live `serve` composes both (above) with
the `GuildCache` as the channel list, fed by the gateway (or, with `web`
alone, by the REST guild sync). Roster wiring: member updates and guild
availability reach `app::roster` (store side) on one sequential roster task
with startup reconciliation, and its `RosterHooks` end sessions and drop
portraits through the web side (`api::auth::roster`; a no-op without `web`,
where the web side's 5-minute re-check ends them later), so the Discord
sign-in preconditions (ordering, reconciliation, admin-role deletion) hold.
Rescans (A7): live serve builds the extractor's `Rescans` queue (Discord
`History` backfill, `Extractor` over the governed model client, the
scheduler `Proposer`, `CardOutbox`), spawns `Rescans::run`, sets
`ApiState.rescans` to `RescanDesk::new(..)` over the gated runner and calls
`Rescans::close` on shutdown before the store closes. Without a model
gateway, and in `serve --offline`, it stays `None` (`503`).
Admin writes' notices (A4 run and timing notices, rollbacks, inbox merge and
requester notices) are written to the store's notice outbox with the change
and posted by the delivery tick's outbox drain once serve runs the tick
(`maintenance-contract.md`, *Notice outbox*). `DeliveryConfig.max_notice_age`
(default `DEFAULT_MAX_NOTICE_AGE`, 6 h; parent decision 2026-09-25) retires
older notices unsent at drain time, so the backlog written before serve
first ticks (admin edits, an import) never floods the channels; serve builds
it with that default. Still dropped: the inbox's Discord card refresh/close
(and its superseded siblings' cards).
Bot token (user decision 2026-09-25): the secrets file `discord_token`
(`/run/secrets/discord_token` in the container, bound read-only from a host
directory outside the repo); plain `KANADE_DISCORD_TOKEN`/`DISCORD_TOKEN` are refused at startup.
Discord allows one gateway session per token, so never run two bots on one
token, and v5 must act only in its
configured guild: ignore every event from other guilds and register commands
per guild, never globally (the production guild keeps v4's commands).

### Edge contract (admin origin)

For a reverse proxy in front of the admin origin (`deploy/README.md`
"Reverse proxy"); owned and applied by whoever runs the proxy.

- Topology: the admin listener binds a private address on the proxy's
  container network (`KANADE_ALLOW_PRIVATE_BIND=1`,
  `KANADE_ADMIN_BIND=<kanade's address>:8080`), and `KANADE_TRUSTED_PROXY` is
  the proxy container's fixed address on that network. Kanade refuses a
  loopback `KANADE_TRUSTED_PROXY` whenever Tailscale sign-in is enabled,
  because every local process shares the loopback address.
- Shared secret: at least 32 random bytes (for example `openssl rand -base64
  48`), one line, stored as a secret file on both sides; kanade reads it from
  the secrets file `edge_secret`. Rotate by replacing both files and restarting
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
- Without the `edge_secret` file, `KANADE_TRUSTED_PROXY` is trusted by
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
`KANADE_SHUTDOWN_TIMEOUT_SECONDS` (default 10, range 1–120), or less when
live serve's shutdown budget (above) has less left; a drain the configured
timeout cuts is an error exit, one the budget cuts is not. Logs are JSON and
emit only safe configuration-error descriptions, not environment values.

## Deliberate boundaries

Live `serve` runs the Discord gateway, roster sync, the chat pilot,
extraction and the delivery tick, chat and extraction each behind its
settings switch (see "Live serve"). There is no `export` command.
`ctl emojis [--dry-run] [--dir PATH]` uploads the difficulty pills
(`assets/emojis/diff_{n,h,c,x}.png`, drawn by `scripts/emojis/draw_pills.py`)
as application emojis: it needs only the secrets file `discord_token`, reads the
application from `GET /applications/@me`, lists its emojis and uploads only
the fixed names `diff_n`, `diff_h`, `diff_c`, `diff_x` that are missing,
from `PATH/<name>.png`. Without `--dir` it reads `KANADE_EMOJI_DIR` (the
image sets `/app/assets/emojis`, where it ships the PNGs), else
`assets/emojis` relative to the working directory. It never deletes or
renames an emoji and is safe to rerun; it prints one line per pill
(`present`, `uploaded`, `missing` on a dry run, `not uploaded (<path>:
<error>)`, `upload failed (<label>)`) and exits nonzero when the list fails
or a pill is not in place. A dry run uploads nothing but still reads each
missing pill's PNG, so an unreadable one fails the dry run too; its readable
missing pills are not a failure. An ambiguous upload may have landed: rerun,
which lists first.
It opens no gateway session, so it can run beside a live bot. Live `serve`
lists the application's emojis once at startup and maps the pills it finds
to the redesigned cards' and the admin preview's difficulty marks; a
missing pill or a failed list keeps the written label
(`difficulty_marks`/`difficulty_marks_unavailable`). Restart serve after
uploading.
`backup [--name FILE]` is the deploy-time snapshot:
it needs only `KANADE_DB_PATH`, `KANADE_OWNER_LOCK_DIR` and
`KANADE_BACKUP_DIR`, refuses when no store exists (it never creates one) and,
through the owner lock, while serve owns the store (exit `69`, "stop the bot
first"); it writes `FILE` (a plain `[A-Za-z0-9._-]` name; default
`kanade-<YYYYMMDDTHHMMSSZ>.sqlite`) with `VACUUM INTO` (0600) plus
`FILE.manifest.json` (`created_at` added to `kanade.backup.v1`), never
overwriting either (exit `78`), and prints the history head, revision and
schema. Opening the store runs pending migrations, so run it with the image
that last served the store. With a `recipients` file in `KANADE_BACKUP_DIR`, the
snapshot is encrypted with age to those X25519 recipients and published as
`FILE.age` (0600) beside a plaintext `FILE.age.manifest.json` (metadata
only); the plaintext copy exists only in the 0700 staging directory and is
deleted after encryption or with the directory on any failure. `--name`
refuses `*.age`. Three key tools need no store or settings:
`backup keygen --out PATH` writes a new identity (0600, never overwrites) and
prints only its `age1…` public key on stdout; `backup encrypt [--recipients
PATH]` (default: `KANADE_BACKUP_DIR/recipients`) encrypts stdin to stdout;
`backup decrypt --identity PATH --in PATH --out PATH` writes a new 0600 file,
refuses an existing `--out`, fails before writing on a wrong identity and
removes a partial output when the input is damaged or truncated. Key and
recipient errors name the line number, never its content. The files are
standard age, so `age -d -i` also opens them. `serve --offline` wires no
scheduler, persistence, Discord, admin API or mutation route.

`restore --from PATH [--identity FILE] [--apply]` replaces the store with a
`kanade backup` snapshot while serve is stopped; it is a dry run unless
`--apply`. It needs only `KANADE_DB_PATH` and `KANADE_OWNER_LOCK_DIR`
(creating both directories 0700 like serve on an empty volume). It first
opens the current store through the owner lock (exit `69`, "stop the bot
first", while serve owns it; that open applies this build's pending
migrations, and a current store newer than this build is refused, exit
`78`: use a newer image or the tarball rollback). An `.age` snapshot needs
`--identity` (exit `64` without it) and is decrypted into a hidden 0700
`.<db>.restore-<uuid>/` directory beside the database (0600, removed on
every exit). The snapshot is restored there by the store (`integrity_check`,
the migration ledger and checksums, a schema no newer than this build, then
this build's migrations) and closed; a garbage, corrupt, newer-schema or
wrong-identity snapshot is refused (exit `78`) with nothing outside that
directory written. `--apply` then closes the current store, keeps it beside
the database as `<db>.pre-restore-<YYYYMMDDTHHMMSSZ>` (a hard link; any
`-wal`/`-shm`/`-journal` file is renamed along), renames the restored copy
over the database (one atomic rename) and reopens it once, holding the
owner lock of both the current store and the copy from the close to the
reopen (a bot started meanwhile exits `69`, never opening either). It
prints the snapshot's schema and history head and the side copy's name;
delete the side copy once the restore is confirmed (it is member data).
Neither `--from` nor a store path is followed through a symbolic link.

`portal open|close|status` and `sessions list|revoke HANDLE|sign-out-all`
talk to the running bot's admin API, never the store. The target is
`KANADE_ADMIN_BIND` (or `--url http://HOST:PORT`): loopback, or the
listener's private address with `KANADE_ALLOW_PRIVATE_BIND=1`, as for
`healthcheck`. Each run signs in with the break-glass token from
the secrets file `admin_token` (`POST /api/admin/auth/token`; plain
`KANADE_ADMIN_TOKEN` is refused), keeps the session cookie and CSRF token in
memory only, sends `Host`/`Origin` of the listener (`KANADE_ADMIN_HOST` with
`https://`, else `localhost:PORT` with `http://`), and signs out
(`POST /api/admin/auth/logout`) at the end, also after a failed step.
`portal` reads or sets `self_service.public_portal` (`GET`/`PATCH
/api/admin/config`, the PATCH with a fresh `Idempotency-Key`) and prints
`public portal switch: open|closed`. `sessions` acts on the break-glass
token's own sessions (the API's `/api/admin/me/sessions` lists only the
caller's identity): `list` prints handle, method, device and times with the
command's own marked `(this command)`, `revoke` ends one by its 24-character
handle, `sign-out-all` ends every other one and then its own. Exit codes: `69`
when the API is unreachable, slow (10 s per request), throttled or failing,
with nothing attempted after a failed sign-in; `78` for a refused token,
Host or CSRF; `64` for a refused request (an unknown handle). Token sign-in
allows five attempts a minute per client, so a burst of commands can answer
`429` (exit `69`). Output and errors never carry the token, the cookie or the
CSRF token. `token rotate --out FILE [--force]` writes a new break-glass
token (32 random bytes, base64url, one line) 0600 without following a
symbolic link, refuses an existing file unless `--force` (then a temporary
file beside it is renamed over), and prints only the path; restarting the
bot applies it and ends the sessions of the old token.

Live serve prunes the model logs (extraction, chat and rewrite logs,
processed cached messages, drained notices) older than 90 days by its clock
at startup and then every 24 hours on a monotonic timer, off the request
path, then checkpoints the store's WAL into the database and truncates it
(`PRAGMA wal_checkpoint(TRUNCATE)` on the write connection, held only for
the checkpoint; readers still open can leave it `busy`, which is not an
error). Prune and checkpoint share one 60 s limit per run (the store
deletes in short batches; a prune that hits the limit skips that day's
checkpoint), failures only log, and shutdown cancels a run in progress
before the store closes.

The runtime installs Rustls' `ring` provider before command processing. SQLx
and Twilight are intentionally absent until storage and Discord work needs
them, so the production graph has no SQLx MySQL/RSA path from the spike.

## Source layout

The repository is a Cargo workspace: one `kanade` binary (the root package)
over library crates in `crates/`, one `Cargo.lock`, shared exact pins in
`[workspace.dependencies]`. Each crate builds and tests on its own
(`cargo test -p kanade-store`); the root `test-support` feature turns on every
crate's.

| Crate | Holds | Depends on |
|---|---|---|
| `kanade-log` | JSON log lines (`log!`, `event`, `emit`) | — |
| `kanade-domain` | pure rules: scheduling, attendance, completion, notify planning, settings, history | — |
| `kanade-store` | SQLite store and delivery journal, both migration sets | domain, log |
| `kanade-llm` | OpenAI-compatible model gateway and governor | domain |
| `kanade-files` | boss catalog, boss knowledge, art and guide source loaders | domain, log |
| `kanade-chardata` | NEXON client, scouter client, character data service | domain, store, log |
| `kanade-voice` | persona bundles, policy prompts, nudge rewrites | domain, llm, files, log |
| `kanade-extract` | extraction rules and pipeline, rescans | domain, llm, voice |
| `kanade-chat` | chatbot driver, tools, context, gate | domain, llm, voice, extract |

The root package keeps what composes them:

```text
src/
├── main.rs        # crypto provider, process inputs, exit codes
├── lib.rs         # module groups; `api` needs feature `web`, `bot` feature `bot`
├── app/           # shared by both sides: scheduler writer, store reads, guild facts,
│                  #   staff rule, settings desk, rescans, roster persistence, ports
├── discord/       # Discord REST transport, mentions, guild cache, identity, reconcile (no gateway)
├── cards/         # card rendering, previews, wording, estimate chart
├── bot/           # the Discord gateway bot (feature `bot`)
├── api/           # admin and public HTTP APIs (feature `web`)
├── cli/           # command-line parsing, healthcheck, `portal`/`sessions` client (web)
└── runtime/
    ├── application.rs, config/, secrets.rs, error.rs, logging.rs, tls.rs, backup/, restore.rs
    └── serve/     # shared core (store, settings, health, retention, character data)
        ├── bot/   # gateway, tick, chat, extraction, commands (feature `bot`)
        └── web/   # listeners, sessions, setup secrets, REST guild sync (feature `web`)
```

`src/bot` and `src/api` never import each other; what both need lives in
`app`, `discord` or `cards`. Integration suites stay in the root `tests/`,
each with the features it needs (`required-features` in `Cargo.toml`).

### Bot and web as separate builds

The root features `bot` and `web` (default both) choose what the binary can
run; `runtime.components` chooses what a `serve` runs. A bot-only build has no
HTTP framework and no `src/api`; a web-only build has no gateway crate and no
`src/bot`. Naming a component the build lacks refuses startup.

```sh
cargo build --release --no-default-features --features bot   # Discord bot only
cargo build --release --no-default-features --features web   # admin app and portal only
```

The image is built with both. One `serve` owns a store (a second process is
refused), so bot and web on the same data always run in one process; pick
the components per deployment, not per process on a shared store.

## Log events

JSON lines on stderr (`level`, `event`, fields). None carries question or reply text, member ids, persona text or secrets; `interaction_id` is the chat-log row id.

| Event | Level | Fields | When |
| --- | --- | --- | --- |
| `config_shadowed` | WARN | `keys` (the `kanade.toml` keys, never the values) | `serve` start when a non-empty `KANADE_*` variable overrides a key the file sets |
| `server_started` | INFO | `mode` (`offline`/`live`), `bind`, `version` (the crate version) | each listener (admin, and public when set) is bound |
| `backup_dir_unreadable` | WARN | no additional fields | checkpoints could not list `KANADE_BACKUP_DIR` (it lists no backups) |
| `backup_manifests_skipped` | WARN | `unreadable`, `missing_snapshot` (counts), `truncated` (more than 4096 entries) | checkpoints skipped unreadable or foreign manifests or ones whose snapshot is gone |
| `model_logs_pruned` | INFO | `cutoff` (ISO-8601 UTC), `extractions`, `chats`, `messages`, `notices`, `rewrites` (deleted counts) | each retention run of live serve (startup, then daily) |
| `model_logs_prune_failed` | WARN | no additional fields | a retention run failed in the store (retried the next day) |
| `model_logs_prune_cut` | WARN | `limit_seconds` | a retention run hit its time limit; the rest goes the next day |
| `wal_checkpointed` | INFO | `busy` (a reader or writer kept it from finishing), `log`, `checkpointed` (WAL frame counts SQLite reported) | after each retention run's prune |
| `wal_checkpoint_failed` | WARN | no additional fields | the checkpoint failed in the store (retried the next day) |
| `wal_checkpoint_cut` | WARN | `limit_seconds` | the checkpoint ran out the run's time limit |
| `persona_selected` | INFO; WARN on a fallback source, any candidate issue, unreadable profiles or `profiles_issue` | `configured`, `effective`, `source` (`configured`/`catalog_default`/`tracked_fallback`), `bundle_file`, `profiles`, `profile_ids`, `unreadable_profiles` [{`file`,`error`}], `profiles_issue`, `issues` [{`candidate`,`error`}] | serve startup, after the persona files load |
| `persona_unavailable` | ERROR | `configured`, `issues` | serve startup when no persona validates (chat stays off) |
| `settings_changed` | INFO | `revision`, `section`, `keys` (stored keys), `values` {key: {`from`,`to`}}, `actor` (`kind:id`), `surface` (`admin_portal`) | every saved config `PATCH` that changed something |
| `persona_switched` | INFO | `from`, `to`, `profiles` | a persona switch was saved and swapped in |
| `personas_reloaded` | INFO; WARN with unreadable profiles | `persona`, `profiles`, `issues` [{`file`,`error`}] | profile reload |
| `chat_setup_changed` | INFO; WARN when enabled but not ready | `enabled`, `ready`, `not_ready` (`no_model_route`/`no_persona`) | chat start, then when either flag flips (read on the next message, status read or settings change) |
| `chat_admitted` | INFO | `interaction_id`, `channel` (`thread`/`channel`), `position` (null when it runs at once) | the gate took a question |
| `chat_ignored` | INFO | `reason` (`disabled`/`not_ready`/`not_chat_category`/`no_pilot_role`/`staff_only`/`rate_limited`/`shed`/`bot_author`) | a message that summoned the bot was not taken; never for ordinary chatter |
| `chat_answered` / `chat_failed` | INFO / WARN | `interaction_id`, `outcome`, `persona`, `profile`, `profile_source` (`saved`/`role`/`default`), `saved_style_unavailable`, `model`, `reasoning`, `route` (`homelab`/`external_unmasked`; `external_masked` is historical), `rounds`, `tools`, `latency_ms`, `model_ms`, `tools_ms`, `clean_retry`, `withheld` | a question concluded after a model attempt |
| `identity_leak_blocked` | WARN (historical) | `role`, `kinds`, `count` | retired boundary-scanner event; current calls do not emit it |
| `former_names_evicted` | WARN (historical) | `remembered` | retired masked-chat name-history event; current calls do not emit it |
| `chat_members_unreadable` | WARN | no additional fields | the roster could not be loaded; chat uses an empty roster and this is not a masking refusal |
| `chat_cancelled` | INFO | `interaction_id`, `reason` (`deleted`/`shutdown`/`expired`/`not_admitted`/`not_ready`/`aborted`) | an admitted question ended without an answer |
| `difficulty_marks` | INFO; WARN when a pill is missing | `missing` (pill names not found) | serve startup, after listing the application's emojis |
| `difficulty_marks_unavailable` | WARN | `reason` (transport label or `timeout`) | serve startup when the emoji list failed; every difficulty is written out |
| `estimate_refresh_pass` | INFO | `runs`, `due`, `queued`, `cap_left` (background lookups left today), `skipped` (why nothing was queued, e.g. `unreachable`, `verification_required`) | each background estimate refresh pass (with a scouter URL only: 10 minutes after start, then hourly) |
| `estimate_finished` | INFO; WARN with a failure | `state` (`done`/`failed`), `source` (`manual`/`background`), `characters`, then `failed`, `total_failed`, `cache_hit` (done) or `code`, `detail`, `status` (failed); never an IGN or a result | an estimate lookup ended |
