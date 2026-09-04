# Changelog

Notable changes to the Boss Scheduler Bot, newest first.

## 3.3.0

**Added**

- The web portal now persists and displays each model round, including thinking
  traces, raw responses, tool arguments, complete tool results and posted-card
  outcomes.
- `get_schedule` now supports `today`, `tonight`, `tomorrow` and weekday filters,
  composed with participant and channel scopes.
- Added `Lotus` boss into list of available bosses.

**Changed**

- Reorganised the Python package into `agent`, `domain`, and `infrastructure`
  namespaces, and split the chatbot's monolithic tool module into individual
  tools behind the existing `bot.chat.tools` interface.
- Reminder reconciliation now preserves mappings for already-posted reminders,
  reopens eligible skipped reminders, retires newly past reminders and rebuilds
  countdowns only for live runs.

**Fixed**

- Run lookup now resolves weekdays to one concrete date across current and next
  boss weeks and refuses conflicting day references instead of choosing one.
- First-person schedule requests no longer mistake the bot's user or managed-role
  trigger mention for a roster participant.
- Bare date questions now default to the whole group's schedule across all
  channels instead of silently applying person and channel filters.
- Member-facing replies no longer expose scheduler function/option syntax or
  emit the `<none>` placeholder; known runs in other channels remain explicit.
- Failed Discord proposal posts can no longer be described as successfully
  posted cards merely because their database rows were created.
- Reminder reconciliation no longer deletes live morning-message mappings,
  creates no-op audit entries or leaves stale reminder states behind.
- Wide Limits tables now scroll on narrow screens instead of clipping the mobile
  portal.
- `get_schedule` tool call description tightened

## 3.2.0

**Added**

- `get_schedule` can filter by `participant="me"` or one roster name while
  retaining whole-group and channel-only schedule scopes.

**Changed**

- Chatbot factual replies now use compact Discord Markdown: bold boss names and
  actions, italic dates and times, and code-formatted ids, statuses and RSVP
  tallies.
- Visible cross-channel schedule references are now clickable Discord channel
  links.
- Multi-block chatbot replies preserve one blank line around headings and
  remarks while keeping consecutive schedule rows compact.
- Package, runtime and API version metadata now report `3.2.0`.

**Fixed**

- Named participant filters are resolved against the roster instead of silently
  returning the unfiltered schedule. Invalid, unknown and ambiguous participant
  values are refused with a clarification prompt.

## 3.1.2

**Fixed**

- `parse_when` now resolves `next <weekday> HH:MM` (e.g. `next tuesday 22:30`).
  `dateparser` returns `None` for this form when `PREFER_DATES_FROM=future` is
  set; the fix falls back to the extractor's own day/time resolver, which already
  handles `next` via `_NEXT_RE` and `_WEEKDAY_ALIASES`.
- Failed proposal cards now include the proposal summary in the pipeline error
  log, making it possible to identify which `propose_add` triggered a post
  failure without enabling `DEBUG` logging.
- Tool response text is now logged at `DEBUG` on the success path in addition to
  the existing argument trace, completing the picture for `LOG_LEVEL=DEBUG`.
- The Config section pills no longer overlap on mobile, and wide settings content
  can no longer force the portal beyond the viewport.

## 3.1.1

**Changed**

- The portal stylesheet now has an SCSS source layout: `bot/api/static/portal.scss`
  is the ordered entrypoint and `bot/api/static/portal/*.scss` holds the split
  partials, broken out from the former monolithic `portal.css`.
- `bot/api/static/portal.css` is now generated and git-ignored. Compose builds it
  into the image from the SCSS sources; local and packaged runs serve the same
  bundle from memory when the artifact is absent.
- The package and API version metadata now report `3.1.1`.

## 3.1.0

**Added**

- **Behaviour plugins** layer reusable Markdown instructions on top of the active
  chatbot persona without replacing its voice or operating rules. Discord roles
  can be assigned different plugins, and a member holding several configured
  roles receives every matching plugin in assignment order while the main chat
  role remains required for access.
