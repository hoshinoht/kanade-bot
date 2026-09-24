# Setup

## Discord developer portal setup

1. Go to <https://discord.com/developers/applications> → **New Application**.
   Name it whatever you like.
2. **Bot** tab → **Reset Token** → copy it. This is `DISCORD_TOKEN`; treat it
   like a password. Never commit it.
3. Still on the **Bot** tab, under *Privileged Gateway Intents*, enable **both**:
   - **Server Members Intent** — the roster is derived from the bossing role.
   - **Message Content Intent** — the extractor reads chat. (The bot declares
     both, so if either is off it exits with a clear error instead of hanging.)
4. **OAuth2 → URL Generator**:
   - Scopes: `bot` and `applications.commands`.
   - Bot permissions — exactly these seven (permissions integer `274878000192`):
     **View Channels**, **Send Messages**, **Send Messages in Threads**,
     **Embed Links**, **Read Message History**, **Add Reactions**, and
     **Manage Messages**.
     *Manage Messages* is only used to keep ✅/❌ one-or-the-other: when someone
     switches their answer the bot removes their previous reaction, and without
     it both stick and the attendance tally is wrong. (It never deletes anyone
     else's messages; `/debug clear_test` only removes the bot's own.) The portal's
     **Config → Channel access** table and `/debug status` show, per channel,
     whether it is actually granted.
     *Mention Everyone is not needed*: pinging a run's participants is an
     ordinary user mention, and the bot never pings `@everyone` or a role — every
     message goes out with an allow-list of exactly the users who need to act.
   - Open the generated URL and invite the bot to your server.
