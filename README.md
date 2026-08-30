# Boss Scheduler Bot

A Discord bot that keeps a MapleStory guild's weekly boss schedule and posts
tagged reminders.

**Phases 1, 2 and 3 are built.** You set baseline timings with `/fixed`, the bot
materialises them into concrete runs each boss week, and it pings exactly the
people on each run — a grouped morning message plus countdowns — with ✅/❌
reactions as the attendance record. On top of that it **reads the party's chat**
with a local LLM and posts a card proposing the change it found; nothing reaches
the schedule until someone reacts ✅. And it serves a **web portal and a
`bossctl` CLI** on `127.0.0.1:8080` — the week at a glance, the fixed-timing
editor, the inbox of what the extractor proposed, and the full extraction log —
reachable from your phone over Tailscale.

---

## What it does today

|                      |                                                                                                                                                                                   |
| -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Membership**       | Anyone with the `BOSSING_ROLE` is a known bosser. The roster syncs from the role on startup and on member updates — no `/roster` upkeep.                                          |
| **Baseline**         | `/fixed add` records a weekly timing (`HStar, HFA — Mon 21:30 — @a @b @c`). Many parties coexist; the bot has no concept of "the" party.                                          |
| **Weekly runs**      | At each boss-week reset (default Thu 00:00) the baseline is materialised into concrete runs for the current and next week. A run whose night has passed goes `done` on the tick and drops out of `/schedule` and the dropdowns. |
| **Watched channels** | `CHAT_CHANNEL_IDS` and/or `CHAT_CATEGORY_IDS` decide where the bot listens. A category watches every channel under it, including ones added later; threads count as their parent. |
| **Home channels**    | Each fixed run's **home channel** is the (watched) channel `/fixed add` was invoked in. All of that run's output lands there, so one channel per party stays clean.               |
| **Reminders**        | One grouped **day-of** message per home channel each morning (one line per run, each tagging only its own participants), plus **countdown** pings at T-1h and T-15m.              |
| **RSVPs**            | The bot puts ✅/❌ on every reminder. ✅ from everyone → the run is `confirmed`; any ❌ → `at_risk` and the bot replies tagging the rest to reschedule.                               |
| **Changes**          | `/amend`, `/status` (with `/otot`, `/cancel`, `/restore`, `/done` as shortcuts), `/rsvp`, `/nick`, `/pingtime`. Every change made outside Discord is announced in the run's home channel, marked *(via portal)*. |
| **Chat extraction**  | A local `gpt-oss:20b` reads the party channels and posts a 📋/💡/📌 card for each change it finds. ✅ from a participant applies it; ❌ rejects it; unanswered cards expire after 24 h. |
| **Portal & CLI**     | A local web portal (week view, fixed editor, proposal inbox, extraction log, config) and `bossctl`, both over one HTTP API inside the bot process. Loopback only; tailnet via `tailscale serve`.  |

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
     message goes out with an allow-list of exactly the users who need to act
     (DESIGN.md §3, "Mention policy").
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

**Boss portraits are optional.** Drop `Star.png`, `Kalos.png` and friends into
[`config/portraits/`](config/portraits/README.md) and the portal shows them next
to each boss, and the bot attaches one as the thumbnail on that run's pings. A
boss with no file gets a coloured monogram instead, so nothing shifts either way.

## 3. Run

```sh
docker compose up --build
```