- **Live plugin management in Config → Chatbot**: create, edit and delete plugin
  files, then add, update or remove role assignments without restarting the bot.
  Plugin and assignment editors are collapsible and independently paginated for
  larger lists, preserving the current page across form submissions.
- Deployments can seed initial role assignments with `CHAT_ROLE_PLUGINS`.
  Portal-managed assignments persist in SQLite, plugin instructions persist in
  the writable, git-ignored `personas/behaviour-plugins/` directory, and the
  tracked `example.md` documents how to write additional plugins.

**Changed**

- Matching behaviour-plugin instructions are reinforced on every model round,
  including direct answers and automatic clarification follow-ups. The base
  persona's factual and safety rules continue to override style instructions.
- The compose persona mount is writable so portal-created plugin files survive
  container rebuilds while the rest of the container root remains read-only.

## 3.0.2
**Added**
- Minor UI tweaks to the portal

## 3.0.1
**Fixed**
- The chat model could combine a canonical boss token with a second difficulty, 
  generating `XBM Hard` for “Extreme BM.” Validation correctly rejected `Hard` 
  as an unknown second boss, so no proposal card was created. Updated the 
  `propose_add` tool description to distinguish canonical tokens from spoken 
  difficulty-first names, prohibit combining both forms, and explicitly map 
  “Extreme BM” to `XBM`.

## 3.0.0

**Added**

- **The portal redesigned as "Kanade's Desktop"**: cream windows with chrome
  title bars on a coloured ground, five selectable colourways — marigold (the
  default), blossom, periwinkle, coral, twilight — each with an after-hours
  dark face, and a System/Light/Dark control. The choice lives in the browser
  and is stamped before first paint, so nothing flashes. The bot's own Discord
  avatar and banner are the portal's identity: the masthead, the favicon and
  the login window's hero.
- **The Week page is a day board**: seven columns starting at the boss-week
  reset, where an empty day collapses to a spine and the days with runs take
  the room. Compact cards open a run sheet — the full card in a dialog, with
  a plain fragment link when JavaScript is off — and a now-strip answers the
  page's four questions (next run, answers owed, inbox, model) before any of
  it is read.
- **No page scrolls on desktop**: the table pages became searchable, paginated
  windows that scroll inside a fixed frame — server-side search and paging on
  Audit, Extractions, Chat and Reminders, search on Members and Fixed — and
  Config became one Settings window, a table of contents on the left and one
  section at a time on the right, switched by fragment alone.
- **The bosses bring their own artwork**: `config/portraits` now has two sizes
  (the full art goes out on Discord's embed thumbnails; the portal's small
  renders keep the crisper 64px icons), and `config/artwork/entry` holds each
  boss's entry splash, laid behind the week's run cards as a veil that costs
  no height — one boss takes the side vignette, two take a corner each and
  meet in a seam. Both directories are git-ignored beside tracked READMEs;
  everything renders fine without them.
- **The portal draws its own icons** — inline Feather strokes in
  `currentColor`, so every colourway and dark face tints every icon. Discord
  keeps its emoji vocabulary untouched: over there a reaction *is* an emoji.
- **The morning ping carries the boss's entry splash**: the day-of message
  wears the lead boss's entry art as its embed image, on top of the portrait
  thumbnail it already had. Countdowns stay text-lean.
- **Limits, a chat's detail and an extraction's detail became tabbed browser
  windows** — fragment-switched tabs on the window chrome, the same no-script
  `:target` machinery as Settings, with live counts on the Limits tabs and the
  allowance form kept outside the polled region so a refresh never eats what
  you were typing.
- **Personas moved into `personas/`**, bind-mounted read-only into the
  container — tracked README and template, everything a deployment actually
  writes git-ignored — and the Config page's Chatbot panel says which file the
  voice is coming from, marked when it fell back to the template.
- **Voices swap live**: every `.md` in `personas/` (bar the README) is a
  dropdown on the Chatbot panel, the choice is runtime config seeded from
  `PERSONA_PATH`'s basename, and the next answer is in the new voice — no
  restart. Submissions are validated by membership in the real directory
  listing, audits carry filenames only, and a chosen file that goes missing
  falls back to the template and says so on the panel.
