# Boss Scheduler Bot

A Discord bot that keeps a MapleStory guild's weekly boss schedule and posts
tagged reminders.

**This is phase 1 (the skeleton).** It is fully useful with zero LLM: you set
baseline timings with `/fixed`, the bot materialises them into concrete runs each
boss week, and it pings exactly the people on each run — a grouped morning
message plus countdowns — with ✅/❌ reactions as the attendance record. The chat
extractor (phase 2) and the web portal / `bossctl` (phase 3) are not built yet.

---

## What it does today

|                      |                                                                                                                                                                                   |
| -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Membership**       | Anyone with the `BOSSING_ROLE` is a known bosser. The roster syncs from the role on startup and on member updates — no `/roster` upkeep.                                          |
| **Baseline**         | `/fixed add` records a weekly timing (`HStar, HFA — Mon 21:30 — @a @b @c`). Many parties coexist; the bot has no concept of "the" party.                                          |
| **Weekly runs**      | At each boss-week reset (default Thu 00:00) the baseline is materialised into concrete runs for the current and next week.                                                        |
| **Watched channels** | `CHAT_CHANNEL_IDS` and/or `CHAT_CATEGORY_IDS` decide where the bot listens. A category watches every channel under it, including ones added later; threads count as their parent. |
| **Home channels**    | Each fixed run's **home channel** is the (watched) channel `/fixed add` was invoked in. All of that run's output lands there, so one channel per party stays clean.               |
| **Reminders**        | One grouped **day-of** message per home channel each morning (one line per run, each tagging only its own participants), plus **countdown** pings at T-1h and T-15m.              |
| **RSVPs**            | The bot puts ✅/❌ on every reminder. ✅ from everyone → the run is `confirmed`; any ❌ → `at_risk` and the bot replies tagging the rest to reschedule.                               |
| **Changes**          | `/amend`, `/cancel`, `/otot`, `/rsvp`, `/nick`, `/pingtime`.                                                                                                                      |

Reminders are rows in SQLite, not in-memory jobs, so restarts and rebuilds never
lose or replay a ping.

---

## 1. Discord developer portal setup

1. Go to <https://discord.com/developers/applications> → **New Application**.
   Name it whatever you like (the design suggests `YuukiSakuna`).
2. **Bot** tab → **Reset Token** → copy it. This is `DISCORD_TOKEN`; treat it
   like a password. Never commit it.
3. Still on the **Bot** tab, under *Privileged Gateway Intents*, enable **both**:
   - **Server Members Intent** — the roster is derived from the bossing role.
   - **Message Content Intent** — not used until phase 2, but enabling it now
     avoids a second trip. (The bot declares both, so if either is off it exits
     with a clear error instead of hanging.)
4. **OAuth2 → URL Generator**:
   - Scopes: `bot` and `applications.commands`.
   - Bot permissions — exactly these six (permissions integer `274877992000`):
     **View Channels**, **Send Messages**, **Send Messages in Threads**,
     **Embed Links**, **Read Message History**, **Add Reactions**.
     *Mention Everyone is not needed*: pinging a run's participants is an
     ordinary user mention, and the bot never pings `@everyone` or a role — every
     message goes out with an allow-list of exactly the users on that run.
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
     (and optionally an admin role → `ADMIN_ROLE_ID`)
6. Make sure the bot's role can see and post in every party channel you will run
`/fixed add` in.

## 2. Configure

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
[`config/bosses.yaml`](config/bosses.yaml). It is bind-mounted read-only, so you
can edit it and restart — no rebuild. It ships with the nine bosses parties
currently run: Chosen Seren, Gatekeeper Kalos, The First Adversary, Carling,
Radiant Malefic Star, Limbo, Baldrix, Jupiter and Black Mage.

## 3. Run

```sh
docker compose up --build
```

The container publishes `127.0.0.1:8080` for the phase-3 portal (nothing listens
on it yet) and bind-mounts `./data` for the SQLite database, so the schedule
survives rebuilds. `restart: unless-stopped` plus Docker Desktop's "start at
login" is all the supervision it needs.