The container publishes `127.0.0.1:8080` for the portal (see
[Portal & CLI](#6-portal--cli)) and bind-mounts `./data` for the SQLite
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
                                # elsewhere: your runs (on them, or you own the
                                # timing). Public, never pings. Runs that have
                                # already happened are hidden.
/schedule scope:mine|all|channel week:this|next show_past:True

/amend run_id:a1b2 to:wed 21:30  # understands "tomorrow 9:45pm", "in 2 hours"
/status run_id:a1b2 state:...   # planned · confirmed · own time · done · cancelled
/cancel run_id:a1b2c3d4         # shortcuts for the same thing
/otot run_id:a1b2c3d4           # own time: stays in the morning ping, no countdowns
/restore run_id:a1b2c3d4        # put a cancelled/own-time/finished run back on
/done run_id:a1b2c3d4           # cleared early
/swap run_id:a1b2 out:@Priya in:@kanon  # this week only; the timing is unchanged
/rsvp run_id:a1b2c3d4 answer:no

/nick user:@harbour4417 alias:MY  # chat nickname, used by the extractor
/pingtime time:08:30            # move the morning ping, reschedules pending ones
/bot pause | /bot resume        # stop/resume chat watching
/rescan hours:24                # re-read this channel's recent chat and propose
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


### How the extractor decides

The bot watches the party channels and turns the conversation into schedule
changes. **The model is an extractor, not the scheduler** — five stages, only one
of which is the LLM, and a human ends every one of them.

**1. Gate (Python, no model).** Every message is scored for a boss alias, a clock
time, a weekday or relative day, a scheduling verb, an `@here`, a mention of a
roster member, or an agreement ("Can", "Ok", "kenot"). Banter is dropped here, so
a 13 GB model is never woken for "botter again sigh". Boss tokens tolerate the guild's
spelling — `hlimb`, `nbald`, `bladrix`, `hkarling`, `exkalos`, `hstarr` — but
`start` never becomes `Star` and `cc9`/`ch7` is a map channel, not a time. Bare
answers ("Can") only trigger a call if the channel was scheduling in the last 6 h.

**2. Burst.** Gated messages buffer per channel and go to the model together
after `EXTRACT_DEBOUNCE_SECONDS` of silence — or immediately when a message has a
mention/@here **and** a boss or a time. One model call per burst, never per
message. Only messages from people holding the bossing role count.

**3. Model.** One call to Ollama, serialised guild-wide (only ever one at a
time), constrained by a JSON schema, at `temperature 0` with `keep_alive=-1`.
The prompt carries the boss table, **that channel's** runs and fixed timings, the
roster members who actually appear, and the messages with `[msg_id]` prefixes so
it can cite evidence. It stays around 2k tokens whatever size the guild grows to.
The model emits `kind`, `bosses`, the **literal words** it saw for the day and
time (`"weds"`, `"1030~11+pm"`), who it is about, `is_question`, a confidence and
its evidence message ids. It never computes a date.

**4. Deterministic assembly (Python again).** The per-message pieces are merged
into one candidate per affected run (latest stated value wins, so "9pm" then
"amend to 9:45pm" is one run at 21:45). `day_ref`/`time_ref` are resolved against
the **last evidence message's** local time: `tonight`, `tmr`, `weds`, `next mon`,
`9:30pm`, `930`, `1030~11+pm`, `at 11`, `11pm onward`. A bare hour of 1–11 means
pm, because every run here is an evening run; anything unparseable stays `TBD`
rather than being guessed. Each candidate is then matched to a run by
**bosses ∩ participants**, scoped to the channel it was said in.

**5. Card, then ✅.** Anything below `EXTRACT_MIN_CONFIDENCE` is logged and never
posted. Everything else becomes one card per burst in that channel:

- 📋 **Proposed change** — the thread reached a decision.
- 💡 **Suggested amendment** — it ended on a question, or a field is still
  unknown. Unknown fields read **TBD** and the card names who has not answered.
  Nobody's availability is ever inferred from silence or past attendance.
- 📌 **New fixed timing** — a recurring time was stated ("lock in Tue 1030pm as
  default"); ✅ creates the fixed run exactly as `/fixed add` would.

✅ from a participant of the target run (or an admin, or the server owner)
applies it and edits the card to "✅ applied by …"; ❌ rejects it. Anyone else's
reaction is ignored. Cards nobody answers expire after 24 hours. RSVPs are the
one exception: "Can" / "kenot" is recorded straight away through the same
path as a ✅ reaction, because it records an opinion rather than changing a
schedule.

Every call — prompt, raw JSON, latency, the amendments it produced — is written
to the `extractions` table. That is the prompt-tuning tool, and phase 3's portal
will render it.

```
/rescan                                  # queue a re-read of this channel's boss week
/rescan window:2weeks                    # week (default) · 2weeks · 48h · 24h
/rescan scope:all channels               # every watched channel, one at a time
/rescan cancel:True                      # stop the one that is running
/debug extract                           # read it now, ephemeral, raw JSON, no card
```

A rescan is **queued, never awaited**: re-reading a boss week is minutes of model
time, so the bot answers commands, ticks reminders and handles reactions
throughout. One worker drains the queue a channel at a time (the model is
single-threaded anyway), and asking twice for the same channels attaches to the
job already running rather than reading everything twice. Cancelling lands
between conversations — a call already in flight is left to finish.

A rescan does three things in order. It **backfills from Discord first** —
paging `channel.history()` for the window, threads included — so it still works
on a database that was just reset, which is exactly when you reach for it. Then
it splits the window back into the conversations it came from (a 15-minute gap
ends one) and sends **each conversation as its own prompt**, oldest first: a
whole boss week in one prompt would be enormous and impossible for the model to
attribute. Finally, anything it finds that is already over — a time more than
three hours in the past, or a run that is finished or cancelled — is dropped
rather than posted, so re-reading old chat never puts up a card about last
Tuesday. The reply says what it read, not just what it found.

If the current boss week holds no scheduling chat at all, it checks the week
before it once (and says so). Never further back than that.

**What it reads, and how it is cut up.** History is grouped into the
conversations it actually was: the unit is the local calendar day, so a planning
thread that ran from lunchtime to the evening with long pauses stays one prompt.
Only a day too big for one prompt is split, at its longest pause. Each
conversation is one model call, and the channel gets **one card at the end** —
not one per burst, which used to leave a stack of cards superseding each other.

**Automated re-reads are capped at 48 hours** and never widen to the previous
week; only a person asking can choose `week` or `2weeks`.

The same backfill runs on startup for every watched channel — no model call,
just history into the database. Turn it off with `BACKFILL_ON_START=false`.

Turn it off with `EXTRACT_ENABLED=false` (messages are still stored) or
`/bot pause` at runtime. If Ollama is unreachable or slow the extraction is
logged as failed and nothing else happens — the reminders keep running.

#### Tuning it

```sh
# replay a real exported channel through the extractor; no Discord, no writes
uv run python -m bot.extract --file data/exports/<name>.jsonl \
    --since 2026-06-01 --host http://127.0.0.1:11434

# the regression suite: 14 fixtures of real (anonymised) chat vs the real model
uv run pytest -m ollama -v
```

`tests/fixtures/extract/*.json` are the guild's own messages with names reduced
to single letters and ids replaced by fake snowflakes; each carries the channel's
runs at the time and the amendments a correct extraction produces. Scoring is
strict — everything expected must be found and nothing extra invented.

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
/debug extract hours:6                # run the extractor here and show its raw JSON
                                      # (ephemeral, and never posts a card)
/debug clear_test                     # delete this channel's 🧪 TEST messages (24h)
```

A `/debug ping` **never touches the run's reminder rows** — the scheduled ping
still goes out on time. Test messages are tracked in a separate `debug_messages`
table, so reacting ✅/❌ to one drives the real RSVP flow and you can watch a run
go `planned → confirmed` or `→ at_risk` end to end.

## 5. Exporting chat

The extractor is tuned against real conversations, which have to be on disk
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

## 6. Portal & CLI

One HTTP API, two front ends. It runs **inside the bot process** — FastAPI on the
same asyncio loop as discord.py — so the portal reads live state, writes to the
same SQLite file the bot has open, and can post to Discord, with no second
process to supervise.

### Set the token

```sh
openssl rand -hex 32          # paste into ADMIN_TOKEN= in .env
docker compose up -d --build
open http://127.0.0.1:8080    # sign in with that token
```

Until `ADMIN_TOKEN` is set the API still starts and `/healthz` still answers, but
every other request comes back `503 set ADMIN_TOKEN` — a half-configured
deployment fails loudly rather than quietly serving your schedule. Rotating the
token signs every browser session out.

### The pages

| Page             | What it is for                                                                                                                                     |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Week**         | The default. A seven-column rail for the boss week — starting on the reset day, not Monday — with a pip per run, then the runs grouped by day. Filter by channel, member or boss; move, preview-ping, and a status control (planned · confirmed · own time · done · cancelled) on each row. Past and cancelled runs are hidden until you ask. |
| **Fixed**        | The baseline timings, with a create/edit form. Bosses are picked from a **boss grid** — the in-game list, one row per boss with its real difficulties as pills — or typed as tokens. The party comes from the synced roster. |
| **Bosses**       | The same grid, read-only, with the difficulties the guild actually has timings for ticked. A quick "what do we run".                                |
| **Inbox**        | What the extractor proposed and nobody has answered: the change, its confidence, and the exact chat lines it cited. Approve, edit-then-approve, or reject — the same code path a ✅ on the Discord card runs, and the card is edited to say it was applied via the portal. |
| **Extractions**  | Every model call: the prompt as sent, the raw JSON back, the latency, and the changes it produced. This is the prompt-tuning tool.                  |
| **Members**      | The roster as synced from the bossing role, plus the chat aliases the extractor matches names against.                                              |
| **Reminders**    | Queued and sent reminder rows, with a link straight to each posted message in Discord.                                                              |
| **Config**       | Morning ping time, countdown offsets, pause chat watching, turn the extractor off, post the weekly digest now, **re-read the party channels**, and a **channel access** table showing what the bot may actually do in each one. The `.env`-only values are listed read-only underneath. |

Every time on every page is in the guild timezone, which is named in the header.
The pages are server-rendered; [htmx](https://htmx.org) (pinned, from cdnjs, with
an integrity hash) only upgrades the actions to in-place swaps. Every control is
a real form, so with the CDN blocked or JavaScript off the portal still works.

### Reaching it from your phone: `tailscale serve`

The container publishes the port on the host's loopback only
(`ports: ["127.0.0.1:8080:8080"]`), so nothing on your LAN can see it. Tailscale
puts it on your tailnet, over HTTPS, with a real certificate:

```sh
tailscale serve --bg 8080
tailscale serve status                       # check what is published
```

Then open **https://your-machine.your-tailnet.ts.net** from any device
signed into your tailnet.

To skip the token on the phone, let the portal trust the identity Tailscale
attaches to each request — add these two to `.env` and restart:

```sh
ALLOWED_TAILSCALE_LOGINS=you@example.com     # the login Tailscale shows for you
TRUST_TAILSCALE_HEADERS=true
```

`tailscale serve` sets a `Tailscale-User-Login` header on every proxied request.
That header is only a string, so the portal accepts it under three conditions at
once: the flag above is on, the login is on the allow-list, and the connection
came from this machine (loopback or the Docker bridge). Anyone who can already
open `127.0.0.1:8080` on this Mac can run code as you anyway, so that is where
the trust boundary genuinely is. With the flag off — the default — the phone just
asks for the token once and keeps a seven-day cookie.

> **Never `tailscale funnel`.** Funnel publishes the port to the public internet.
> `serve` keeps it inside your tailnet. If you want to narrow it further, a
> Tailscale ACL can restrict port 443 on this node to your own devices.

To stop publishing it:

```sh
tailscale serve --bg --https=443 off
```

### `bossctl`

```sh
uv run bossctl schedule                   # from the project directory
uv tool install .                         # or install it on your PATH
docker compose exec bot bossctl schedule  # or from inside the container
```

It reads `ADMIN_TOKEN` from the environment or from the nearest `.env`, and talks
to `BOSSCTL_URL` (default `http://127.0.0.1:8080`). Ids may be any unique prefix.
An API refusal is printed as its message and exits non-zero.

```
bossctl schedule [--week next] [--channel ID] [--user ID] [--boss hstar]
bossctl fixed list | add | edit | rm
bossctl fixed add -b "hstar, hfa" -d mon -t 21:30 -c <channel-id> -m <user-id> -m <user-id>
bossctl pending                          # what the extractor proposed
bossctl approve <id> | reject <id>
bossctl amend <run> --to "wed 21:30"
bossctl cancel <run> | otot <run>
bossctl rsvp <run> yes --user <user-id>
bossctl members | nick <user-id> MY
bossctl reminders [--run <id>]
bossctl rescan [--window week] [--channel ID ...]   # default: every watched channel
bossctl rescan-stop [JOB]                # stop it after the current channel
bossctl rescans                          # the last few
bossctl channels                         # what a rescan would cover
bossctl swap <run> --out ID --in ID      # change the party for one week only
bossctl access                           # per-channel read/post permissions
bossctl status <run> planned|confirmed|otot|done|cancelled
bossctl restore <run>
bossctl digest [--channel <id>] [--week next]
bossctl ping <run> day_of                # posts a 🧪 TEST reminder now
bossctl extractions [-n 25] | extraction <id> [--no-prompt]
bossctl export --channel <id> --since 2026-06-01 --out data/exports/party.jsonl
bossctl config get [key] | config set <key> <value>
```

`config set` takes the four runtime settings the portal edits —
`day_of_ping_time`, `countdown_minutes`, `paused`, `extract_enabled`. Everything
else is `.env` and a redeploy.

### The API itself

Everything under `/api` is JSON, documented at
<http://127.0.0.1:8080/api/docs>. Errors are `{"error": "..."}` with a real
status code.

```sh
TOKEN=$(grep '^ADMIN_TOKEN=' .env | cut -d= -f2-)
curl -s http://127.0.0.1:8080/healthz                                   # -> ok
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/api/schedule
```

`GET /healthz` is the one unauthenticated route and returns nothing but `ok`; the
compose healthcheck uses it alongside the SQLite heartbeat.

## 7. Development

```sh
uv sync                 # includes dev deps
uv run pytest           # unit tests, no Discord connection needed
uv run ruff check .
uv run ruff format .
```

Tests cover the pure layers — boss-week arithmetic, token parsing,
materialisation idempotency, reminder generation and pruning, the
reaction → RSVP → status transitions, and every stage of the extractor (gate,
resolve, merge, match, commit, cards). Discord I/O is kept thin on purpose so it
needs no mocking.

The portal and CLI are covered the same way: `tests/fake_bot.py` is a stand-in
client that records every Discord side effect, so the API routes, the auth paths,
each page's rendering (empty and populated) and the `bossctl` commands are all
exercised without a gateway. `tests/test_api_server.py` starts the real uvicorn
server on a real port to prove it shares the loop rather than stealing it.

`uv run pytest` never touches the model. The fixture suite that does is marked
`ollama` and excluded by default; run it with `uv run pytest -m ollama`, which
skips itself if Ollama is unreachable.

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
  health.py      container healthcheck (heartbeat + /healthz)
  cli.py         `bossctl` -- the Typer CLI, over the same HTTP API
  extract/       the chat extractor (phase 2)
    gate.py      keyword gate + boss-token finder (pure, no model)
    prompt.py    the prompt: boss table, channel runs, roster, messages
    schema.py    the JSON schema the model is constrained to (pydantic)
    llm.py       one guarded, serialised call to Ollama
    merge.py     per-message pieces -> one candidate per run (pure)
    resolve.py   "weds"/"1030~11+pm" -> a datetime (pure)
    match.py     amendment -> the run it is about (pure)
    pipeline.py  per-channel buffering and the whole flow
    commit.py    ✅ on a card -> the schedule change (pure repo work)
    __main__.py  `python -m bot.extract` -- offline dry run over an export
  api/           the portal + CLI API (phase 3), served on the bot's own loop
    server.py    uvicorn as a task next to discord.py; start/stop
    app.py       the FastAPI app, built around the live client
    auth.py      bearer token, tailnet identity, signed session cookie
    service.py   what the API does, in repository terms (no FastAPI here)
    routes_api.py  JSON under /api
    routes_web.py  the portal's pages and HTMX partials
    models.py    request/response shapes
    templating.py the Jinja environment
    templates/   Jinja pages and partials
    static/      portal.css
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