- **The README shows the portal**: six screenshots in `docs/images/`, with the
  week board in a `<picture>` tag so GitHub serves the light face to light
  readers and the Twilight one after dark.
- **A caddy front door** (`caddy/` service in compose): the portal is served
  over HTTPS at a personal domain with a real Let's Encrypt certificate,
  reachable only from the tailnet. The public A record points at the host's
  Tailscale IP — a CGNAT address that resolves everywhere and routes nowhere
  outside the tailnet — and the DNS-01 challenge means no port ever opens to
  the internet. Docker publishes 443 on that IP alone (`CADDY_BIND_IP` in
  `.env`), so the socket never exists on the LAN. Personal pieces follow the
  `.env.example` pattern: `caddy/Caddyfile.example` is the tracked template;
  the real Caddyfile and the Cloudflare token (`.env.caddy`) stay untracked.

**Changed**

- **`tailscale serve` is retired** — it only speaks its machine's ts.net name
  and rejects any other hostname at the TLS handshake, so the old ts.net URL
  is gone. The loopback `127.0.0.1:8080` mapping stays for host-local CLI and
  dev use.
- With the serve proxy gone, the `Tailscale-User-Login` header no longer
  arrives: the portal asks for `ADMIN_TOKEN` login on every device, and
  `TRUST_TAILSCALE_HEADERS` / `ALLOWED_TAILSCALE_LOGINS` are effectively
  idle until some future front door re-authenticates tailnet identity.
- The board's compact cards speak the party's own shorthand — `NCarling`,
  `HStar` — so a boss and its difficulty pill always hold one line; the full
  names stay on the sheet, the tooltips and the screen-reader labels.
- The Config page's `.env` panel names both models — **Data model** for
  extraction and rescans, **Speech model** for the chatbot's conversations —
  where one "Model" row used to stand for two different machines.
- The compose project follows the repo's name: project and container are
  `kanade-bot`, and `docker compose up -d --build` is the whole deploy.

**Fixed**

- Type reads at an honest size everywhere — a seven-step ladder with body text
  at a true 16px — and a difficulty pill can no longer be clipped at a narrow
  column or orphaned on a line away from its boss.
- `pytest -q` no longer doubles into silence: the verbosity flag is out of
  `addopts`, which keeps only the marker filter.
- The Settings sidebar's raised ground meets the window's title bar instead of
  leaving a strip of card surface between the two.

## 2.1.0

**Added**

- **Capacity controls** for the one 13 GB model the host has. A shared model
  lock (`bot/modellock.py`) serialises the chatbot, the extractor and rescans;
  staff questions queue for the model while everybody else is turned away with
  💬 after a short wait (`CHAT_PILOT_LOCK_WAIT_S`). A guild-wide answer budget
  (`CHAT_PILOT_GLOBAL_RATE_*`) sits on top of the per-person window, so handing
  out the pilot role more widely cannot monopolise the machine.
- Rate-limit refusals now say when to come back — ⏳ plus one canned sentence
  per episode with the wait in it, never a model call. Per-member windows can
  be cleared from the portal, `DELETE /api/limits/windows/{id}` and
  `bossctl limits reset`, all audited.
- **Custom rate limits**: the four capacity numbers are runtime config like
  `chat_mode` — seeded from `.env`, edited from the portal and `bossctl`
  without a restart — and members can be granted their own allowance
  (schema v8), applied live and quoted in their own refusal notice.
- **A Limits page** in the portal and `GET /api/limits`: who has the model and
  for how long, both budgets as used-of-total, open per-member windows with
  reset and override controls, everyone holding the pilot role (staff marked
  exempt), and the rescan queue — updated by server-sent events
  (`bot/events.py`, `GET /limits/events`) the moment something changes, with a
  slow visibility-aware poll and a plain Refresh link as fallbacks.
  `bossctl limits` prints the same view.