Health: `docker compose ps` shows healthy once the bot has ticked; the check is
`python -m bot.health`, which passes when the database opens and the bot wrote a
heartbeat in the last 3 minutes.

To run it without Docker:

```sh
uv sync
uv run python -m bot
```

## 4. Using it

```
/fixed add bosses:hstar, hfa day:Mon time:21:30 member1:@Alvin member2:@Priya
                                # run this in your party's channel - that becomes
                                # the run's home channel, where its pings go.
                                # member1..member6 open Discord's member picker;
                                # you are added automatically.
/fixed list
/fixed edit id:a1b2c3d4 time:22:00
/fixed edit id:a1b2 member1:@Alvin member2:@kanon   # replaces the participant list
/fixed edit id:1 channel:#other-party   # move where its pings go
/fixed remove id:a1b2c3d4

/schedule                       # in a party channel: that channel's runs
                                # elsewhere: your runs. Public, never pings.
/schedule scope:mine|all|channel week:this|next

/amend run_id:a1b2 to:wed 21:30  # understands "tomorrow 9:45pm", "in 2 hours"
/cancel run_id:a1b2c3d4
/otot run_id:a1b2c3d4           # own time: stays in the morning ping, no countdowns
/rsvp run_id:a1b2c3d4 answer:no

/nick user:@harbour4417 alias:MY  # chat nickname, used by the phase-2 extractor
/pingtime time:08:30            # move the morning ping, reschedules pending ones
/bot pause | /bot resume        # stop/resume chat watching (phase 2)
```


**About ids.** Runs and fixed runs are identified by a UUID, shown as its first
eight characters — `#a1b2c3d4`. You almost never type one: every command that
takes an id has a **dropdown** listing your runs as
`HStar + HFA · Mon 21:30 · #hstar-alvin-kanon · a1b2c3d4`. If you do type one,
any unique prefix of four characters or more works, case-insensitively, with or
without the `#` — so you can paste `#a1b2c3d4` straight out of `/schedule`. An
ambiguous prefix comes back with the candidates listed.

Boss tokens always need a difficulty prefix: `e`asy, `n`ormal, `h`ard, `c`haos,
e`x`treme. `hstar`, `HFA`, `xkalos`, `ncarling`, `hbaldguy` all work. Two things
are rejected rather than guessed, both with the valid forms listed:

- a bare name — `kalos` → *missing a difficulty prefix (e/n/h/c/x) — try EKalos,
  NKalos, CKalos, XKalos*
- a difficulty that boss does not have — `hkalos` → *Gatekeeper Kalos has no Hard
  difficulty — did you mean EKalos, NKalos, CKalos, XKalos?* (so `cseren`, which
  looks like "Chosen Seren", is caught too)

`participants:` still accepts typed names as a fallback (`MY, alvin` — matched
against display names, server nicknames and `/nick` aliases); anything it can't
match, or that could mean two people, comes back as an error naming the problem.
The pickers are more reliable.

Only members with the bossing role can use the commands. Only a run's
participants, its owner, or `ADMIN_ROLE_ID` members can change it. Bot accounts
are never rostered and cannot be participants, even if they hold the role.


### Testing it

`/debug` posts the *real* reminder messages on demand so you can check the whole
flow without waiting for 09:00. Restricted to the server owner, `ADMIN_ROLE_ID`
members, and ids in `DEBUG_USER_IDS`.

```
/debug ping run_id:a1b2 kind:day_of   # posts "🧪 TEST — ..." in the run's home
                                      # channel with real ✅/❌ reactions
/debug reminders [run_id:a1b2]        # reminder rows: fire_at, sent, message ids
/debug tick                           # run the reminder tick right now
/debug materialise                    # force current+next week materialisation
/debug upcoming hours:24              # dry run: what would fire, nothing sent
/debug status                         # uptime, heartbeat, week, Ollama reachability
/debug clear_test                     # delete this channel's 🧪 TEST messages (24h)
```

