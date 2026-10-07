# v5 runtime bootstrap

`kanade` is the single Rust executable, version `1.0.0-beta.3` (the release
label everywhere: Cargo, CHANGELOG and the planned `v1.0.0-beta.3` tag).

## Available now

The offline development server and live `serve` (Discord gateway, roster
sync, delivery tick, the chat pilot and extraction (behind its settings
switch) for the configured guild):

```sh
KANADE_TIMEZONE=Asia/Kuala_Lumpur kanade serve --offline
kanade serve        # live: the "Serve environment" below is required
KANADE_HEALTHCHECK_URL=http://127.0.0.1:8080/healthz kanade healthcheck
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
warning: UNMASKED: chat model codex-like leaves the homelab; raw member names, IDs, messages, and complete URLs are sent to the provider. Kanata ZDR is operator-stated retention only and does not prevent transmission.
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
With `KANADE_DISCORD_GATEWAY=0`, `scheduler`/`discord` are `disabled` and
only storage decides. The live gateway also reports `extraction`
(informational, never degrades `status`): `disabled` (switched off or
paused), `degraded` (on, but no model gateway or extraction model, or the
latest extraction call `failed` or was `turned_away`), `running` (a rescan
job is queued or running) or `idle`.
`healthcheck` accepts the exact offline document or a live one with `status`
and `storage` `ok`, ignoring extra fields.
Binds are loopback-only unless `KANADE_ALLOW_PRIVATE_BIND=1` also admits a
private address (RFC 1918, IPv4 link-local, IPv6 `fc00::/7` and `fe80::/10`)
on the internal edge network; wildcard (`0.0.0.0`, `::`) and public addresses
are always refused. Runtime configuration comes from the process
environment and, when `KANADE_CONFIG` names one, the `kanade.toml` file
("Config file" below); `.env` is not loaded automatically. `healthcheck` accepts only a
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
| `KANADE_WEB_DIR` | unset | Web workspace root; serves `apps/admin/dist` and `apps/public/dist` (same layout as `devtools/pwa-mock`). Unset serves no shell. |
| `KANADE_BOSS_DIR` | unset | Private boss art root (`portraits/`, `portraits/icon/`, `artwork/entry/`). Unset or missing art is 404. |
| `KANADE_IDENTITY_DIR` | unset | Cached `avatar.*`/`banner.*`; unset serves generated SVG stand-ins. With the gateway on, serve creates it (`0700`) and, after each `READY` and on the bot's own nickname/avatar/user changes, fetches the guild avatar (else user avatar) and banner from Discord's CDN (png/webp/gif/jpeg, ≤ 8 MiB, 20 s timeout, no redirects), replacing files by temp + rename; no avatar/banner removes the file (monogram/wash). Logs `identity_cached {avatar, banner}`; failures log WARN `identity_refresh_failed {kind, reason}` and keep the last files. Compose sets `/data/identity`. |
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
| `KANADE_DISCORD_TOKEN_FILE` | required | Bot token file (one line, ≤ 4 KiB); read at startup (also with the gateway off), so a missing secret fails the deploy. Plain `KANADE_DISCORD_TOKEN`/`DISCORD_TOKEN` are refused by every command. |
| `KANADE_EXPECT_V4_STOPPED` | `0` | `0` or `1`. Must be `1` before the gateway connects ("stop the v4 container first"): checked before the store opens. Set it only after `docker stop kanade-bot`. Not checked with `KANADE_DISCORD_GATEWAY=0`. |
| `KANADE_DISCORD_GATEWAY` | `1` | `0` or `1`. `0` serves the admin API only: no gateway, roster sync or delivery tick (tests, maintenance). |
| `KANADE_GUILD_ID`, `KANADE_BOSSING_ROLE_ID` | required | Snowflakes. |
| `KANADE_ADMIN_ROLE_ID`, `KANADE_CHAT_PILOT_ROLE_ID` | unset | Snowflakes. |
| `KANADE_DEBUG_USER_IDS` | empty | Snowflake list. |
| `KANADE_TEST_CHANNEL_ID` | unset | Snowflake. Where `/debug ping` and `/debug header` post without `channel:`. Unset: a run's home channel (else the post channel); sample runs and `/debug header` use the invoking channel, the week's `digest` the post channel. A run's test card posted anywhere but its home channel is display only (reactions do nothing, never refreshed). |
| `KANADE_DB_PATH`, `KANADE_OWNER_LOCK_DIR` | required | Absolute paths without `..`. The lock directory and the database's directory are created `0700` when absent (existing ones are never re-moded); ownership, symlink and mode checks run when the store opens. A second process on the same store is refused. |
| `KANADE_BACKUP_DIR` | unset | Absolute path without `..`. Serve only reads it: History checkpoints (`GET /api/admin/history/checkpoints`) list the `kanade backup` manifests there on every request; unset reports `backup_dir_configured: false`. Compose mounts it read-only at `/backups`. |
| `KANADE_CATALOG_FILE` | `boss/bosses.yaml` | Boss catalog. |
| `KANADE_KNOWLEDGE_DIR` | unset | Boss knowledge root. |
| `KANADE_PERSONA_DIR` | `config/personas` | Persona layout root. |
| `KANADE_MODEL_BASE_URL` | unset | `https`, or `http` only to loopback, `localhost` or `host.docker.internal`; no userinfo or query. Unset disables models; the key, CA and alias variables then are refused. |
| `KANADE_MODEL_KEY_FILE`, `KANADE_MODEL_CA_FILE` | unset | Bearer key file (plain `KANADE_MODEL_KEY` is refused) and a CA file (PEM bundle or one DER certificate) that replaces the compiled webpki roots. |
| `KANADE_EXTRACT_MODEL`, `KANADE_CHAT_MODEL`, `KANADE_REWRITE_MODEL` | unset | Model aliases (printable ASCII, ≤ 200); seeds for a role with no saved alias. |
| `KANADE_EXTRACT_REASONING`, `KANADE_CHAT_REASONING`, `KANADE_REWRITE_REASONING` | unset | Reasoning seeds (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`; chat and rewrite also `inherit` = extraction's level): used only where no `extract_reasoning` / `chat_pilot_think` / `v5.rewrite_reasoning` row is saved (row → env → default: extraction `off`, chat/rewrite `inherit`). Need `KANADE_MODEL_BASE_URL`. |
| `KANADE_MODEL_PERMITS` | `2` | Concurrent model calls in the single `gateway` group, 1–16. |
| `KANADE_MODEL_GROUPS` | unset | Capacity groups as a JSON list of `{name, permits, aliases}` (normally `[[models.groups]]` in `kanade.toml`): names unique, ≤ 64 of `[A-Za-z0-9._-]`; permits 1–16; each alias in one group. Non-empty replaces the `gateway` group; exclusive with `KANADE_MODEL_PERMITS`; needs `KANADE_MODEL_BASE_URL`. A role whose alias is in no group starts with an `ungrouped` warning and its calls are refused; switching a role to such an alias in the config API is refused. Without groups, an alias a role is switched to live joins the `gateway` group. The admin config view reports the groups the governor runs (`models.groups`, read-only). |
| `KANADE_MODEL_CONTEXT` | unset | JSON seed from `[models.context]`: cloud/local defaults, per-role `{reserve, cap?}` and alias overrides, all required except `cap` and `overrides`. Override keys match route aliases exactly, so an override on a base alias does not apply to its `<base>:<level>` variants. It applies only when `v5.model_context` is unsaved; the Config API is the live writer. A window (default, cap or override) above 131072 is clamped to 131072 and logged once when the seed applies (`model_context_clamped`, WARN, `fields`, `max`). A zero window or reserve, a reserve above 131072, an unknown field, a reserve not smaller than its role's seed window (its cap, else the smaller zone default), or a reserve of 15360 or more refuses startup naming the field: each model call reserves its prompt estimate (request bytes / 4) plus `max_tokens` out of a 16384-token call budget and must leave at least 1024 for the prompt, else every call fails before sending (`budget_exceeded`). A Config save of such a reserve answers 422 naming the role, reserve and budget. Needs `KANADE_MODEL_BASE_URL`. |
| `KANADE_RUN_LENGTHS` | unset | JSON seed from `[settings.run_lengths]`: `{default_minutes, overrides}`. It applies only when `v5.run_lengths` is unsaved; Config is the live writer. The default is 5-240 minutes; each `{boss, difficulty, minutes}` override is 5-480 and must name an exact catalog key and valid lowercase difficulty when saved through Config. Every admin run, including own-time runs, reports the sum across its bosses. |
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
ignored, as for the HTTP settings. **Exception:** the retired
`KANADE_PSEUDONYMIZE` and `KANADE_ALLOW_EXTERNAL_UNMASKED` names, and their
TOML counterparts `models.pseudonymize` and `models.allow_external_unmasked`,
are rejected on presence, even when empty or false. Remove both from private
deployment settings before a separately approved rollout; an old config must
not silently switch formerly masked calls to raw traffic. Configured external
roles send raw names, IDs, messages and URLs when present, with a warning that
data leaves the homelab. Unknown or not-yet-listed aliases are treated as
external until a successful listing establishes their zone. Kanata provider
ZDR is operator-stated retention, not a limit on transmission.