- **`/limits`** slash command: your own allowance as a progress bar, ephemeral,
  with when a spent answer comes back; staff get one line and no numbers.
  Reading it never spends anything.
- **`propose_change_fixed`**: the chatbot can change an existing weekly timing
  in place — its night, its party, or both — through the usual ✅/❌ card.
  Same row, same run ids, RSVPs kept; several matching weeklies refuse with a
  candidate list rather than guessing, and `propose_move`/`propose_add` steer
  the recurring case here instead of minting duplicates.
- **Chat memory**: remembered turns age out per turn
  (`CHAT_PILOT_HISTORY_TTL_S`, 45 min default) so a stale topic cannot claim
  "move it to 22:00" an hour later; the prompt names the last card posted in
  the channel, party included; and replying to an old bot answer re-anchors
  that exchange into context past the TTL.

**Changed**

- Creating a weekly timing whose week already holds the matching one-off run
  now **adopts** it — same id, answers and reminders kept, retimed to the
  weekly slot — instead of materialising a duplicate beside it. Through every
  door: the card, `/fixed add` and the portal.
- Tool steering closes three live failures: "this is fixed" on a new run maps
  to the weekly flag, "for me" puts the asker on the run, and asking to change
  a weekly that does not exist explains the conversion instead of offering
  other bosses' timings.
- Portal cards for all `fix` variants finally read alike — "change weekly ·
  every Wed 23:30" instead of "new weekly · TBD" — in the inbox and the chat
  interaction trace.

**Fixed**

- Chat generations in two channels could overlap each other and an extraction
  inside Ollama, timing everything out at once while the host did all the
  work; everything now queues for the same lock.
- `resolve_fixed` no longer matches a query's weekday against other bosses'
  weeklies when the boss it names has none.
- The Limits page no longer rebuilds its poll timer on every refresh or wipes
  a half-typed form; forms live outside the refreshed region.

## 2.0.0

**Added**

- **The chatbot** (`bot/chat/`): mention-gated, role-gated, rate-limited, with a
  persona loaded from the data volume. Read tools answer scheduling questions
  directly; write tools draft the same ✅/❌ proposal cards everything else
  uses — the model can never touch the schedule itself. Understands the group's
  own language: "tonight 23:00", "tmr 2300", bare clock times, "Hard Baldrix"
  and "Extreme Kalos" spelled out, weekly versus one-time runs.
- Rejection follow-up: ❌ a card the chatbot drafted for you and it asks — in
  voice, once per card, cooldown-guarded — what you would like instead.
- Chat analytics: every interaction logged with its tool trace, rounds, latency
  and token counts; a Chat page in the portal, `GET /api/chat`, `bossctl chat`.
- The chat model is its own setting (`CHAT_PILOT_MODEL`), so conversation can
  run on a larger model — Ollama's hosted ones included — while extraction
  stays local.
- **Audit trail** (schema v7): every schedule mutation records surface, actor,
  action, subject and detail. Portal actions name the tailnet login when
  `TRUST_TAILSCALE_HEADERS` vouches for it, `bossctl` names the OS user, cards
  name the reacting member, chat-drafted cards name the asker, slash commands
  name the invoker. An Audit page in the portal, `GET /api/audit`,
  `bossctl audit`.
- Container hardening: read-only root filesystem, all capabilities dropped,
  no-new-privileges, memory and pid caps. Dependabot version bumps, security
  alerts and secret-scanning push protection on the repository.

**Changed**

- Chat write tools are scoped server-side: proposing a change to an existing
  run requires being on it (or owning the weekly timing behind it) and asking
  from its home channel; admins are exempt. Retiring superseded cards is
  channel-scoped the same way, so a draft raised elsewhere can no longer bury
  a party's pending card.
- Schedule answers mark finished runs as already happened and say plainly when
  nothing upcoming is left, instead of leaving the arithmetic to the model.
- The system prompt states the configured chat model and the developer
  attribution, so "what model are you on" and "who made you" get facts, not
  inventions.
