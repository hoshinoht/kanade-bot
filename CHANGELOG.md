# Changelog

Notable changes to the Boss Scheduler Bot, newest first.

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