### Config file

`KANADE_CONFIG` names a TOML file (in Compose: the private, git-ignored
`kanade.toml` at the checkout root, mounted read-only at
`/config/kanade.toml`; the tracked `kanade.example.toml` documents every key)
for non-secret settings. Unset or empty keeps the environment-only behaviour.
Each key sets one variable below, whose rules apply unchanged
(`src/runtime/config/file/`); all commands (`serve`, `healthcheck`, `import`,
`models check`) read it.

- Precedence: a non-empty `KANADE_*` environment variable (Compose
  `environment:` or `env_file`) overrides its key; an empty one does not, so a
  blank Compose variable cannot clear a file value.
- Secrets never go in the file: a key containing `token`, `secret`, `key`,
  `password`, `passwd` or `credential` that does not end in `_file` stops
  startup; `*_file` keys name the secret files.
- Unknown keys or tables, wrong TOML types (e.g. `tick_seconds = "30"`) and
  syntax errors stop startup with exit `78`, naming the key (or the line for
  syntax), never the value. A value that fails the variable's own rule names
  both, e.g. ``KANADE_TIMEZONE must be a valid IANA timezone (kanade.toml
  `runtime.timezone`)``. The file is at most 1 MiB.
- Booleans are `true`/`false` (written as `1`/`0`); lists are TOML arrays
  (joined with commas, so string items may not contain one); snowflakes may be
  strings or integers (strings keep ids above `i64` exact).