- Documentation split: setup, commands, extractor, chatbot, portal and
  development each have their own guide under `docs/`; the README is a pitch
  and an index. Licensed under MIT.

**Fixed**

- Member text can no longer impersonate the scheduler's own bracketed notes to
  the model; the note shapes are defused where member text enters the prompt,
  and guild tags like `[SAKU]` pass untouched.
- The persona voice reminder now actually arrives last: gpt-oss's template
  hoists trailing system messages into the top instructions header, so it is
  sent as a user-role scheduler note instead — which is also why card
  confirmations kept coming out flat.
- A blank `API_PORT=` line in `.env` no longer silently fails the container
  healthcheck.

## 1.9.0

**Added**

- `/say` — admins post as the bot, verbatim. The mention allow-list is built from
  the `@mentions` in the text, so the message reaches exactly who it names and
  nobody else. `@everyone`/`@here` is always blocked, and quiet mode silences it
  like everything else.
- `ADMIN_ROLE_ID` is now the "who runs the bot" role: it grants `/say`, `/debug`
  and the right to change any run, not just your own. Discord's own Administrator
  permission and the server owner qualify too, so leaving the setting empty locks
  nobody out.
- The portal records who answered a run and when, alongside the reaction tally.
- Continuous integration: lint, format check and the full offline test suite.

**Changed**

- `/say` and `/debug` no longer appear in a non-admin's command picker, and the
  permission is checked again when the command runs — a server can hand the
  picker entry back out, so hiding it is not the gate.
- Countdown pings now go to everyone on the run except those who have declined.
  An hour out, the people who are coming want the reminder whether or not they
  have ticked; somebody who reacted ❌ has already answered and is named on the
  card without being pinged again.

**Fixed**

- Posted reminder cards no longer freeze at the tally they had when they were
  sent. Every write that changes what a card shows — a reaction, `/rsvp`, the
  portal, an RSVP extracted from chat — queues a re-render, so a card that still
  read "confirmed · 2/4 ✅" hours after everyone had answered now keeps up. Card
  edits carry the same mention allow-list as the original send, so refreshing can
  never become a second way to ping.

## 1.8.0

**Added**

- The weekly digest posts automatically at boss-week reset, idempotent across
  restarts and slept-through resets.
- Nightly database backup: one SQLite online-backup snapshot per local day,
  written to `data/backups` on the host, converted out of WAL so each file is
  self-contained, and pruned to the newest fourteen.

**Changed**

- The digest distinguishes runs that are at risk because somebody declined from
  those that are merely unconfirmed, and counts them separately.

**Fixed**

- Removing a ❌ reaction now retracts the reschedule notice, matching what
  `/rsvp` and the portal already did.
- A fully answered run with someone out no longer reads as all-confirmed.

## 1.7.0

**Added**

- Quiet mode: a runtime toggle that posts everything with an empty mention
  allow-list and a bell marker, for working against a live guild without
  notifying it.
- Post resilience — bounded retry on DNS and timeout failures, stranded proposals
  re-posted, and one channel's failing rescan no longer affects the others.
- Portal: dialog editors on the Fixed page, rendered mentions and move arrows in
  the inbox, and a quiet-mode toggle on Config.

**Changed**

- Prompts are token-budgeted against a calibrated estimator. Oversized message
  bursts are read in chunks but still consolidated onto a single card.
- Card arbitration: decisions beat questions for the same run, the latest
  evidence wins ties, and ambiguous matches are split or dropped rather than
  guessed.
- Compose: the database moved to a named volume with a 60-second stop grace
  period, after a hard kill mid-write corrupted it.

**Fixed**

- A partial move inherits the matched run's own day and time instead of
  resolving to TBD.
- Evidence can no longer match a run in a later boss week, so next week's runs
  cannot be dragged backwards.
- A `sub` with no named replacement proposes a plain weekly removal rather than
  claiming a stand-in is needed.
- An incomplete or retracted tally no longer demotes an already-confirmed run;
  only a decline or a line-up change does.

## 1.6.0

**Added**