5. In Discord: **User Settings → Advanced → Developer Mode** on. Then right-click
   to copy the ids you need:
   - the **server** → `GUILD_ID`
   - *(optional)* a guild-wide channel → `POST_CHANNEL_ID`. Runs normally post in
     their own home channel, so this is only the weekly-digest / fallback channel.
   - the channel(s) the parties chat in → `CHAT_CHANNEL_IDS`, and/or the bossing
     category → `CHAT_CATEGORY_IDS` (both comma separated). **At least one is
     required** — `/fixed add` only works inside a watched channel. Listing the
     category is usually easiest: new party channels are then picked up
     automatically, with no restart.

     > This server's setup uses `CHAT_CATEGORY_IDS`: one category holds every
     > party channel, so `CHAT_CHANNEL_IDS` can stay empty and a channel added
     > for a new party is watched the moment it appears.
   - **Server Settings → Roles**, right-click the bossing role → `BOSSING_ROLE_ID`
     (and optionally an admin role → `ADMIN_ROLE_ID`, which grants `/say`,
     `/debug` and the right to change anyone's run)
6. Make sure the bot's role can see and post in every party channel you will run
`/fixed add` in.

## Configure

```sh
cp .env.example .env
$EDITOR .env          # fill in the ids from step 1
```

`.env.example` lists only what an operator fills in: `DISCORD_TOKEN`, `GUILD_ID`,
`BOSSING_ROLE_ID`, at least one of `CHAT_CHANNEL_IDS` / `CHAT_CATEGORY_IDS`,
`ADMIN_TOKEN`, `DB_OWNER_LOCK_DIR`, the gateway key file and model alias, and an
optional chatbot block. Every other setting has a default and is documented in
the [settings reference](#settings-reference).

With extraction on (the default) or the chatbot configured, the model gateway
settings are required too — see [Model gateway](#model-gateway).

Every persistent writer also needs `DB_OWNER_LOCK_DIR`: one shared absolute
directory owned by the service UID with mode `0700`. Native and manually mounted
deployments must expose the same directory to the bot and any offline recorder;
the example leaves it unset rather than inventing a global or database-sibling
fallback. Read-only health checks and non-recording commands do not need it.

Boss names, levels, aliases and the difficulties each boss actually has live in
[`boss/bosses.yaml`](../boss/bosses.yaml). The catalog is bind-mounted read-only
and loaded at startup, so edit it and restart — no rebuild. It ships with the eleven bosses parties
currently run: Lotus, Chosen Seren, Gatekeeper Kalos, The First Adversary,
Carling, Radiant Malefic Star, Bellona, Limbo, Baldrix, Jupiter and Black Mage.

**Boss portraits are optional.** Drop `MaleficStar.png`, `Kalos.png` and friends into
[`boss/portraits/`](../boss/portraits/README.md) and the portal shows them next
to each boss, and the bot attaches one as the thumbnail on that run's pings. A
boss with no file gets a coloured monogram instead, so nothing shifts either way.
Portraits and entry artwork are served from disk, so their changes appear on the
next page load without a restart.

When the chatbot is configured, `BOSS_KNOWLEDGE_PATH` (default
`boss/knowledge`) must contain `_meta.yaml` and one lowercase YAML document for
every catalog boss. The catalog and knowledge are validated together at startup;
missing, extra, or malformed documents stop the chat-enabled bot until fixed.

Personal chatbot memory has been removed. Opening an older database runs the
v16 upgrade, which takes the usual pre-upgrade snapshot and then drops the
`chat_memor*` tables. That snapshot still holds the old memory rows, so keep or
delete it under your usual private-data handling. Leftover `CHAT_MEMORY_*` values
in `.env` are ignored with one startup warning; delete them at your convenience.

## Model gateway

Every model call — extraction and chatbot — goes to the Kanata inference
gateway, an OpenAI-compatible, tailnet-only HTTPS endpoint that requires a
bearer key. The bot never talks to Ollama directly; Ollama (or any other
backend) runs behind Kanata.

| Setting                | Meaning                                                                                                  |
| ---------------------- | -------------------------------------------------------------------------------------------------------- |
| `KANATA_BASE_URL`      | Gateway URL, default `https://sumi.kanata.hoshinoht.dev`. `https://` only; TLS is always verified.       |
| `KANATA_API_KEY_FILE`  | Path to a file holding the bearer key (whitespace is stripped). Keep it outside the repository.          |
| `EXTRACT_MODEL`        | **Seed.** Kanata alias for extraction. No default; Config → Models owns it after first run.              |
| `CHAT_PILOT_MODEL`     | **Seed.** Kanata alias for the chatbot. No default; Config → Models owns it after first run.             |
| `KANATA_TIMEOUT`       | Seconds per extraction call (default `120`).                                                             |
| `EXTRACT_REASONING`    | **Seed.** `off`, `low`, `medium` or `high` (default `off`, sent as `reasoning_effort: "none"`).          |
| `MODEL_CONTEXT_TOKENS` | Client-side prompt budget (default `8192`). It cannot resize the model: keep it equal to the host model's `num_ctx` (Modelfile) or `OLLAMA_CONTEXT_LENGTH`. |

The four model settings seed the `extract_model`, `extract_reasoning`,
`chat_pilot_model` and `chat_pilot_think` rows on first run (empty aliases and an
empty `CHAT_PILOT_THINK` are not seeded); afterwards the stored selection wins
and changing `.env` does nothing. Pick aliases in **Config → Models** or with
`bossctl config set extract_model <alias>`; a change applies to the next model
call without a restart.

Request bodies follow each alias's capabilities from Kanata's `GET /v1/models`
(the optional `kanata` object on each entry, cached for five minutes):
`response_format` only with `structured_output`, `temperature`/`seed` only with
`sampling_controls`, and `reasoning_effort` only with `reasoning_control`. An alias
without that metadata, or a failed lookup, gets a minimal body (model, messages
and tools); extraction then puts the JSON schema in the prompt and keeps the same
strict validation, single retry and quarantine. The chatbot refuses an alias whose
metadata says `function_tools: false`. If Kanata still answers HTTP 400 naming one
of those optional fields in `error.param`, the bot drops it for that alias for the
cache TTL, retries the call once, and logs one WARNING.

At startup the bot exits with a clear error (after the usual fatal-exit pause)
when extraction is on and neither the database nor `EXTRACT_MODEL` selects an
extraction model, when the chatbot is configured and no chat model is selected
either way, or when either feature is enabled and the
key file is unset, missing, unreadable, empty, or holds characters a bearer
header cannot carry. Error messages name the setting, never its value or the
key. The key itself is never logged, stored, or shown in the portal or
`/debug status`; there is no environment variable that carries it.

`EXTRACT_ENABLED` only seeds the extractor switch stored in the database; after
that the portal/`bossctl` switch wins. If that stored switch is on while
`EXTRACT_ENABLED=false` and the extractor's settings are missing, startup does
not fail (the switch can only be changed through the running bot): it logs one
ERROR naming the problem, and each burst then fails quietly after a single
ERROR until you set the alias/key or turn the extractor off in the portal.

A `KANATA_BASE_URL` ending in `/v1` (the OpenAI SDK convention) is accepted and
trimmed; the bot appends `/v1/...` itself.

Under Compose the key is a Compose secret mounted read-only at
`/run/secrets/kanata_api_key`, and `KANATA_API_KEY_FILE` is fixed to that path.
Point `KANATA_API_KEY_HOST_FILE` at the operator's key file for interpolation —
export it in the shell or pass the private env file explicitly:

```sh
KANATA_API_KEY_HOST_FILE=/path/outside/repo/kanata.key docker compose up --build
# or, with KANATA_API_KEY_HOST_FILE in the root .env:
docker compose --env-file ../../.env up --build
```

Compose bind-mounts the secret file as-is, so it must be readable by the
container user (UID 10001). `/debug status` shows whether the gateway answers an
authenticated `GET /v1/models` and whether both aliases are listed.

Older `.env` files may still contain `OLLAMA_HOST`, `OLLAMA_MODEL`,
`OLLAMA_THINK`, `OLLAMA_TIMEOUT` or `OLLAMA_NUM_CTX`. They are ignored with one
startup warning naming each replacement (`KANATA_BASE_URL`, `EXTRACT_MODEL`,
`EXTRACT_REASONING`, `KANATA_TIMEOUT`, `MODEL_CONTEXT_TOKENS`); delete them.

**Upgrading from the old layout:** if your `.env` explicitly says
`BOSSES_PATH=config/bosses.yaml`, change it to `BOSSES_PATH=boss/bosses.yaml`,
set `BOSS_KNOWLEDGE_PATH=boss/knowledge`, and restart. `docker compose up --build`
also picks up the new `boss/` image copy.

## Settings reference

Settings come from `.env` or the process environment (case-insensitive).
**Seed** means the value only seeds SQLite on first run; afterwards the portal,
`bossctl` or slash commands own it. **Env-only** security settings are never
runtime-editable.

**Discord**

| Name | Default | Meaning |
| --- | --- | --- |
| `DISCORD_TOKEN` | required | Bot token. Secret. |
| `GUILD_ID` | required | The server the bot serves. |
| `BOSSING_ROLE_ID` | required | Role marking bossers; gates slash commands. |
| `CHAT_CHANNEL_IDS` | empty | Watched party channels, comma separated. One of these two lists is required. |
| `CHAT_CATEGORY_IDS` | empty | Watched categories; threads use their parent, new channels are picked up live. |
| `POST_CHANNEL_ID` | unset | Weekly digest channel and fallback when a run's home channel is gone. |
| `ADMIN_ROLE_ID` | unset | **Env-only.** Staff role; server admins and the owner are always staff. |
| `DEBUG_USER_IDS` | empty | **Env-only.** Extra user ids allowed `/debug`, comma separated. |

**Scheduling**

| Name | Default | Meaning |
| --- | --- | --- |
| `TZ` | `Asia/Kuala_Lumpur` | IANA zone all typed times are read in. |
| `BOSS_WEEK_RESET_WEEKDAY` | `thu` | Boss-week reset day. |
| `BOSS_WEEK_RESET_TIME` | `00:00` | Boss-week reset time. |
| `DAY_OF_PING_TIME` | `01:00` | **Seed.** Morning ping for the day's runs (`/pingtime`). |
| `COUNTDOWN_MINUTES` | `60` | **Seed.** Countdown pings, minutes before a run. |
| `TICK_SECONDS` | `30` | Reminder-loop interval, 5–600. |

**Storage and catalogs**

| Name | Default | Meaning |
| --- | --- | --- |
| `DB_PATH` | `data/bot.sqlite` | SQLite file; Compose overrides it to `/app/data/bot.sqlite`. |
| `DB_OWNER_LOCK_DIR` | unset | Shared absolute `0700` directory for all persistent writers; file-backed writes fail closed without it. |
| `BOSSES_PATH` | `boss/bosses.yaml` | Boss catalog. Keep coupled with the knowledge path. |
| `BOSS_KNOWLEDGE_PATH` | `boss/knowledge` | Chat knowledge; needs one lowercase YAML per catalog boss plus `_meta.yaml` when the chatbot is on. |

**Model gateway and extraction**

| Name | Default | Meaning |
| --- | --- | --- |
| `KANATA_BASE_URL` | `https://sumi.kanata.hoshinoht.dev` | **Env-only.** Gateway URL; HTTPS only. |
| `KANATA_API_KEY_FILE` | empty | File holding the bearer key. Compose fixes it to `/run/secrets/kanata_api_key` from `KANATA_API_KEY_HOST_FILE`. |
| `KANATA_TIMEOUT` | `120` | Extraction timeout in seconds, including cold model load. |
| `EXTRACT_MODEL` | empty | **Seed.** Extraction model alias; startup refuses while extraction is on and neither this nor the stored selection is set. |
| `EXTRACT_REASONING` | `off` | **Seed.** `low`/`medium`/`high`/`off`; sent only to aliases with `reasoning_control`. Keep `off` unless the `live_model` suite says otherwise. |
| `MODEL_CONTEXT_TOKENS` | `8192` | Client prompt budget only; must match the host model's `num_ctx` / `OLLAMA_CONTEXT_LENGTH`. |
| `EXTRACT_ENABLED` | `true` | **Seed.** Extraction switch; messages are still stored when off (`/bot pause` at runtime). |
| `EXTRACT_DEBOUNCE_SECONDS` | `90` | Silence that ends a burst and triggers one model call. |
| `EXTRACT_CONTEXT_MESSAGES` | `25` | Earlier messages (last 48 h) shown as context, 0–100. |
| `EXTRACT_MIN_CONFIDENCE` | `0.6` | Amendments below this are logged, never posted. |
| `BACKFILL_ON_START` | `true` | Backfill the current boss week at startup without the model. |

**Portal, API and CLI**

| Name | Default | Meaning |
| --- | --- | --- |
| `ADMIN_TOKEN` | empty | **Env-only.** Shared API/portal/bossctl token; empty refuses all but `/healthz`. Rotation signs browsers out. |
| `ALLOWED_TAILSCALE_LOGINS` | empty | **Env-only.** Tailnet logins allowed via `tailscale serve`; used only with trusted headers. |
| `TRUST_TAILSCALE_HEADERS` | `false` | **Env-only.** Trust identity headers only behind local `tailscale serve`. Never behind Caddy or Funnel. |
| `PORTAL_ACTOR_ID` | server owner | Discord user credited for portal changes. |
| `API_HOST` | `127.0.0.1` | API bind; Compose overrides it to `0.0.0.0` inside the container. |
| `API_PORT` | `8080` | API port. |
| `BOSSCTL_URL` | `http://127.0.0.1:8080` | Where `bossctl` finds the API (read by the CLI, not `Settings`). |

**Chatbot**

The chatbot's channel/category lists are independent of the extractor's.
Pointing `CHAT_PILOT_CATEGORY_IDS` at the bossing category makes the bot answer
in party channels; prefer a dedicated chat channel.

| Name | Default | Meaning |
| --- | --- | --- |
| `CHAT_PILOT_ROLE_ID` | unset | Role required to talk to the bot; unset means nobody. |
| `CHAT_PILOT_CHANNEL_IDS` | empty | Chat channels, comma separated. |
| `CHAT_PILOT_CATEGORY_IDS` | empty | Chat categories; both lists empty keeps the chatbot off. |
| `CHAT_PILOT_MODEL` | empty | **Seed.** Chat model alias; startup refuses while the chatbot is configured and neither this nor the stored selection is set. |
| `CHAT_ROLE_PLUGINS` | empty | **Seed.** `ROLE_ID=profile,...`; managed in Config → Chatbot afterwards. |
| `CHAT_PILOT_RATE_COUNT` | `4` | **Seed.** Per-member requests per window; admins exempt. |
| `CHAT_PILOT_RATE_WINDOW_S` | `300` | **Seed.** Per-member window, seconds. |
| `CHAT_PILOT_GLOBAL_RATE_COUNT` | `12` | **Seed.** Guild-wide requests per window. |
| `CHAT_PILOT_GLOBAL_RATE_WINDOW_S` | `900` | **Seed.** Guild-wide window, seconds. |
| `CHAT_PILOT_LOCK_WAIT_S` | `2` | Wait for the shared model lock before shedding a request. |
| `CHAT_PILOT_HISTORY_TTL_S` | `2700` | Conversation and last-card retention, seconds. |
| `CHAT_PILOT_TIMEOUT` | `60` | Seconds for one whole answer, including tool calls. |
| `CHAT_PILOT_TEMPERATURE` | `0.7` | Sampling temperature, 0–2; sent only to aliases with `sampling_controls`. |
| `CHAT_PILOT_THINK` | empty | **Seed** (when set). Chat reasoning effort; empty falls back to the extraction reasoning level. |
| `PERSONA_PATH` | `config/personas/identities/persona.md` | **Seed, deprecated.** Basename resolves to a persona id; use `config/personas/personas.yaml`. |
| `STAGING_PATH` | `config/personas/behaviours/staging.yaml` | Legacy-only staging file. |
| `STAGING_PROFILES_DIR` | `config/personas/behaviours/staging` | Per-profile staging overrides. |

**Logging**

| Name | Default | Meaning |
| --- | --- | --- |
| `LOG_LEVEL` | `INFO` | Python log level. |

## Run

```sh
docker compose up --build
```

The container publishes `127.0.0.1:8080` for the portal (see
[the portal guide](portal.md)) and bind-mounts `./data` for the SQLite
database, so the schedule survives rebuilds. `restart: unless-stopped` plus
Docker Desktop's "start at login" is all the supervision it needs.

Health: `docker compose ps` shows healthy once the bot has ticked; the check is
`python -m bot.health`, which passes when the database opens, the bot wrote a
heartbeat in the last 3 minutes, **and** the in-process API answers `/healthz`.

To run it without Docker:

```sh
uv sync
uv run python -m bot
```

## Troubleshooting

| Symptom                                                                      | Cause                                                                                                                                                                     |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Bot exits with "Message Content and/or Server Members intent is not enabled" | the intents step above — turn both on in the portal.                                                                                                                                    |
| "This channel isn't watched" on `/fixed add`                                 | The channel is not in `CHAT_CHANNEL_IDS` and its category is not in `CHAT_CATEGORY_IDS`.                                                                                  |
| Commands don't appear                                                        | They are guild-scoped to `GUILD_ID` and sync on startup, so this is usually a wrong `GUILD_ID`, or the bot was invited without the `applications.commands` scope.         |
| "You need the bossing role"                                                  | `BOSSING_ROLE_ID` is wrong, or the roster hasn't synced — check the startup log line `roster synced: N members`.                                                          |
| No reminders                                                                 | The bot must be able to post in the run's home channel (where `/fixed add` was used); the log says `channel ... unavailable` if not. Set `POST_CHANNEL_ID` as a fallback. |
| Container unhealthy                                                          | `docker compose logs bot`. The healthcheck fails if the tick loop stopped writing its heartbeat.                                                                          |