- `[[models.groups]]` takes exactly `name`, `permits` and `aliases`.

| Key | Variable | TOML type |
|---|---|---|
| `runtime.timezone` | `KANADE_TIMEZONE` | string |
| `runtime.instance_id` | `KANADE_INSTANCE_ID` | string |
| `runtime.tick_seconds` | `KANADE_TICK_SECONDS` | integer |
| `runtime.shutdown_timeout_seconds` | `KANADE_SHUTDOWN_TIMEOUT_SECONDS` | integer |
| `runtime.allow_private_bind` | `KANADE_ALLOW_PRIVATE_BIND` | bool |
| `runtime.healthcheck_url` | `KANADE_HEALTHCHECK_URL` | string |
| `runtime.healthcheck_timeout_seconds` | `KANADE_HEALTHCHECK_TIMEOUT_SECONDS` | integer |
| `admin.bind` | `KANADE_ADMIN_BIND` | string |
| `admin.host` | `KANADE_ADMIN_HOST` | string |
| `admin.trusted_proxy` | `KANADE_TRUSTED_PROXY` | string |
| `admin.edge_secret_file` | `KANADE_EDGE_SECRET_FILE` | string |
| `admin.web_dir` | `KANADE_WEB_DIR` | string |
| `admin.boss_dir` | `KANADE_BOSS_DIR` | string |
| `admin.identity_dir` | `KANADE_IDENTITY_DIR` | string |
| `admin.token_file` | `KANADE_ADMIN_TOKEN_FILE` | string |
| `admin.discord_client_id` | `KANADE_ADMIN_DISCORD_CLIENT_ID` | snowflake (string or integer) |
| `admin.discord_client_secret_file` | `KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE` | string |
| `admin.discord_redirect_uri` | `KANADE_ADMIN_DISCORD_REDIRECT_URI` | string |
| `admin.tailscale_logins` | `KANADE_ADMIN_TAILSCALE_LOGINS` | string list |
| `admin.session_idle_minutes` | `KANADE_ADMIN_SESSION_IDLE_MINUTES` | integer |
| `admin.session_absolute_hours` | `KANADE_ADMIN_SESSION_ABSOLUTE_HOURS` | integer |
| `public.bind` | `KANADE_PUBLIC_BIND` | string |
| `public.host` | `KANADE_PUBLIC_HOST` | string |
| `public.cloudflared_peer` | `KANADE_CLOUDFLARED_PEER` | string |
| `discord.token_file` | `KANADE_DISCORD_TOKEN_FILE` | string |
| `discord.gateway` | `KANADE_DISCORD_GATEWAY` | bool |
| `discord.guild_id` | `KANADE_GUILD_ID` | snowflake (string or integer) |
| `discord.bossing_role_id` | `KANADE_BOSSING_ROLE_ID` | snowflake (string or integer) |
| `discord.admin_role_id` | `KANADE_ADMIN_ROLE_ID` | snowflake (string or integer) |
| `discord.chat_pilot_role_id` | `KANADE_CHAT_PILOT_ROLE_ID` | snowflake (string or integer) |
| `discord.debug_user_ids` | `KANADE_DEBUG_USER_IDS` | snowflake list |
| `discord.test_channel` | `KANADE_TEST_CHANNEL_ID` | snowflake (string or integer) |
| `store.db_path` | `KANADE_DB_PATH` | string |
| `store.owner_lock_dir` | `KANADE_OWNER_LOCK_DIR` | string |
| `files.catalog_file` | `KANADE_CATALOG_FILE` | string |
| `files.knowledge_dir` | `KANADE_KNOWLEDGE_DIR` | string |
| `files.persona_dir` | `KANADE_PERSONA_DIR` | string |
| `models.base_url` | `KANADE_MODEL_BASE_URL` | string |
| `models.key_file` | `KANADE_MODEL_KEY_FILE` | string |
| `models.ca_file` | `KANADE_MODEL_CA_FILE` | string |
| `models.permits` | `KANADE_MODEL_PERMITS` | integer |
| `models.groups` | `KANADE_MODEL_GROUPS` | array of tables |
| `models.context` | `KANADE_MODEL_CONTEXT` | table |
| `settings.run_lengths` | `KANADE_RUN_LENGTHS` | table |
| `models.extraction.model` | `KANADE_EXTRACT_MODEL` | string |
| `models.extraction.reasoning` | `KANADE_EXTRACT_REASONING` | string |
| `models.chat.model` | `KANADE_CHAT_MODEL` | string |
| `models.chat.reasoning` | `KANADE_CHAT_REASONING` | string |
| `models.rewrite.model` | `KANADE_REWRITE_MODEL` | string |
| `models.rewrite.reasoning` | `KANADE_REWRITE_REASONING` | string |
| `settings.post_channel_id` | `KANADE_POST_CHANNEL_ID` | snowflake (string or integer) |
| `settings.watch_channel_ids` | `KANADE_WATCH_CHANNEL_IDS` | snowflake list |
| `settings.watch_category_ids` | `KANADE_WATCH_CATEGORY_IDS` | snowflake list |
| `settings.chat_category_ids` | `KANADE_CHAT_CATEGORY_IDS` | snowflake list |
| `settings.extraction_enabled` | `KANADE_EXTRACTION_ENABLED` | bool |
| `settings.chat_enabled` | `KANADE_CHAT_ENABLED` | bool |
| `settings.boss_week_reset_weekday` | `KANADE_BOSS_WEEK_RESET_WEEKDAY` | string |
| `settings.boss_week_reset_time` | `KANADE_BOSS_WEEK_RESET_TIME` | string |
| `settings.day_of_ping_time` | `KANADE_DAY_OF_PING_TIME` | string |
| `settings.countdown_minutes` | `KANADE_COUNTDOWN_MINUTES` | integer list |