A `/debug ping` **never touches the run's reminder rows** — the scheduled ping
still goes out on time. Test messages are tracked in a separate `debug_messages`
table, so reacting ✅/❌ to one drives the real RSVP flow and you can watch a run
go `planned → confirmed` or `→ at_risk` end to end.

## 5. Exporting chat

Phase 2 tunes the extractor against real conversations, which have to be on disk
first. `python -m bot.export` logs in with the **bot token** (never a user token —
self-botting breaks Discord's ToS), pages channel history, and writes one JSONL
file per channel into git-ignored `data/exports/`:

```sh
docker compose run --rm bot python -m bot.export --category <category-id> --since 2026-06-01
```

- `--channel <id>` / `--category <id>` — repeatable. With neither, every watched
  channel is exported. **Only watched channels can be exported**; anything else
  is refused.
- `--since` / `--until` — `YYYY-MM-DD` (midnight guild time) or an ISO timestamp.
  `--since` defaults to the start of the current boss week.
- `--out <path>` — a single file; only valid when one channel is selected.
  Otherwise files are named `data/exports/<channel-name>-<since>.jsonl`.

Threads are included and filed under their parent channel. Each line carries
`id, channel_id, channel_name, thread_id, author_id, author_name, author_bot,
created_at, content, mentions[], reply_to, reactions{emoji: [user_ids]},
attachments[]` (plus `edited_at` when edited). **Attachments are recorded as
`[image] name.png` / `[file] name.txt` markers only — the bot never downloads
them.** Exported messages are also upserted into the `messages` table.

The export needs the same privileged intents as the bot, and it does not post,
sync commands, or run the reminder loop.

## 6. Development

```sh
uv sync                 # includes dev deps
uv run pytest           # unit tests, no Discord connection needed
uv run ruff check .
uv run ruff format .
```

Tests cover the pure layers — boss-week arithmetic, token parsing,
materialisation idempotency, reminder generation and pruning, and the
reaction → RSVP → status transitions. Discord I/O is kept thin on purpose so it
needs no mocking.

### Layout

```
bot/
  __main__.py    entrypoint, login backoff, signal handling
  config.py      env settings (pydantic-settings)
  db.py          SQLite schema + repository
  weeks.py       boss-week arithmetic (pure)
  bosses.py      alias table + token parsing (pure)
  materialise.py fixed runs -> runs -> reminder rows (pure-ish)
  rsvp.py        reaction -> RSVP -> run status (pure)
  formatting.py  message and embed text (pure)
  watch.py       which channels the bot listens to (pure)
  client.py      discord.py client, tick loop, reactions
  commands.py    slash commands
  export.py      `python -m bot.export` -- channel history -> JSONL
  health.py      container healthcheck
config/bosses.yaml
tests/
```

## Troubleshooting

| Symptom                                                                      | Cause                                                                                                                                                                     |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Bot exits with "Message Content and/or Server Members intent is not enabled" | Step 1.3 — turn both on in the portal.                                                                                                                                    |
| "This channel isn't watched" on `/fixed add`                                 | The channel is not in `CHAT_CHANNEL_IDS` and its category is not in `CHAT_CATEGORY_IDS`.                                                                                  |
| Commands don't appear                                                        | They are guild-scoped to `GUILD_ID` and sync on startup, so this is usually a wrong `GUILD_ID`, or the bot was invited without the `applications.commands` scope.         |
| "You need the bossing role"                                                  | `BOSSING_ROLE_ID` is wrong, or the roster hasn't synced — check the startup log line `roster synced: N members`.                                                          |
| No reminders                                                                 | The bot must be able to post in the run's home channel (where `/fixed add` was used); the log says `channel ... unavailable` if not. Set `POST_CHANNEL_ID` as a fallback. |
| Container unhealthy                                                          | `docker compose logs bot`. The healthcheck fails if the tick loop stopped writing its heartbeat.                                                                          |