- Per-member ping levels — `/pings essential|all|off`, also settable from the
  portal, the API and `bossctl`.
- A single mention resolver: only day-of cards, unanswered countdowns, proposal
  cards and decline notices notify anyone. Every other post names people in
  plain text.
- A live per-channel Manage Messages check, surfaced on the portal's Config
  page, in `/debug status` and in `bossctl access`.
- `scripts/bench_extract.py`, the benchmark behind the current model choice.

**Fixed**

- One timing change per run per card, chosen by precedence, instead of a card
  carrying two contradictory amendments for the same run.
- Run hints and matches now require a shared boss, so an amendment can no longer
  land on an unrelated run.
- Moves, `otot`s and cancels that would change nothing are dropped rather than
  proposed.
- Deleting a card marks its proposals withdrawn, so they leave the inbox.

## 1.5.0

**Added**

- Rescans run on a queue instead of blocking the bot. A request returns a job id
  with progress, cancellation, and a list of recent jobs.
- Per-run member swap for a single week — `/swap`, `bossctl swap`, the API, and a
  chip UI in the portal.

**Changed**

- Rescan bursts are grouped by local calendar day, carry the 25 messages before
  them as context, and produce one consolidated card per channel per rescan.
- Automated rescans are capped at 48 hours and never widen into the previous week.

**Fixed**

- Day-only amendments whose day has already passed are dropped as stale.
- A day's single stated time carries onto same-day moves that lack one.

## 1.4.0

**Added**

- Week-wide rescan that pulls Discord history first, plus a startup backfill for
  each watched channel.
- Explicit run status control — planned, confirmed, otot, done, cancelled — from
  `/status`, the portal, the API and `bossctl`.
- Portal: in-game difficulty pills, a boss-grid picker, a bosses page, and boss
  portraits with a monogram fallback.

**Changed**

- A run is marked done once its slot has passed, and drops out of `/schedule`,
  the portal and `bossctl` by default.
- Every change made through the portal or CLI is announced in the run's home
  channel, marked *(via portal)*.

**Fixed**

- Open redirect on the `next=` parameter of the login route.

## 1.3.0

**Added**

- An HTTP API served by uvicorn inside the bot's own asyncio loop, bound to
  loopback. Bearer-token auth, opt-in Tailscale identity, and a signed session
  cookie; the health endpoint stays unauthenticated.
- The web portal — week view, fixed-timing editor, proposal inbox, extraction
  log, members, reminders and config. Light and dark, and every form works
  without JavaScript.
- `bossctl`, covering the same operations from a terminal.
- The weekly digest card.

## 1.2.0

**Added**

- Chat extraction: a keyword gate with fuzzy boss aliases, a structured-output
  schema, and a deterministic merge, resolve and match pipeline that proposes
  `move`, `add`, `cancel`, `otot`, `sub`, `split` and `fix` amendments as cards.
- A proposal applies only when a participant with the bossing role reacts ✅, and
  expires after 24 hours. An RSVP stated in chat is the one exception and is
  recorded straight away.
- `/rescan`, `/debug extract`, and `python -m bot.extract` for offline dry runs
  over an exported channel.
- A fixture suite runnable against the live model with `pytest -m ollama`.

## 1.1.0

**Added**

- Day-of and countdown reminders are embeds carrying full boss names and
  difficulty.
- A stale-reminder guard, so a host that was asleep does not replay old pings.

**Changed**

- ✅ and ❌ are mutually exclusive. Decline notices are deduplicated, and
  retracted when the decline is withdrawn.
- `/fixed add` no longer adds its creator automatically — only the participants
  named are pinged.
- Times may be typed as `2359`, `930`, `9pm` or `9:30pm`.

## 1.0.0

First working release.

**Added**

- Roster synced from the bossing role, with no manual upkeep.
- `/fixed` baseline timings, materialised into concrete runs at each boss-week
  reset, with the current and next week always populated.
- Day-of and countdown reminders, stored as rows in SQLite so a restart never
  loses or replays a ping.
- ✅/❌ reactions driving run status.
- Deployment with Docker Compose.