Variables without a key: `KANADE_CONFIG` itself; the refused plain
secrets (`KANADE_DISCORD_TOKEN`, `KANADE_MODEL_KEY`, `KANADE_ADMIN_TOKEN`,
`KANADE_ADMIN_DISCORD_CLIENT_SECRET`) and the renamed `KANADE_BIND` /
`KANADE_PILOT_CHANNEL_IDS`.

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
{"level":"WARN","event":"model_warning","kind":"external_unmasked","message":"UNMASKED: rewrite model ext leaves the homelab; raw member names, IDs, messages, and complete URLs are sent to the provider. Kanata ZDR is operator-stated retention only and does not prevent transmission."}
```

`models_degraded` (WARN, `reason`) replaces `models_listed` when the
listing fails (every route then stays external until one succeeds);
`models_disabled` is logged without `KANADE_MODEL_BASE_URL`. `source` is
`fixed` (a `<base>:<level>` variant), `floor` (the configured level is not
accepted; the lowest accepted one is used), `inherit` (extraction's level),
else where the role's own level came from: `stored`, `env` or `default`.
`route` is currently `homelab` or `external_unmasked`; `external_masked` and
`external_refused` are historical values only. Every external route, including
an unknown alias until a listing classifies it, gets an `external_unmasked`
WARN with the raw-data and ZDR-retention notice. `capacity` and `ungrouped`
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
stack refused them (the save stands and a notice says they apply at
restart). Chat reads its route once when a question is prepared; the
system prompt's runtime line, permit, requests, chat row and
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
`KANADE_EXPECT_V4_STOPPED`, opens and owns the store, loads the catalog,
knowledge and personas, resolves settings, then serves both listeners: one
shared `SchedulerWriter`, `ApiState` (schedule policy from settings, one
`GuildAccess` shared with the staff gate, guild id for card links, the
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

Runbook (production token, real guild): `docker stop kanade-bot` (v4) first,
then set `KANADE_EXPECT_V4_STOPPED=1` and start v5; to roll back, stop v5,
set it back to `0` and `docker start kanade-bot`.

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

Headers match `devtools/pwa-mock` (CSP, report-only Trusted Types, `nosniff`,
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
| `GET /art/{portraits,icons,entry,animated}/{key}` (`animated` honours a single byte `Range`) | file or 404 | `503 closed` |
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
answer `503 auth_unavailable`. Live `serve` composes both (above) with
the gateway's `GuildCache` as the channel list. Gateway wiring (see
"Live serve"): `BotEvent::Roster` → `api::auth::roster::on_roster_update`,
`BotEvent::GuildAvailable` → role pruning then `on_guild_available`, both on
one sequential roster task with startup reconciliation, so the Discord
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
as application emojis: it needs only `KANADE_DISCORD_TOKEN_FILE`, reads the
application from `GET /applications/@me`, lists its emojis and uploads only
the fixed names `diff_n`, `diff_h`, `diff_c`, `diff_x` that are missing,
from `PATH/<name>.png` (default `assets/emojis`, relative to the working
directory). It never deletes or renames an emoji and is safe to rerun; it
prints one line per pill (`present`, `uploaded`, `missing` on a dry run,
`not uploaded`, `upload failed (<label>)`) and exits nonzero when the list
fails or a pill is not in place (a dry run's missing pills are not a
failure). An ambiguous upload may have landed: rerun, which lists first.
It opens no gateway session, so it can run beside a live bot. Live `serve`
lists the application's emojis once at startup and maps the pills it finds
to the redesigned cards' and the admin preview's difficulty marks; a
missing pill or a failed list keeps the written label
(`difficulty_marks`/`difficulty_marks_unavailable`). Restart serve after
uploading.
`import v4` is the one-off testing import from a v4 snapshot
(`v4-import.md`). `backup [--name FILE]` is the deploy-time snapshot:
it needs only `KANADE_DB_PATH`, `KANADE_OWNER_LOCK_DIR` and
`KANADE_BACKUP_DIR`, refuses when no store exists (it never creates one) and,
through the owner lock, while serve owns the store (exit `69`, "stop the bot
first"); it writes `FILE` (a plain `[A-Za-z0-9._-]` name; default
`kanade-<YYYYMMDDTHHMMSSZ>.sqlite`) with `VACUUM INTO` (0600) plus
`FILE.manifest.json` (`created_at` added to `kanade.backup.v1`), never
overwriting either (exit `78`), and prints the history head, revision and
schema. Opening the store runs pending migrations, so run it with the image
that last served the store. `serve --offline` wires no scheduler, persistence,
Discord, import, admin API or mutation route.

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
    ├── serve/           # live serve: store, settings, API composition, health, discord/ (gateway, roster, tick)
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

## Log events

JSON lines on stderr (`level`, `event`, fields). None carries question or reply text, member ids, persona text or secrets; `interaction_id` is the chat-log row id.

| Event | Level | Fields | When |
| --- | --- | --- | --- |
| `backup_dir_unreadable` | WARN | no additional fields | checkpoints could not list `KANADE_BACKUP_DIR` (it lists no backups) |
| `backup_manifests_skipped` | WARN | `unreadable`, `missing_snapshot` (counts), `truncated` (more than 4096 entries) | checkpoints skipped unreadable or foreign manifests or ones whose snapshot is gone |
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
