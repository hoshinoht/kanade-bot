# Setup

Everything to get the bot from zero to running: the Discord developer
portal, `.env`, and the container.

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

Every variable is documented inline in `.env.example`. The ones you must set are
`DISCORD_TOKEN`, `GUILD_ID`, `BOSSING_ROLE_ID`, and at least one of
`CHAT_CHANNEL_IDS` / `CHAT_CATEGORY_IDS`; the rest
have sensible defaults (`TZ=Asia/Kuala_Lumpur`, reset `Thu 00:00`, morning ping
`09:00`, countdowns `60,15`).

Boss names, levels, aliases and the difficulties each boss actually has live in
[`boss/bosses.yaml`](../boss/bosses.yaml). The catalog is bind-mounted read-only
and loaded at startup, so edit it and restart — no rebuild. It ships with the eleven bosses parties
currently run: Lotus, Chosen Seren, Gatekeeper Kalos, The First Adversary,
Carling, Radiant Malefic Star, Bellona, Limbo, Baldrix, Jupiter and Black Mage.

**Boss portraits are optional.** Drop `Star.png`, `Kalos.png` and friends into
[`boss/portraits/`](../boss/portraits/README.md) and the portal shows them next
to each boss, and the bot attaches one as the thumbnail on that run's pings. A
boss with no file gets a coloured monogram instead, so nothing shifts either way.
Portraits and entry artwork are served from disk, so their changes appear on the
next page load without a restart.

When the chatbot is configured, `BOSS_KNOWLEDGE_PATH` (default
`boss/knowledge`) must contain `_meta.yaml` and one lowercase YAML document for
every catalog boss. The catalog and knowledge are validated together at startup;
missing, extra, or malformed documents stop the chat-enabled bot until fixed.

**Upgrading from the old layout:** if your `.env` explicitly says
`BOSSES_PATH=config/bosses.yaml`, change it to `BOSSES_PATH=boss/bosses.yaml`,
set `BOSS_KNOWLEDGE_PATH=boss/knowledge`, and restart. `docker compose up --build`
also picks up the new `boss/` image copy.

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
